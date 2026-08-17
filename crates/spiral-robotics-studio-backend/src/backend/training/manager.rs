//! Rust-owned supervision for Spiral ADBC and an injected training backend.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc as std_mpsc};
use std::time::{Duration, Instant};

use spiral_robotics_contracts::api::{
    ConnectionTestResponse, MAX_ROLLOUT_ACTION_STEPS, MAX_TRAINING_HISTORY_RUNS,
    StartTrainingRequest, TrainingCheckpoint, TrainingMetricPoint, TrainingReadiness, TrainingRun,
    TrainingRunPhase, TrainingRuntimeInfo, TrainingSnapshot, TrainingSource, TrainingUpdate,
};
use spiral_robotics_contracts::spiral;
use spiral_robotics_training::{
    ActTrainingConfig, ActTrainingJobConfig, PendingTrainingStep, SpiralJobConnection,
    TRAINING_BACKEND_API_VERSION, TrainingBackend, TrainingCheckpointRequest,
    TrainingInitialization, TrainingJobPhase, TrainingJobRequest, TrainingJobSnapshot,
    TrainingModelConfig, TrainingSession, TrainingSessionInfo, TrainingSessionRequest,
};
use tokio::sync::{broadcast, mpsc};

use crate::backend::adapters::clock::SystemClock;
use crate::backend::application::checkpoints::{
    CheckpointRepository, CheckpointWrite, validate_checkpoint_artifact_size,
};
use crate::backend::application::clock::{Clock, StableIdSequence};
use crate::backend::application::error::ApplicationError;
use crate::backend::application::policy_runtime::{
    ACT_ARTIFACT_FORMAT, ACT_TENSOR_NAMESPACE, ArtifactIdentity, validate_artifact_identity,
};
use crate::backend::application::training::{
    ActTrainingSettings as ApplicationActTrainingSettings,
    StartTrainingCommand as ApplicationStartTrainingCommand,
    TrainingCheckpoint as ApplicationTrainingCheckpoint,
    TrainingMetric as ApplicationTrainingMetric, TrainingPolicy as ApplicationTrainingPolicy,
    TrainingReadiness as ApplicationTrainingReadiness, TrainingRun as ApplicationTrainingRun,
    TrainingRunPhase as ApplicationTrainingRunPhase, TrainingSource as ApplicationTrainingSource,
};
use crate::backend::application::training_execution::{
    self, RunStateChange, TrainingCoordinatorPort, TrainingLaunch, TrainingRunIdentity,
    TrainingRunPort, TrainingRuntimeEvent, TrainingRuntimeOutcome, TrainingRuntimePort,
};
use crate::backend::application::training_population::NormalizationStats;
use crate::backend::application::training_runs::{
    RunPhase as ApplicationRunPhase, RunRecord, TrainingRunRegistry,
};
use crate::backend::storage::manager::StorageManager;
use crate::backend::storage::repository::RepositoryConnection;
use crate::backend::training::data::{LoadedActBatch, LoaderMetrics, SpiralActLoader};

const PREFETCH_BUFFER_SIZE: usize = 1;
const LOADER_STALL: Duration = Duration::from_secs(2);
static RUN_IDS: StableIdSequence = StableIdSequence::new("train", 0);

#[derive(Clone)]
pub(crate) struct TrainingManager {
    inner: Arc<TrainingManagerInner>,
}

struct TrainingManagerInner {
    storage: Arc<StorageManager>,
    training_backend: Arc<dyn TrainingBackend>,
    ffmpeg: String,
    clock: Arc<dyn Clock>,
    snapshot: Mutex<TrainingSnapshot>,
    updates: broadcast::Sender<TrainingUpdate>,
    controls: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
}

enum BackendEvent {
    Configured(Box<StartTrainingRequest>),
    Started(String),
    Metric(TrainingMetricPoint),
    Checkpoint(BackendCheckpoint),
    Log(String),
}

pub(crate) struct BackendCheckpoint {
    step: u64,
    created_ms: i64,
    validation_loss: Option<f64>,
    manifest: serde_json::Value,
    payload: Vec<u8>,
    acknowledged: std::sync::mpsc::SyncSender<Result<(), String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrefetchedBatchKind {
    Training,
    Validation,
}

enum PrefetchMessage {
    Batch {
        step: u64,
        kind: PrefetchedBatchKind,
        result: Result<LoadedActBatch, String>,
    },
    Stopped,
}

struct ActDataContract {
    state_dimensions: usize,
    action_dimensions: usize,
    image_width: usize,
    image_height: usize,
    stats: NormalizationStats,
    joint_names: Vec<String>,
    robot_type: String,
    sample_hz: f64,
}

impl ActDataContract {
    fn from_loader(loader: &SpiralActLoader<'_>) -> Self {
        Self {
            state_dimensions: loader.state_dimensions(),
            action_dimensions: loader.action_dimensions(),
            image_width: loader.image_width(),
            image_height: loader.image_height(),
            stats: loader.stats.clone(),
            joint_names: loader.joint_names.clone(),
            robot_type: loader.robot_type.clone(),
            sample_hz: loader.sample_hz,
        }
    }
}

impl TrainingManager {
    /// Construct the in-memory manager without touching external services.
    pub(crate) fn new(
        storage: Arc<StorageManager>,
        training_backend: Arc<dyn TrainingBackend>,
        ffmpeg: String,
    ) -> Self {
        Self::new_with_clock(storage, training_backend, ffmpeg, Arc::new(SystemClock))
    }

    fn new_with_clock(
        storage: Arc<StorageManager>,
        training_backend: Arc<dyn TrainingBackend>,
        ffmpeg: String,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let (updates, _) = broadcast::channel(512);
        Self {
            inner: Arc::new(TrainingManagerInner {
                storage,
                training_backend,
                ffmpeg,
                clock,
                snapshot: Mutex::new(TrainingSnapshot::default()),
                updates,
                controls: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// Restore persisted history after the HTTP listener is already serving.
    pub(crate) async fn hydrate_from_storage(&self) -> Result<(), String> {
        let storage = self.inner.storage.clone();
        let mut snapshot = tokio::task::spawn_blocking(move || storage.load_training_snapshot())
            .await
            .map_err(|error| format!("training history task stopped: {error}"))??;
        let run_registry = registry(&snapshot)?;
        let retained = run_registry
            .retained_ids()
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>();
        snapshot.runs.retain(|run| retained.contains(&run.id));
        // Readiness is live service state, not durable history. Preserve the
        // concurrently computed value if probes finish before hydration.
        let mut current = lock(&self.inner.snapshot);
        snapshot.readiness = current.readiness.clone();
        *current = snapshot.clone();
        drop(current);
        let _ = self.inner.updates.send(TrainingUpdate::Snapshot {
            snapshot: Box::new(snapshot.clone()),
        });
        // A worker-owned job continues while Studio is down. Re-submit the
        // immutable ID (the runtime treats POST as idempotent) and resume
        // polling its replayable state instead of marking the run failed.
        for run in snapshot.runs.into_iter().filter(|run| run.phase.active()) {
            let cancel = Arc::new(AtomicBool::new(false));
            lock(&self.inner.controls).insert(run.id.clone(), cancel.clone());
            let manager = self.clone();
            tokio::spawn(async move {
                manager
                    .run_backend(
                        TrainingLaunch {
                            run_id: run.id,
                            command: application_training_command(run.config),
                        },
                        cancel,
                    )
                    .await;
            });
        }
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> TrainingSnapshot {
        lock(&self.inner.snapshot).clone()
    }

    pub(crate) fn reload_from_storage(&self) -> Result<(), String> {
        if !lock(&self.inner.controls).is_empty() {
            return Err("cannot reload training history while a run is active".to_owned());
        }
        let snapshot = self.inner.storage.load_training_snapshot()?;
        registry(&snapshot)?;
        *lock(&self.inner.snapshot) = snapshot;
        Ok(())
    }

    /// Capture the initial state and subscribe to later changes as one ordered
    /// operation. Publishers send while holding the same snapshot lock, so an
    /// update can appear in the snapshot or the receiver, never both.
    pub(crate) fn subscription(&self) -> (TrainingSnapshot, broadcast::Receiver<TrainingUpdate>) {
        let snapshot = lock(&self.inner.snapshot);
        let updates = self.inner.updates.subscribe();
        (snapshot.clone(), updates)
    }

    pub(crate) async fn refresh_readiness(&self) -> TrainingReadiness {
        let next = readiness(&self.inner.storage, &self.inner.training_backend).await;
        if lock(&self.inner.snapshot).readiness != next {
            let _ = self.apply_update(
                TrainingUpdate::Readiness {
                    readiness: next.clone(),
                },
                false,
            );
        }
        next
    }

    pub(crate) async fn test_runtime(&self) -> ConnectionTestResponse {
        match probe_training_backend(&self.inner.training_backend).await {
            Ok(info) => ConnectionTestResponse {
                ok: true,
                message: format!("Connected · {}", info.summary()),
            },
            Err(error) => ConnectionTestResponse {
                ok: false,
                message: error,
            },
        }
    }

    pub(crate) async fn start(&self, request: StartTrainingRequest) -> Result<TrainingRun, String> {
        training_execution::coordinate_training_start(self, application_training_command(request))
            .await
            .map(contract_training_run)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn stop(&self, run_id: &str) -> Result<(), String> {
        training_execution::coordinate_training_stop(self, run_id)
            .map_err(|error| error.to_string())
    }

    /// Remove checkpoint rows before saving the run-history
    /// removal. A failed checkpoint deletion therefore leaves the complete run
    /// visible and retryable instead of hiding orphaned model artifacts.
    pub(crate) async fn delete(&self, run_id: &str) -> Result<(), String> {
        if lock(&self.inner.controls).contains_key(run_id) {
            return Err(format!(
                "training run {run_id:?} is still active; stop it before deleting it"
            ));
        }
        let run = lock(&self.inner.snapshot)
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .cloned()
            .ok_or_else(|| format!("training run {run_id:?} is no longer available"))?;
        if run.phase.active() {
            return Err(format!(
                "training run {run_id:?} is still active; stop it before deleting it"
            ));
        }

        self.inner
            .storage
            .delete_run_checkpoints(run_id.to_owned())
            .await?;

        if lock(&self.inner.controls).contains_key(run_id) {
            return Err(format!(
                "training run {run_id:?} became active while deletion was in progress"
            ));
        }
        let update = TrainingUpdate::RunRemoved {
            run_id: run_id.to_owned(),
        };
        let mut snapshot = lock(&self.inner.snapshot);
        let current = snapshot
            .runs
            .iter()
            .find(|candidate| candidate.id == run_id)
            .ok_or_else(|| format!("training run {run_id:?} is no longer available"))?;
        if current.phase.active() {
            return Err(format!(
                "training run {run_id:?} became active while deletion was in progress"
            ));
        }
        let mut candidate = snapshot.clone();
        candidate.apply(update.clone());
        self.inner.storage.save_training_snapshot(&candidate)?;
        *snapshot = candidate;
        let _ = self.inner.updates.send(update);
        drop(snapshot);
        Ok(())
    }

    pub(crate) async fn shutdown(&self) {
        // A Studio shutdown only detaches the orchestration client. The remote worker owns
        // the job and must keep running so a restarted Studio process can reattach to it.
        lock(&self.inner.controls).clear();
    }

    async fn run_backend(&self, launch: TrainingLaunch, cancel: Arc<AtomicBool>) {
        let run_id = launch.run_id;
        let request = contract_training_request(launch.command);
        let credentials = self.inner.storage.spiral_credentials();
        let job_request = TrainingJobRequest {
            job_id: run_id.clone(),
            run_name: request.run_name.clone(),
            spiral: SpiralJobConnection {
                api_url: self.inner.storage.spiral_endpoint(),
                spfs_url: self.inner.storage.spiral_spfs_endpoint(),
                project_id: self.inner.storage.project_id(),
                namespace: self.inner.storage.namespace().to_owned(),
                client_id: credentials
                    .as_ref()
                    .map(|credentials| credentials.client_id.clone()),
                client_secret: credentials
                    .as_ref()
                    .map(|credentials| credentials.client_secret.clone()),
            },
            sql: request.sql.clone(),
            state_column: request.state_column.clone(),
            action_column: request.action_column.clone(),
            device: request.device.clone(),
            initialization: if request.act.pretrained_backbone {
                TrainingInitialization::PretrainedBackbone
            } else {
                TrainingInitialization::Random
            },
            steps: request.steps,
            batch_size: request.batch_size,
            validation_split: request.validation_split,
            learning_rate: request.learning_rate,
            checkpoint_every: request.checkpoint_every,
            chunk_size: request.chunk_size,
            seed: request.seed,
            act: ActTrainingJobConfig {
                camera_streams: request.act.camera_streams.clone(),
                image_width: request.act.image_width,
                image_height: request.act.image_height,
                video_decoder: request.act.video_decoder.clone(),
                hidden_dimensions: request.act.hidden_dim,
                latent_dimensions: request.act.latent_dim,
                attention_heads: request.act.attention_heads,
                feedforward_dimensions: request.act.feedforward_dim,
                encoder_layers: request.act.encoder_layers,
                vae_encoder_layers: request.act.vae_encoder_layers,
                decoder_layers: request.act.decoder_layers,
                kl_weight: request.act.kl_weight,
            },
        };
        let backend = self.inner.training_backend.clone();
        let existing_run_id = run_id.clone();
        let initial = tokio::task::spawn_blocking(move || match backend.job(&existing_run_id) {
            Ok(snapshot) => Ok(snapshot),
            Err(_) => backend.create_job(job_request),
        })
        .await;
        let mut snapshot = match initial {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                self.finish_remote_job(&run_id, TrainingJobPhase::Failed, 0, error.to_string());
                return;
            }
            Err(error) => {
                self.finish_remote_job(
                    &run_id,
                    TrainingJobPhase::Failed,
                    0,
                    format!("training job launch task stopped: {error}"),
                );
                return;
            }
        };
        let persisted_run = lock(&self.inner.snapshot)
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .cloned();
        let persisted_metric_step = persisted_run
            .as_ref()
            .and_then(|run| run.metrics.iter().map(|metric| metric.step).max())
            .unwrap_or(0);
        let persisted_checkpoint_step = persisted_run
            .as_ref()
            .and_then(|run| {
                run.checkpoints
                    .iter()
                    .map(|checkpoint| checkpoint.step)
                    .max()
            })
            .unwrap_or(0);
        let mut metric_count = snapshot
            .metrics
            .partition_point(|metric| metric.step <= persisted_metric_step);
        let mut checkpoint_count = snapshot
            .checkpoints
            .partition_point(|checkpoint| checkpoint.step <= persisted_checkpoint_step);
        let mut log_count = persisted_run
            .as_ref()
            .map(|run| {
                snapshot
                    .logs
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| run.logs.contains(line))
                    .map(|(index, _)| index + 1)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        let mut configured = false;
        let mut stop_sent = false;
        let mut poll_failures = 0_u64;
        loop {
            if let Err(error) = self.apply_remote_job_snapshot(
                &run_id,
                &request,
                &snapshot,
                &mut metric_count,
                &mut checkpoint_count,
                &mut log_count,
                &mut configured,
            ) {
                self.finish_remote_job(
                    &run_id,
                    TrainingJobPhase::Failed,
                    snapshot.current_step,
                    error,
                );
                return;
            }
            if snapshot.phase.terminal() {
                self.finish_remote_job(
                    &run_id,
                    snapshot.phase,
                    snapshot.current_step,
                    snapshot.message.clone().unwrap_or_default(),
                );
                return;
            }
            let backend = self.inner.training_backend.clone();
            let next_run_id = run_id.clone();
            let should_stop = cancel.load(Ordering::Acquire) && !stop_sent;
            tokio::time::sleep(Duration::from_millis(250)).await;
            let next = tokio::task::spawn_blocking(move || {
                if should_stop {
                    backend.stop_job(&next_run_id)
                } else {
                    backend.job(&next_run_id)
                }
            })
            .await;
            snapshot = match next {
                Ok(Ok(snapshot)) => {
                    if should_stop {
                        stop_sent = true;
                    }
                    if poll_failures > 0 {
                        self.push_log(
                            &run_id,
                            "Reconnected to the remote training worker.".to_owned(),
                        );
                    }
                    poll_failures = 0;
                    snapshot
                }
                Ok(Err(error)) => {
                    poll_failures += 1;
                    if poll_failures == 1 {
                        self.push_log(
                            &run_id,
                            format!(
                                "Remote training worker is temporarily unavailable; retrying: {error}"
                            ),
                        );
                    }
                    continue;
                }
                Err(error) => {
                    self.finish_remote_job(
                        &run_id,
                        TrainingJobPhase::Failed,
                        snapshot.current_step,
                        format!("training job poll task stopped: {error}"),
                    );
                    return;
                }
            };
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_remote_job_snapshot(
        &self,
        run_id: &str,
        request: &StartTrainingRequest,
        snapshot: &TrainingJobSnapshot,
        metric_count: &mut usize,
        checkpoint_count: &mut usize,
        log_count: &mut usize,
        configured: &mut bool,
    ) -> Result<(), String> {
        if snapshot.job_id != run_id {
            return Err("training runtime returned the wrong job ID".to_owned());
        }
        if !*configured && let Some(resolved) = &snapshot.resolved {
            let mut resolved_request = request.clone();
            resolved_request.act.camera_streams = resolved.camera_streams.clone();
            self.apply_update(
                TrainingUpdate::RunConfigured {
                    run_id: run_id.to_owned(),
                    config: Box::new(resolved_request),
                },
                true,
            )?;
            *configured = true;
        }
        if snapshot.phase == TrainingJobPhase::Running
            && self.phase(run_id) == Some(TrainingRunPhase::Starting)
        {
            self.set_phase(
                run_id,
                TrainingRunPhase::Running,
                None,
                snapshot.message.clone(),
            )?;
        }
        for metric in snapshot.metrics.iter().skip(*metric_count) {
            self.apply_update(
                TrainingUpdate::Metric {
                    run_id: run_id.to_owned(),
                    point: Box::new(TrainingMetricPoint {
                        step: metric.step,
                        train_loss: metric.train_loss,
                        validation_loss: metric.validation_loss,
                        learning_rate: metric.learning_rate,
                        transfer_bytes_per_second: 0.0,
                        examples_per_second: metric.examples_per_second,
                        cpu_percent: metric.cpu_percent,
                        gpu_percent: metric.gpu_percent,
                        memory_bytes: metric.memory_bytes,
                        loader_ms: metric.loader_ms,
                        arrow_encode_ms: None,
                        upload_ms: None,
                        device_copy_ms: metric.device_copy_ms,
                        model_ms: metric.model_ms,
                        elapsed_seconds: metric.elapsed_seconds,
                        event: metric.event.clone(),
                    }),
                },
                false,
            )?;
        }
        *metric_count = snapshot.metrics.len();
        for checkpoint in snapshot.checkpoints.iter().skip(*checkpoint_count) {
            self.inner.storage.invalidate_idle_connections();
            self.apply_update(
                TrainingUpdate::Checkpoint {
                    run_id: run_id.to_owned(),
                    checkpoint: TrainingCheckpoint {
                        step: checkpoint.step,
                        created_ms: checkpoint.created_ms,
                        validation_loss: checkpoint.validation_loss,
                        byte_size: checkpoint.byte_size,
                        inference_ready: true,
                        recommended_action_steps: checkpoint.recommended_action_steps,
                        sample_hz: checkpoint.sample_hz,
                    },
                },
                true,
            )?;
        }
        *checkpoint_count = snapshot.checkpoints.len();
        for line in snapshot.logs.iter().skip(*log_count) {
            self.push_log(run_id, line.clone());
        }
        *log_count = snapshot.logs.len();
        Ok(())
    }

    fn finish_remote_job(
        &self,
        run_id: &str,
        phase: TrainingJobPhase,
        current_step: u64,
        message: String,
    ) {
        let phase = match phase {
            TrainingJobPhase::Completed => TrainingRunPhase::Completed,
            TrainingJobPhase::Stopped => TrainingRunPhase::Stopped,
            TrainingJobPhase::Stopping => TrainingRunPhase::Stopping,
            TrainingJobPhase::Starting | TrainingJobPhase::Running | TrainingJobPhase::Failed => {
                TrainingRunPhase::Failed
            }
        };
        let update = TrainingUpdate::RunState {
            run_id: run_id.to_owned(),
            phase,
            current_step,
            finished_ms: (!phase.active()).then(|| self.inner.clock.now_ms()),
            message: (!message.is_empty()).then_some(message),
        };
        let _ = self.apply_update(update, true);
        lock(&self.inner.controls).remove(run_id);
    }

    #[allow(clippy::too_many_arguments)]
    async fn save_checkpoint_bytes(
        &self,
        run_id: &str,
        request: &StartTrainingRequest,
        step: u64,
        created_ms: i64,
        validation_loss: Option<f64>,
        manifest: &serde_json::Value,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        if step == 0 || step > request.steps {
            return Err(format!(
                "trainer returned checkpoint step {step}, outside this run"
            ));
        }
        if validation_loss.is_some_and(|loss| !loss.is_finite()) {
            return Err("trainer returned a non-finite validation loss".to_owned());
        }
        let byte_size = u64::try_from(payload.len())
            .map_err(|_| "checkpoint byte size does not fit u64".to_owned())?;
        let file_name = format!("checkpoint-{step}.safetensors");
        let config_json =
            validate_checkpoint_artifact(&file_name, &payload, manifest, run_id, step, request)?;
        validate_checkpoint_artifact_size(config_json.len(), payload.len())?;
        let policy = serde_json::to_value(request.policy)
            .map_err(|error| format!("could not encode checkpoint policy: {error}"))?;
        let policy = policy
            .as_str()
            .ok_or_else(|| "checkpoint policy did not encode as a name".to_owned())?
            .to_owned();
        let sample_hz = checkpoint_sample_hz(manifest)?;
        self.inner
            .storage
            .save_checkpoint(CheckpointWrite {
                run_id: run_id.to_owned(),
                run_name: request.run_name.clone(),
                policy,
                step,
                created_ms,
                validation_loss,
                config_json,
                payload,
            })
            .await?;
        self.apply_update(
            TrainingUpdate::Checkpoint {
                run_id: run_id.to_owned(),
                checkpoint: TrainingCheckpoint {
                    step,
                    created_ms,
                    validation_loss,
                    byte_size,
                    inference_ready: true,
                    recommended_action_steps: request.chunk_size.min(MAX_ROLLOUT_ACTION_STEPS),
                    sample_hz,
                },
            },
            true,
        )?;
        Ok(())
    }

    fn phase(&self, run_id: &str) -> Option<TrainingRunPhase> {
        lock(&self.inner.snapshot)
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .map(|run| run.phase)
    }

    fn set_phase(
        &self,
        run_id: &str,
        phase: TrainingRunPhase,
        finished_ms: Option<i64>,
        message: Option<String>,
    ) -> Result<(), String> {
        let mut snapshot = lock(&self.inner.snapshot);
        let current_step = snapshot
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .map_or(0, |run| run.current_step);
        let mut registry = registry(&snapshot)?;
        registry.transition(run_id, application_phase(phase))?;
        let update = TrainingUpdate::RunState {
            run_id: run_id.to_owned(),
            phase,
            current_step,
            finished_ms,
            message,
        };
        let mut candidate = snapshot.clone();
        candidate.apply(update.clone());
        self.inner.storage.save_training_snapshot(&candidate)?;
        *snapshot = candidate;
        let _ = self.inner.updates.send(update);
        Ok(())
    }

    fn mark_persistence_failure(&self, run_id: &str, error: &str) {
        if let Some(cancel) = lock(&self.inner.controls).get(run_id).cloned() {
            cancel.store(true, Ordering::Release);
        }
        let mut snapshot = lock(&self.inner.snapshot);
        let Ok((log_update, state_update)) =
            persistence_failure_updates(&snapshot, run_id, self.inner.clock.now_ms(), error)
        else {
            return;
        };
        // The persistence boundary itself is unavailable, so this terminal
        // evidence can only be kept in memory and broadcast. Retrying the same
        // failed write here would hide the failure behind recursion.
        snapshot.apply(log_update.clone());
        snapshot.apply(state_update.clone());
        let _ = self.inner.updates.send(log_update);
        let _ = self.inner.updates.send(state_update);
    }

    fn push_log(&self, run_id: &str, line: String) {
        let _ = self.apply_update(
            TrainingUpdate::Log {
                run_id: run_id.to_owned(),
                line,
            },
            false,
        );
    }

    fn apply_update(&self, update: TrainingUpdate, persist: bool) -> Result<(), String> {
        let mut snapshot = lock(&self.inner.snapshot);
        let mut candidate = snapshot.clone();
        candidate.apply(update.clone());
        if persist {
            self.inner.storage.save_training_snapshot(&candidate)?;
        }
        *snapshot = candidate;
        let _ = self.inner.updates.send(update);
        Ok(())
    }
}

impl TrainingRunPort for TrainingManager {
    fn runtime_is_active(&self, run_id: &str) -> bool {
        lock(&self.inner.controls).contains_key(run_id)
    }

    fn transition_run(&self, run_id: &str, change: RunStateChange) -> Result<(), String> {
        self.set_phase(
            run_id,
            contract_training_phase(change.phase),
            change.finished_ms,
            change.message,
        )
    }

    fn signal_runtime_stop(&self, run_id: &str) {
        if let Some(cancel) = lock(&self.inner.controls).get(run_id).cloned() {
            cancel.store(true, Ordering::Release);
        }
    }

    fn mark_persistence_failure(&self, run_id: &str, error: &str) {
        TrainingManager::mark_persistence_failure(self, run_id, error);
    }

    fn cleanup_runtime(&self, run_id: &str) {
        lock(&self.inner.controls).remove(run_id);
    }

    fn now_ms(&self) -> i64 {
        self.inner.clock.now_ms()
    }
}

impl TrainingCoordinatorPort for TrainingManager {
    async fn refresh_training_readiness(&self) -> ApplicationTrainingReadiness {
        application_training_readiness(self.refresh_readiness().await)
    }

    fn resolve_training_source(
        &self,
        dataset_id: &str,
    ) -> Result<ApplicationTrainingSource, ApplicationError> {
        Ok(ApplicationTrainingSource {
            dataset_id: dataset_id.to_owned(),
        })
    }

    async fn validate_training_query(
        &self,
        _command: &ApplicationStartTrainingCommand,
    ) -> Result<(), ApplicationError> {
        // The worker owns the Spiral connection and validates the immutable
        // query while resolving its data contract. Studio does not execute the
        // training query or read a training batch.
        Ok(())
    }

    fn next_training_identity(&self) -> TrainingRunIdentity {
        let identity = RUN_IDS.next(self.inner.clock.as_ref());
        TrainingRunIdentity {
            id: identity.value,
            created_ms: identity.created_ms,
        }
    }

    fn admit_training_run(&self, run: ApplicationTrainingRun) -> Result<(), ApplicationError> {
        // Registry admission, durable snapshot replacement, and event delivery
        // share one lock, so concurrent starts cannot both become active.
        let mut snapshot = lock(&self.inner.snapshot);
        let (candidate, update) = admitted_snapshot(
            self.inner.storage.as_ref(),
            &snapshot,
            contract_training_run(run),
        )
        .map_err(ApplicationError::conflict)?;
        *snapshot = candidate;
        let _ = self.inner.updates.send(update);
        Ok(())
    }

    fn launch_training_runtime(&self, launch: TrainingLaunch) -> Result<(), ApplicationError> {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut controls = lock(&self.inner.controls);
            if controls.contains_key(&launch.run_id) {
                return Err(ApplicationError::Internal(format!(
                    "training runtime {:?} was already launched",
                    launch.run_id
                )));
            }
            controls.insert(launch.run_id.clone(), cancel.clone());
        }
        let manager = self.clone();
        tokio::spawn(async move {
            manager.run_backend(launch, cancel).await;
        });
        Ok(())
    }
}

impl TrainingRuntimePort for TrainingManager {
    type CheckpointEnvelope = BackendCheckpoint;

    fn run_phase(&self, run_id: &str) -> Option<ApplicationTrainingRunPhase> {
        self.phase(run_id).map(application_training_phase)
    }

    fn persist_resolved_configuration(
        &self,
        run_id: &str,
        command: &ApplicationStartTrainingCommand,
    ) -> Result<(), String> {
        self.apply_update(
            TrainingUpdate::RunConfigured {
                run_id: run_id.to_owned(),
                config: Box::new(contract_training_request(command.clone())),
            },
            true,
        )
    }

    fn record_metric(&self, run_id: &str, metric: ApplicationTrainingMetric) {
        let _ = self.apply_update(
            TrainingUpdate::Metric {
                run_id: run_id.to_owned(),
                point: Box::new(contract_training_metric(metric)),
            },
            false,
        );
    }

    fn record_log(&self, run_id: &str, message: String) {
        self.push_log(run_id, message);
    }

    async fn persist_runtime_checkpoint(
        &self,
        run_id: &str,
        command: &ApplicationStartTrainingCommand,
        checkpoint: Self::CheckpointEnvelope,
    ) -> Result<(), String> {
        let BackendCheckpoint {
            step,
            created_ms,
            validation_loss,
            manifest,
            payload,
            acknowledged,
        } = checkpoint;
        let result = self
            .save_checkpoint_bytes(
                run_id,
                &contract_training_request(command.clone()),
                step,
                created_ms,
                validation_loss,
                &manifest,
                payload,
            )
            .await;
        let _ = acknowledged.send(result.clone());
        result
    }

    fn reject_runtime_checkpoint(&self, checkpoint: Self::CheckpointEnvelope, error: String) {
        let _ = checkpoint.acknowledged.send(Err(error));
    }
}

#[allow(clippy::too_many_arguments)]
fn train_blocking(
    connection: &mut RepositoryConnection,
    training_backend: &dyn TrainingBackend,
    run_id: &str,
    request: &StartTrainingRequest,
    namespace: &str,
    ffmpeg: &str,
    clock: Arc<dyn Clock>,
    cancel: Arc<AtomicBool>,
    events: mpsc::UnboundedSender<BackendEvent>,
) -> Result<TrainingRuntimeOutcome, String> {
    let _ = events.send(BackendEvent::Log(
        "Opening the dedicated Spiral ADBC connection and indexing eligible ACT anchors…"
            .to_owned(),
    ));
    let mut loader = SpiralActLoader::open(connection, request, namespace, ffmpeg)?;
    let mut resolved_request = request.clone();
    resolved_request.act.camera_streams = loader.camera_streams().to_vec();
    let _ = events.send(BackendEvent::Configured(Box::new(resolved_request.clone())));
    let config = ActTrainingConfig {
        state_dimensions: loader.state_dimensions(),
        action_dimensions: loader.action_dimensions(),
        camera_streams: resolved_request.act.camera_streams.clone(),
        image_width: loader.image_width(),
        image_height: loader.image_height(),
        chunk_size: request.chunk_size,
        hidden_dimensions: request.act.hidden_dim,
        latent_dimensions: request.act.latent_dim,
        attention_heads: request.act.attention_heads,
        feedforward_dimensions: request.act.feedforward_dim,
        encoder_layers: request.act.encoder_layers,
        vae_encoder_layers: request.act.vae_encoder_layers,
        decoder_layers: request.act.decoder_layers,
        kl_weight: request.act.kl_weight,
        learning_rate: request.learning_rate,
        total_steps: request.steps,
        seed: request.seed,
    };
    let mut trainer = training_backend
        .create_session(TrainingSessionRequest {
            model: TrainingModelConfig::Act(config),
            device: request.device.clone(),
            initialization: if request.act.pretrained_backbone {
                TrainingInitialization::PretrainedBackbone
            } else {
                TrainingInitialization::Random
            },
        })
        .map_err(|error| error.to_string())?;
    let trainer_info = trainer.info().clone();
    let _ = events.send(BackendEvent::Started(format!(
        "Vision ACT is running on {}/{}: {}; {:.1}M parameters.",
        trainer_info.backend.display_name,
        trainer_info.device,
        loader.summary(),
        trainer_info.parameter_count as f64 / 1_000_000.0
    )));
    let _ = events.send(BackendEvent::Log(format!(
        "ACT inputs: state={}, action={}, image={}x{}, chunk={}, cameras={}. Spiral owns the normal decode path; Studio recovers unusable managed indexes from the original MP4 with equivalent resize. Images cross the runtime boundary as HWC RGB bytes and are converted and ImageNet-normalized on the selected device.",
        loader.state_dimensions(),
        loader.action_dimensions(),
        loader.image_width(),
        loader.image_height(),
        request.chunk_size,
        resolved_request.act.camera_streams.join(", ")
    )));
    let _ = events.send(BackendEvent::Log(
        "ACT input pipelining is active: Spiral prepares one batch ahead and Studio fills one bounded remote Arrow buffer while the current optimizer step runs."
            .to_owned(),
    ));

    let data_contract = ActDataContract::from_loader(&loader);
    let has_validation = loader.has_validation();
    let validation_every = request.checkpoint_every.div_ceil(4).clamp(1, 250);
    if has_validation {
        let _ = events.send(BackendEvent::Log(format!(
            "Episode-level validation runs every {validation_every} optimizer step(s); normalization statistics use training episodes only."
        )));
    }
    std::thread::scope(|scope| {
        let (prefetch_tx, prefetch_rx) = std_mpsc::sync_channel(PREFETCH_BUFFER_SIZE);
        let prefetch_cancel = cancel.clone();
        let batch_size = request.batch_size;
        let steps = request.steps;
        let prefetch_worker = scope.spawn(move || {
            prefetch_batches(
                |validation| loader.fetch(batch_size, validation),
                steps,
                validation_every,
                has_validation,
                &prefetch_cancel,
                &prefetch_tx,
            );
        });

        let result = train_prefetched(
            prefetch_rx,
            run_id,
            request,
            &resolved_request,
            clock.as_ref(),
            cancel.as_ref(),
            &events,
            &data_contract,
            &trainer_info,
            trainer.as_mut(),
            validation_every,
            has_validation,
        );
        let worker_result = prefetch_worker
            .join()
            .map_err(|_| "ACT input prefetch worker panicked".to_owned());
        match (result, worker_result) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(outcome), Ok(())) => Ok(outcome),
        }
    })
}

fn prefetch_batches<F>(
    mut fetch: F,
    steps: u64,
    validation_every: u64,
    has_validation: bool,
    cancel: &AtomicBool,
    sender: &std_mpsc::SyncSender<PrefetchMessage>,
) where
    F: FnMut(bool) -> Result<LoadedActBatch, String>,
{
    for step in 1..=steps {
        if cancel.load(Ordering::Acquire) {
            let _ = sender.send(PrefetchMessage::Stopped);
            return;
        }
        if !send_prefetched_batch(sender, step, PrefetchedBatchKind::Training, fetch(false)) {
            return;
        }
        if has_validation
            && step % validation_every == 0
            && !send_prefetched_batch(sender, step, PrefetchedBatchKind::Validation, fetch(true))
        {
            return;
        }
    }
}

fn send_prefetched_batch(
    sender: &std_mpsc::SyncSender<PrefetchMessage>,
    step: u64,
    kind: PrefetchedBatchKind,
    result: Result<LoadedActBatch, String>,
) -> bool {
    let failed = result.is_err();
    sender
        .send(PrefetchMessage::Batch { step, kind, result })
        .is_ok()
        && !failed
}

fn receive_prefetched_batch(
    receiver: &std_mpsc::Receiver<PrefetchMessage>,
    expected_step: u64,
    expected_kind: PrefetchedBatchKind,
) -> Result<Option<LoadedActBatch>, String> {
    match receiver.recv() {
        Ok(PrefetchMessage::Batch { step, kind, result })
            if step == expected_step && kind == expected_kind =>
        {
            result.map(Some)
        }
        Ok(PrefetchMessage::Batch { step, kind, .. }) => Err(format!(
            "ACT input prefetch order mismatch: expected {expected_kind:?} batch for step {expected_step}, received {kind:?} batch for step {step}"
        )),
        Ok(PrefetchMessage::Stopped) => Ok(None),
        Err(_) => Err("ACT input prefetch worker stopped unexpectedly".to_owned()),
    }
}

struct BufferedTrainingStep {
    step: u64,
    loader_metrics: LoaderMetrics,
    loader_wait: Duration,
}

struct RunningTrainingStep {
    buffered: BufferedTrainingStep,
    pending: Box<dyn PendingTrainingStep>,
}

fn buffer_prefetched_training_step(
    receiver: &std_mpsc::Receiver<PrefetchMessage>,
    trainer: &mut dyn TrainingSession,
    step: u64,
) -> Result<Option<BufferedTrainingStep>, String> {
    let loader_wait_started = Instant::now();
    let Some(loaded) = receive_prefetched_batch(receiver, step, PrefetchedBatchKind::Training)?
    else {
        return Ok(None);
    };
    let loader_wait = loader_wait_started.elapsed();
    trainer
        .buffer_training_batch(loaded.batch, step)
        .map_err(|error| error.to_string())?;
    Ok(Some(BufferedTrainingStep {
        step,
        loader_metrics: loaded.metrics,
        loader_wait,
    }))
}

fn begin_buffered_training_step(
    trainer: &mut dyn TrainingSession,
    buffered: BufferedTrainingStep,
) -> Result<RunningTrainingStep, String> {
    let pending = trainer
        .begin_training_step(buffered.step)
        .map_err(|error| error.to_string())?;
    Ok(RunningTrainingStep { buffered, pending })
}

#[allow(clippy::too_many_arguments)]
fn train_prefetched(
    receiver: std_mpsc::Receiver<PrefetchMessage>,
    run_id: &str,
    request: &StartTrainingRequest,
    resolved_request: &StartTrainingRequest,
    clock: &dyn Clock,
    cancel: &AtomicBool,
    events: &mpsc::UnboundedSender<BackendEvent>,
    data_contract: &ActDataContract,
    trainer_info: &TrainingSessionInfo,
    trainer: &mut dyn TrainingSession,
    validation_every: u64,
    has_validation: bool,
) -> Result<TrainingRuntimeOutcome, String> {
    let started = Instant::now();
    let mut metric_started = started;
    let mut examples_since_metric = 0_usize;
    let mut last_validation = None;
    let mut last_checkpoint = 0;
    let mut completed_steps: u64;
    if cancel.load(Ordering::Acquire) {
        return Ok(TrainingRuntimeOutcome {
            steps: 0,
            stopped: true,
        });
    }
    let Some(first) = buffer_prefetched_training_step(&receiver, trainer, 1)? else {
        return Ok(TrainingRuntimeOutcome {
            steps: 0,
            stopped: cancel.load(Ordering::Acquire),
        });
    };
    let mut running = begin_buffered_training_step(trainer, first)?;
    loop {
        let step = running.buffered.step;
        let validation_due = has_validation && step % validation_every == 0;
        let next_buffer_result =
            if step < request.steps && !validation_due && !cancel.load(Ordering::Acquire) {
                buffer_prefetched_training_step(&receiver, trainer, step + 1)
            } else {
                Ok(None)
            };
        let output = running
            .pending
            .finish()
            .map_err(|error| error.to_string())?;
        let loader_metrics = running.buffered.loader_metrics;
        let loader_wait = running.buffered.loader_wait;
        let usage = output.telemetry;
        completed_steps = step;
        examples_since_metric += request.batch_size;
        let mut event = Vec::new();
        let validation_loss = if validation_due {
            let batch = receive_prefetched_batch(&receiver, step, PrefetchedBatchKind::Validation)?
                .ok_or_else(|| {
                    format!("ACT input prefetch stopped before validation at step {step}")
                })?;
            let validation = trainer
                .validation_step(batch.batch, step)
                .map_err(|error| error.to_string())?;
            last_validation = Some(validation.loss.total);
            event.push("validation");
            last_validation
        } else {
            None
        };
        if loader_wait >= LOADER_STALL {
            event.push("loader stall");
        }
        let now = Instant::now();
        let elapsed = now.duration_since(metric_started).as_secs_f64().max(1.0e-9);
        let _ = events.send(BackendEvent::Metric(TrainingMetricPoint {
            step,
            train_loss: output.loss.total,
            validation_loss,
            learning_rate: output.learning_rate,
            transfer_bytes_per_second: loader_metrics.source_bytes as f64
                / loader_metrics.seconds.max(1.0e-9),
            examples_per_second: examples_since_metric as f64 / elapsed,
            cpu_percent: usage.cpu_percent,
            gpu_percent: usage.gpu_percent,
            memory_bytes: usage.memory_bytes,
            loader_ms: Some(loader_metrics.seconds * 1_000.0),
            arrow_encode_ms: usage.arrow_encode_ms,
            upload_ms: usage.upload_ms,
            device_copy_ms: usage.device_copy_ms,
            model_ms: usage.model_ms,
            elapsed_seconds: now.duration_since(started).as_secs_f64(),
            event: (!event.is_empty()).then(|| event.join(" · ")),
        }));
        if step == 1 {
            let _ = events.send(BackendEvent::Log(format!(
                "Loss components at step 1: L1={:.4}, KL={:.4}, pad={:.4}; decoded {} frame(s).",
                output.loss.l1, output.loss.kl, output.loss.pad, loader_metrics.requested_frames
            )));
        }
        metric_started = now;
        examples_since_metric = 0;
        if step % request.checkpoint_every == 0 {
            send_checkpoint(
                events,
                run_id,
                resolved_request,
                data_contract,
                trainer_info,
                trainer,
                step,
                last_validation,
                clock,
            )?;
            last_checkpoint = step;
        }
        let next_buffered = next_buffer_result?;
        if step >= request.steps || cancel.load(Ordering::Acquire) {
            break;
        }
        let next_buffered = match next_buffered {
            Some(buffered) => buffered,
            None => {
                let Some(buffered) = buffer_prefetched_training_step(&receiver, trainer, step + 1)?
                else {
                    break;
                };
                buffered
            }
        };
        running = begin_buffered_training_step(trainer, next_buffered)?;
    }
    if completed_steps > 0 && last_checkpoint != completed_steps {
        send_checkpoint(
            events,
            run_id,
            resolved_request,
            data_contract,
            trainer_info,
            trainer,
            completed_steps,
            last_validation,
            clock,
        )?;
    }
    Ok(TrainingRuntimeOutcome {
        steps: completed_steps,
        stopped: cancel.load(Ordering::Acquire),
    })
}

#[allow(clippy::too_many_arguments)]
fn send_checkpoint(
    events: &mpsc::UnboundedSender<BackendEvent>,
    run_id: &str,
    request: &StartTrainingRequest,
    data_contract: &ActDataContract,
    trainer_info: &TrainingSessionInfo,
    trainer: &dyn TrainingSession,
    step: u64,
    validation_loss: Option<f64>,
    clock: &dyn Clock,
) -> Result<(), String> {
    let manifest = checkpoint_manifest(run_id, request, data_contract, trainer_info, step);
    let manifest_json = serde_json::to_string(&manifest)
        .map_err(|error| format!("encoding ACT checkpoint manifest: {error}"))?;
    let artifact = trainer
        .checkpoint(TrainingCheckpointRequest {
            manifest_json,
            state_mean: data_contract.stats.state_mean.clone(),
            state_std: data_contract.stats.state_std.clone(),
            action_mean: data_contract.stats.action_mean.clone(),
            action_std: data_contract.stats.action_std.clone(),
        })
        .map_err(|error| error.to_string())?;
    if artifact.format != "safetensors" || artifact.extension != "safetensors" {
        return Err(format!(
            "training backend returned unsupported checkpoint format {:?} with extension {:?}",
            artifact.format, artifact.extension
        ));
    }
    let (acknowledged, result) = std::sync::mpsc::sync_channel(1);
    events
        .send(BackendEvent::Checkpoint(BackendCheckpoint {
            step,
            created_ms: clock.now_ms(),
            validation_loss,
            manifest,
            payload: artifact.payload,
            acknowledged,
        }))
        .map_err(|_| "training event channel closed before checkpoint persistence".to_owned())?;
    result
        .recv()
        .map_err(|_| "checkpoint writer stopped before acknowledging the artifact".to_owned())?
}

fn checkpoint_manifest(
    run_id: &str,
    request: &StartTrainingRequest,
    data_contract: &ActDataContract,
    trainer_info: &TrainingSessionInfo,
    step: u64,
) -> serde_json::Value {
    let mut input_features = serde_json::Map::new();
    input_features.insert(
        "observation.state".to_owned(),
        serde_json::json!({"type": "STATE", "shape": [data_contract.state_dimensions]}),
    );
    for camera in &request.act.camera_streams {
        input_features.insert(
            camera.clone(),
            serde_json::json!({
                "type": "VISUAL",
                "shape": [3, data_contract.image_height, data_contract.image_width]
            }),
        );
    }
    let model_config = serde_json::json!({
        "type": "act",
        "n_obs_steps": 1,
        "normalization_mapping": {
            "VISUAL": "MEAN_STD",
            "STATE": "MEAN_STD",
            "ACTION": "MEAN_STD"
        },
        "input_features": input_features,
        "output_features": {
            "action": {"type": "ACTION", "shape": [data_contract.action_dimensions]}
        },
        "chunk_size": request.chunk_size,
        "n_action_steps": request.chunk_size.min(MAX_ROLLOUT_ACTION_STEPS),
        "vision_backbone": "resnet18",
        "replace_final_stride_with_dilation": false,
        "pre_norm": false,
        "dim_model": request.act.hidden_dim,
        "n_heads": request.act.attention_heads,
        "dim_feedforward": request.act.feedforward_dim,
        "feedforward_activation": "relu",
        "n_encoder_layers": request.act.encoder_layers,
        "n_vae_encoder_layers": request.act.vae_encoder_layers,
        "n_decoder_layers": request.act.decoder_layers,
        "use_vae": true,
        "latent_dim": request.act.latent_dim,
        "spiral_camera_embedding": false,
        "spiral_camera_order": request.act.camera_streams,
    });
    serde_json::json!({
        "format": ACT_ARTIFACT_FORMAT,
        "artifact": "inference",
        "run_id": run_id,
        "step": step,
        "request": request,
        "data_contract": {
            "value_space": "centered_normalized",
            "robot_type": data_contract.robot_type,
            "joint_names": data_contract.joint_names,
            "camera_streams": request.act.camera_streams,
            "sample_hz": data_contract.sample_hz,
            "image_layout": "chw",
            "image_normalization": "imagenet",
            "frame_alignment": "fixed_fps",
            "spiral_schema_version": spiral::SCHEMA_VERSION,
        },
        "runtime": {
            "training_backend": {
                "api_version": trainer_info.backend.api_version,
                "id": trainer_info.backend.id,
                "runtime": trainer_info.backend.runtime,
                "device": trainer_info.device,
            },
            "training_image_transport": {
                "dtype": "uint8",
                "layout": "hwc",
                "normalization": "runtime_imagenet",
            },
            "transport": "adbc 0.24 / arrow 58.4",
            "language": "python",
        },
        "normalization": "embedded_mean_std",
        "model_config": model_config,
        "tensor_namespace": ACT_TENSOR_NAMESPACE,
    })
}

fn checkpoint_sample_hz(manifest: &serde_json::Value) -> Result<f64, String> {
    manifest
        .pointer("/data_contract/sample_hz")
        .and_then(serde_json::Value::as_f64)
        .filter(|sample_hz| sample_hz.is_finite() && *sample_hz > 0.0)
        .ok_or_else(|| "checkpoint manifest has no finite positive data sample rate".to_owned())
}

async fn readiness(
    storage: &StorageManager,
    training_backend: &Arc<dyn TrainingBackend>,
) -> TrainingReadiness {
    let backend_probe = probe_training_backend(training_backend);
    let spiral_probe = async {
        tokio::time::timeout(Duration::from_secs(8), storage.test_spiral())
            .await
            .unwrap_or_else(|_| ConnectionTestResponse {
                ok: false,
                message: "the connection probe timed out after 8 seconds".to_owned(),
            })
    };
    let (backend, connection) = tokio::join!(backend_probe, spiral_probe);
    let runtime = backend.as_ref().ok().map(|info| TrainingRuntimeInfo {
        id: info.id.clone(),
        display_name: info.display_name.clone(),
        runtime: info.runtime.clone(),
        devices: info.devices.clone(),
    });
    let spiral_ready = connection.ok;
    let ready = spiral_ready && backend.is_ok();
    let summary = match (connection, backend) {
        (result, _) if !result.ok => {
            format!("Spiral is not ready for training: {}", result.message)
        }
        (_, Err(error)) => format!("Training runtime is not ready: {error}"),
        (_, Ok(backend)) => format!(
            "Runtime ready · {probe} · Spiral project {} · namespace {} · query checked at run start",
            storage.project_id(),
            storage.namespace(),
            probe = backend.summary(),
        ),
    };
    TrainingReadiness {
        ready,
        spiral_ready,
        runtime,
        summary,
    }
}

async fn probe_training_backend(
    training_backend: &Arc<dyn TrainingBackend>,
) -> Result<spiral_robotics_training::TrainingBackendInfo, String> {
    let backend = training_backend.clone();
    tokio::time::timeout(
        Duration::from_secs(8),
        tokio::task::spawn_blocking(move || backend.health()),
    )
    .await
    .map_err(|_| "training backend health probe timed out after 8 seconds".to_owned())
    .and_then(|result| {
        result.map_err(|error| format!("training backend health task failed: {error}"))
    })
    .and_then(|result| result.map_err(|error| error.to_string()))
    .and_then(|info| {
        if info.api_version == TRAINING_BACKEND_API_VERSION {
            Ok(info)
        } else {
            Err(format!(
                "training backend API version {} is incompatible with Studio version {}",
                info.api_version, TRAINING_BACKEND_API_VERSION
            ))
        }
    })
}

fn validate_checkpoint_artifact(
    file_name: &str,
    payload: &[u8],
    manifest: &serde_json::Value,
    run_id: &str,
    step: u64,
    request: &StartTrainingRequest,
) -> Result<String, String> {
    if Path::new(file_name).extension() != Some(std::ffi::OsStr::new("safetensors")) {
        return Err("the training backend must publish a .safetensors checkpoint".to_owned());
    }
    let tensors = safetensors::SafeTensors::deserialize(payload)
        .map_err(|error| format!("checkpoint {file_name} is not valid SafeTensors: {error}"))?;
    if tensors.is_empty() {
        return Err(format!("checkpoint {file_name} contains no tensors"));
    }
    let (_, metadata) = safetensors::SafeTensors::read_metadata(payload)
        .map_err(|error| format!("checkpoint {file_name} metadata is invalid: {error}"))?;
    let metadata = metadata
        .metadata()
        .as_ref()
        .ok_or_else(|| format!("checkpoint {file_name} has no artifact metadata"))?;
    if metadata.get("format").map(String::as_str) != Some("spiral-act") {
        return Err(format!(
            "checkpoint {file_name} does not declare its tensor framework"
        ));
    }
    let embedded_manifest = metadata
        .get("spiral_robotics")
        .ok_or_else(|| format!("checkpoint {file_name} has no Spiral manifest"))?;
    let embedded_manifest: serde_json::Value =
        serde_json::from_str(embedded_manifest).map_err(|error| {
            format!("checkpoint {file_name} has an invalid Spiral manifest: {error}")
        })?;
    if &embedded_manifest != manifest {
        return Err(format!(
            "checkpoint {file_name} metadata does not match the trainer event"
        ));
    }
    validate_artifact_identity(
        &ArtifactIdentity {
            format: manifest
                .get("format")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            purpose: manifest
                .get("artifact")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            run_id: manifest
                .get("run_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            step: manifest
                .get("step")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            tensor_namespace: manifest
                .get("tensor_namespace")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        run_id,
        step,
    )
    .map_err(|error| format!("checkpoint {file_name}: {error}"))?;
    let request_json = serde_json::to_value(request)
        .map_err(|error| format!("could not validate checkpoint configuration: {error}"))?;
    if manifest.get("request") != Some(&request_json) {
        return Err(format!(
            "checkpoint {file_name} configuration does not match the active run"
        ));
    }
    let model_config = manifest
        .get("model_config")
        .ok_or_else(|| format!("checkpoint {file_name} has no native model configuration"))?;
    serde_json::to_string(model_config)
        .map_err(|error| format!("could not encode native model configuration: {error}"))
}

fn application_training_command(request: StartTrainingRequest) -> ApplicationStartTrainingCommand {
    ApplicationStartTrainingCommand {
        run_name: request.run_name,
        dataset_id: request.dataset_id,
        policy: match request.policy {
            spiral_robotics_contracts::api::TrainingPolicy::Act => ApplicationTrainingPolicy::Act,
        },
        query: request.sql,
        state_column: request.state_column,
        action_column: request.action_column,
        device: request.device,
        steps: request.steps,
        batch_size: request.batch_size,
        validation_split: request.validation_split,
        learning_rate: request.learning_rate,
        checkpoint_every: request.checkpoint_every,
        chunk_size: request.chunk_size,
        seed: request.seed,
        act: ApplicationActTrainingSettings {
            camera_streams: request.act.camera_streams,
            image_width: request.act.image_width,
            image_height: request.act.image_height,
            video_decoder: request.act.video_decoder,
            pretrained_backbone: request.act.pretrained_backbone,
            hidden_dim: request.act.hidden_dim,
            latent_dim: request.act.latent_dim,
            encoder_layers: request.act.encoder_layers,
            vae_encoder_layers: request.act.vae_encoder_layers,
            decoder_layers: request.act.decoder_layers,
            attention_heads: request.act.attention_heads,
            feedforward_dim: request.act.feedforward_dim,
            kl_weight: request.act.kl_weight,
        },
    }
}

fn contract_training_request(command: ApplicationStartTrainingCommand) -> StartTrainingRequest {
    StartTrainingRequest {
        run_name: command.run_name,
        dataset_id: command.dataset_id,
        policy: match command.policy {
            ApplicationTrainingPolicy::Act => spiral_robotics_contracts::api::TrainingPolicy::Act,
        },
        sql: command.query,
        state_column: command.state_column,
        action_column: command.action_column,
        device: command.device,
        steps: command.steps,
        batch_size: command.batch_size,
        validation_split: command.validation_split,
        learning_rate: command.learning_rate,
        checkpoint_every: command.checkpoint_every,
        chunk_size: command.chunk_size,
        seed: command.seed,
        act: spiral_robotics_contracts::api::ActTrainingConfig {
            camera_streams: command.act.camera_streams,
            image_width: command.act.image_width,
            image_height: command.act.image_height,
            video_decoder: command.act.video_decoder,
            pretrained_backbone: command.act.pretrained_backbone,
            hidden_dim: command.act.hidden_dim,
            latent_dim: command.act.latent_dim,
            encoder_layers: command.act.encoder_layers,
            vae_encoder_layers: command.act.vae_encoder_layers,
            decoder_layers: command.act.decoder_layers,
            attention_heads: command.act.attention_heads,
            feedforward_dim: command.act.feedforward_dim,
            kl_weight: command.act.kl_weight,
        },
    }
}

fn contract_training_source(source: ApplicationTrainingSource) -> TrainingSource {
    TrainingSource {
        dataset_id: source.dataset_id,
    }
}

fn application_training_phase(phase: TrainingRunPhase) -> ApplicationTrainingRunPhase {
    match phase {
        TrainingRunPhase::Starting => ApplicationTrainingRunPhase::Starting,
        TrainingRunPhase::Running => ApplicationTrainingRunPhase::Running,
        TrainingRunPhase::Stopping => ApplicationTrainingRunPhase::Stopping,
        TrainingRunPhase::Completed => ApplicationTrainingRunPhase::Completed,
        TrainingRunPhase::Stopped => ApplicationTrainingRunPhase::Stopped,
        TrainingRunPhase::Failed => ApplicationTrainingRunPhase::Failed,
    }
}

fn contract_training_phase(phase: ApplicationTrainingRunPhase) -> TrainingRunPhase {
    match phase {
        ApplicationTrainingRunPhase::Starting => TrainingRunPhase::Starting,
        ApplicationTrainingRunPhase::Running => TrainingRunPhase::Running,
        ApplicationTrainingRunPhase::Stopping => TrainingRunPhase::Stopping,
        ApplicationTrainingRunPhase::Completed => TrainingRunPhase::Completed,
        ApplicationTrainingRunPhase::Stopped => TrainingRunPhase::Stopped,
        ApplicationTrainingRunPhase::Failed => TrainingRunPhase::Failed,
    }
}

fn application_training_readiness(readiness: TrainingReadiness) -> ApplicationTrainingReadiness {
    ApplicationTrainingReadiness {
        ready: readiness.ready,
        spiral_ready: readiness.spiral_ready,
        runtime: readiness.runtime.map(|runtime| {
            crate::backend::application::training::TrainingRuntimeInfo {
                id: runtime.id,
                display_name: runtime.display_name,
                runtime: runtime.runtime,
                devices: runtime.devices,
            }
        }),
        summary: readiness.summary,
    }
}

fn application_training_metric(metric: TrainingMetricPoint) -> ApplicationTrainingMetric {
    ApplicationTrainingMetric {
        step: metric.step,
        train_loss: metric.train_loss,
        validation_loss: metric.validation_loss,
        learning_rate: metric.learning_rate,
        transfer_bytes_per_second: metric.transfer_bytes_per_second,
        examples_per_second: metric.examples_per_second,
        cpu_percent: metric.cpu_percent,
        gpu_percent: metric.gpu_percent,
        memory_bytes: metric.memory_bytes,
        loader_ms: metric.loader_ms,
        batch_encode_ms: metric.arrow_encode_ms,
        upload_ms: metric.upload_ms,
        device_copy_ms: metric.device_copy_ms,
        model_ms: metric.model_ms,
        elapsed_seconds: metric.elapsed_seconds,
        event: metric.event,
    }
}

fn contract_training_metric(metric: ApplicationTrainingMetric) -> TrainingMetricPoint {
    TrainingMetricPoint {
        step: metric.step,
        train_loss: metric.train_loss,
        validation_loss: metric.validation_loss,
        learning_rate: metric.learning_rate,
        transfer_bytes_per_second: metric.transfer_bytes_per_second,
        examples_per_second: metric.examples_per_second,
        cpu_percent: metric.cpu_percent,
        gpu_percent: metric.gpu_percent,
        memory_bytes: metric.memory_bytes,
        loader_ms: metric.loader_ms,
        arrow_encode_ms: metric.batch_encode_ms,
        upload_ms: metric.upload_ms,
        device_copy_ms: metric.device_copy_ms,
        model_ms: metric.model_ms,
        elapsed_seconds: metric.elapsed_seconds,
        event: metric.event,
    }
}

fn contract_training_checkpoint(checkpoint: ApplicationTrainingCheckpoint) -> TrainingCheckpoint {
    TrainingCheckpoint {
        step: checkpoint.step,
        created_ms: checkpoint.created_ms,
        validation_loss: checkpoint.validation_loss,
        byte_size: checkpoint.byte_size,
        inference_ready: checkpoint.inference_ready,
        recommended_action_steps: checkpoint.recommended_action_steps,
        sample_hz: checkpoint.sample_hz,
    }
}

fn contract_training_run(run: ApplicationTrainingRun) -> TrainingRun {
    TrainingRun {
        id: run.id,
        config: contract_training_request(run.config),
        source: contract_training_source(run.source),
        phase: contract_training_phase(run.phase),
        current_step: run.current_step,
        started_ms: run.started_ms,
        finished_ms: run.finished_ms,
        message: run.message,
        metrics: run
            .metrics
            .into_iter()
            .map(contract_training_metric)
            .collect(),
        checkpoints: run
            .checkpoints
            .into_iter()
            .map(contract_training_checkpoint)
            .collect(),
        logs: run.logs,
    }
}

fn application_runtime_event(event: BackendEvent) -> TrainingRuntimeEvent<BackendCheckpoint> {
    match event {
        BackendEvent::Configured(command) => {
            TrainingRuntimeEvent::Configured(Box::new(application_training_command(*command)))
        }
        BackendEvent::Started(message) => TrainingRuntimeEvent::Started(message),
        BackendEvent::Metric(metric) => {
            TrainingRuntimeEvent::Metric(application_training_metric(metric))
        }
        BackendEvent::Checkpoint(checkpoint) => TrainingRuntimeEvent::Checkpoint(checkpoint),
        BackendEvent::Log(message) => TrainingRuntimeEvent::Log(message),
    }
}

fn application_phase(phase: TrainingRunPhase) -> ApplicationRunPhase {
    match phase {
        TrainingRunPhase::Starting => ApplicationRunPhase::Starting,
        TrainingRunPhase::Running => ApplicationRunPhase::Running,
        TrainingRunPhase::Stopping => ApplicationRunPhase::Stopping,
        TrainingRunPhase::Completed => ApplicationRunPhase::Completed,
        TrainingRunPhase::Stopped => ApplicationRunPhase::Stopped,
        TrainingRunPhase::Failed => ApplicationRunPhase::Failed,
    }
}

fn registry(snapshot: &TrainingSnapshot) -> Result<TrainingRunRegistry, String> {
    TrainingRunRegistry::new(
        MAX_TRAINING_HISTORY_RUNS,
        snapshot.runs.iter().map(|run| RunRecord {
            id: run.id.clone(),
            phase: application_phase(run.phase),
        }),
    )
}

fn admitted_snapshot(
    storage: &StorageManager,
    snapshot: &TrainingSnapshot,
    run: TrainingRun,
) -> Result<(TrainingSnapshot, TrainingUpdate), String> {
    let mut registry = registry(snapshot)?;
    registry.admit(RunRecord {
        id: run.id.clone(),
        phase: application_phase(run.phase),
    })?;
    let retained = registry
        .retained_ids()
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let update = TrainingUpdate::RunAdded { run: Box::new(run) };
    let mut candidate = snapshot.clone();
    candidate.apply(update.clone());
    candidate.runs.retain(|run| retained.contains(&run.id));
    storage.save_training_snapshot(&candidate)?;
    Ok((candidate, update))
}

fn persistence_failure_updates(
    snapshot: &TrainingSnapshot,
    run_id: &str,
    finished_ms: i64,
    error: &str,
) -> Result<(TrainingUpdate, TrainingUpdate), String> {
    let run = snapshot
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .ok_or_else(|| format!("training run {run_id:?} is not in history"))?;
    let message =
        format!("Training history could not be persisted, so execution was stopped: {error}");
    Ok((
        TrainingUpdate::Log {
            run_id: run_id.to_owned(),
            line: message.clone(),
        },
        TrainingUpdate::RunState {
            run_id: run_id.to_owned(),
            phase: TrainingRunPhase::Failed,
            current_step: run.current_step,
            finished_ms: Some(finished_ms),
            message: Some(message),
        },
    ))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_training::{
        TrainingBackendError, TrainingBatch, TrainingCheckpointArtifact, TrainingStepOutput,
        TrainingTelemetry,
    };

    fn loaded_batch(marker: f32) -> LoadedActBatch {
        LoadedActBatch {
            batch: TrainingBatch {
                batch_size: 1,
                states: vec![marker].into(),
                images: vec![marker as u8].into(),
                actions: vec![marker].into(),
                action_is_pad: vec![0].into(),
            },
            metrics: crate::backend::training::data::LoaderMetrics {
                seconds: f64::from(marker),
                source_bytes: 4,
                requested_frames: 1,
            },
        }
    }

    fn session_info() -> TrainingSessionInfo {
        TrainingSessionInfo {
            backend: spiral_robotics_training::TrainingBackendInfo {
                api_version: TRAINING_BACKEND_API_VERSION,
                id: "remote.example".to_owned(),
                display_name: "Remote example".to_owned(),
                runtime: "HTTP".to_owned(),
                devices: vec!["remote-gpu".to_owned()],
            },
            device: "remote-gpu".to_owned(),
            parameter_count: 42,
        }
    }

    struct PipelinePendingStep {
        step: u64,
        next_step_buffered: Arc<AtomicBool>,
    }

    impl PendingTrainingStep for PipelinePendingStep {
        fn finish(self: Box<Self>) -> Result<TrainingStepOutput, TrainingBackendError> {
            if self.step == 1 && !self.next_step_buffered.load(Ordering::Acquire) {
                return Err(TrainingBackendError::new(
                    "step 1 finished before step 2 reached the upload buffer",
                ));
            }
            Ok(TrainingStepOutput {
                loss: spiral_robotics_training::TrainingLoss {
                    total: self.step as f64,
                    l1: self.step as f64,
                    kl: 0.0,
                    pad: 0.0,
                },
                learning_rate: 1.0e-5,
                telemetry: TrainingTelemetry {
                    cpu_percent: None,
                    gpu_percent: None,
                    memory_bytes: None,
                    arrow_encode_ms: Some(1.0),
                    upload_ms: Some(2.0),
                    device_copy_ms: Some(3.0),
                    model_ms: Some(4.0),
                },
            })
        }
    }

    struct PipelineSession {
        info: TrainingSessionInfo,
        buffered_step: Option<u64>,
        next_step_buffered: Arc<AtomicBool>,
    }

    impl PipelineSession {
        fn new() -> Self {
            Self {
                info: session_info(),
                buffered_step: None,
                next_step_buffered: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl TrainingSession for PipelineSession {
        fn info(&self) -> &TrainingSessionInfo {
            &self.info
        }

        fn buffer_training_batch(
            &mut self,
            _batch: TrainingBatch<'static>,
            step: u64,
        ) -> Result<(), TrainingBackendError> {
            if self.buffered_step.replace(step).is_some() {
                return Err(TrainingBackendError::new(
                    "test buffer was already occupied",
                ));
            }
            if step == 2 {
                self.next_step_buffered.store(true, Ordering::Release);
            }
            Ok(())
        }

        fn begin_training_step(
            &mut self,
            step: u64,
        ) -> Result<Box<dyn PendingTrainingStep>, TrainingBackendError> {
            if self.buffered_step.take() != Some(step) {
                return Err(TrainingBackendError::new("test buffer step mismatch"));
            }
            Ok(Box::new(PipelinePendingStep {
                step,
                next_step_buffered: self.next_step_buffered.clone(),
            }))
        }

        fn validation_step(
            &mut self,
            _batch: TrainingBatch<'static>,
            _after_step: u64,
        ) -> Result<TrainingStepOutput, TrainingBackendError> {
            Err(TrainingBackendError::new(
                "validation was not expected in the pipeline test",
            ))
        }

        fn checkpoint(
            &self,
            _request: TrainingCheckpointRequest,
        ) -> Result<TrainingCheckpointArtifact, TrainingBackendError> {
            Ok(TrainingCheckpointArtifact {
                format: "safetensors".to_owned(),
                extension: "safetensors".to_owned(),
                payload: vec![1],
            })
        }
    }

    #[test]
    fn remote_upload_buffer_is_filled_before_the_current_step_finishes() {
        let (sender, receiver) = std_mpsc::sync_channel(2);
        for step in 1..=2 {
            sender
                .send(PrefetchMessage::Batch {
                    step,
                    kind: PrefetchedBatchKind::Training,
                    result: Ok(loaded_batch(step as f32)),
                })
                .unwrap();
        }
        drop(sender);
        let request = StartTrainingRequest {
            steps: 2,
            batch_size: 1,
            checkpoint_every: 10,
            ..Default::default()
        };
        let data_contract = ActDataContract {
            state_dimensions: 1,
            action_dimensions: 1,
            image_width: 1,
            image_height: 1,
            stats: NormalizationStats::identity(1, 1),
            joint_names: vec!["joint".to_owned()],
            robot_type: "test".to_owned(),
            sample_hz: 30.0,
        };
        let (events, mut updates) = mpsc::unbounded_channel();
        let checkpoint_writer = std::thread::spawn(move || {
            while let Some(event) = updates.blocking_recv() {
                if let BackendEvent::Checkpoint(checkpoint) = event {
                    checkpoint.acknowledged.send(Ok(())).unwrap();
                    return;
                }
            }
        });
        let mut trainer = PipelineSession::new();

        let outcome = train_prefetched(
            receiver,
            "train-pipeline",
            &request,
            &request,
            &SystemClock,
            &AtomicBool::new(false),
            &events,
            &data_contract,
            &session_info(),
            &mut trainer,
            25,
            false,
        )
        .unwrap();

        checkpoint_writer.join().unwrap();
        assert_eq!(outcome.steps, 2);
        assert!(trainer.next_step_buffered.load(Ordering::Acquire));
    }

    #[test]
    fn checkpoint_predicts_the_full_horizon_but_recommends_twenty_actions() {
        let request = StartTrainingRequest {
            chunk_size: 100,
            act: spiral_robotics_contracts::api::ActTrainingConfig {
                camera_streams: vec!["observation.images.wrist".to_owned()],
                ..Default::default()
            },
            ..Default::default()
        };
        let data_contract = ActDataContract {
            state_dimensions: 2,
            action_dimensions: 2,
            image_width: 640,
            image_height: 480,
            stats: NormalizationStats::identity(2, 2),
            joint_names: vec!["shoulder".to_owned(), "elbow".to_owned()],
            robot_type: "so101_follower".to_owned(),
            sample_hz: 30.0,
        };

        let manifest =
            checkpoint_manifest("train-1", &request, &data_contract, &session_info(), 100);

        assert_eq!(manifest["model_config"]["chunk_size"], 100);
        assert_eq!(
            manifest["model_config"]["n_action_steps"],
            MAX_ROLLOUT_ACTION_STEPS
        );
        assert_eq!(
            manifest["runtime"]["training_backend"]["id"],
            "remote.example"
        );
    }

    fn run(id: &str) -> TrainingRun {
        TrainingRun {
            id: id.to_owned(),
            config: StartTrainingRequest::default(),
            source: TrainingSource::default(),
            phase: TrainingRunPhase::Starting,
            current_step: 0,
            started_ms: 1,
            finished_ms: None,
            message: None,
            metrics: Vec::new(),
            checkpoints: Vec::new(),
            logs: Vec::new(),
        }
    }

    #[test]
    fn runtime_persistence_failure_becomes_a_visible_failed_run_and_log() {
        let mut active = run("active");
        active.phase = TrainingRunPhase::Running;
        active.current_step = 12;
        let mut snapshot = TrainingSnapshot {
            runs: vec![active],
            ..TrainingSnapshot::default()
        };

        let (log, state) =
            persistence_failure_updates(&snapshot, "active", 99, "Spiral unavailable").unwrap();
        snapshot.apply(log);
        snapshot.apply(state);

        let failed = &snapshot.runs[0];
        assert_eq!(failed.phase, TrainingRunPhase::Failed);
        assert_eq!(failed.current_step, 12);
        assert_eq!(failed.finished_ms, Some(99));
        assert!(
            failed
                .message
                .as_deref()
                .unwrap()
                .contains("Spiral unavailable")
        );
        assert!(
            failed
                .logs
                .last()
                .unwrap()
                .contains("execution was stopped")
        );
    }

    #[test]
    fn checkpoint_sample_rate_never_defaults_invalid_metadata_to_zero() {
        assert_eq!(
            checkpoint_sample_hz(&serde_json::json!({
                "data_contract": {"sample_hz": 60.0}
            }))
            .unwrap(),
            60.0
        );
        for invalid in [
            serde_json::json!({}),
            serde_json::json!({"data_contract": {"sample_hz": 0.0}}),
            serde_json::json!({"data_contract": {"sample_hz": -1.0}}),
        ] {
            assert!(checkpoint_sample_hz(&invalid).is_err());
        }
    }

    #[test]
    fn input_prefetch_fetches_the_next_batch_while_training_consumes_the_current_one() {
        let cancel = AtomicBool::new(false);
        let (prefetch_tx, prefetch_rx) = std_mpsc::sync_channel(PREFETCH_BUFFER_SIZE);
        let (fetch_tx, fetch_rx) = std_mpsc::channel();

        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let mut marker = 0.0_f32;
                prefetch_batches(
                    |validation| {
                        assert!(!validation);
                        marker += 1.0;
                        fetch_tx.send(marker).unwrap();
                        Ok(loaded_batch(marker))
                    },
                    2,
                    25,
                    false,
                    &cancel,
                    &prefetch_tx,
                );
            });

            assert_eq!(fetch_rx.recv_timeout(Duration::from_secs(1)), Ok(1.0));
            let first = receive_prefetched_batch(&prefetch_rx, 1, PrefetchedBatchKind::Training)
                .unwrap()
                .unwrap();
            assert_eq!(first.batch.states, vec![1.0]);

            // The consumer deliberately has not requested step 2 yet. Seeing
            // this signal proves its load is already overlapping step 1 work.
            assert_eq!(fetch_rx.recv_timeout(Duration::from_secs(1)), Ok(2.0));
            let second = receive_prefetched_batch(&prefetch_rx, 2, PrefetchedBatchKind::Training)
                .unwrap()
                .unwrap();
            assert_eq!(second.batch.states, vec![2.0]);
            worker.join().unwrap();
        });
    }

    #[test]
    fn input_prefetch_preserves_training_and_validation_order() {
        let cancel = AtomicBool::new(false);
        let (prefetch_tx, prefetch_rx) = std_mpsc::sync_channel(4);
        let mut requests = Vec::new();
        prefetch_batches(
            |validation| {
                requests.push(validation);
                Ok(loaded_batch(requests.len() as f32))
            },
            2,
            2,
            true,
            &cancel,
            &prefetch_tx,
        );

        assert_eq!(requests, vec![false, false, true]);
        assert!(
            receive_prefetched_batch(&prefetch_rx, 1, PrefetchedBatchKind::Training)
                .unwrap()
                .is_some()
        );
        assert!(
            receive_prefetched_batch(&prefetch_rx, 2, PrefetchedBatchKind::Training)
                .unwrap()
                .is_some()
        );
        assert!(
            receive_prefetched_batch(&prefetch_rx, 2, PrefetchedBatchKind::Validation)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn input_prefetch_propagates_loader_errors_and_honors_cancellation() {
        let cancel = AtomicBool::new(false);
        let (prefetch_tx, prefetch_rx) = std_mpsc::sync_channel(3);
        let mut calls = 0;
        prefetch_batches(
            |_| {
                calls += 1;
                if calls == 2 {
                    Err("decode failed".to_owned())
                } else {
                    Ok(loaded_batch(calls as f32))
                }
            },
            3,
            25,
            false,
            &cancel,
            &prefetch_tx,
        );
        assert_eq!(calls, 2);
        assert!(
            receive_prefetched_batch(&prefetch_rx, 1, PrefetchedBatchKind::Training)
                .unwrap()
                .is_some()
        );
        match receive_prefetched_batch(&prefetch_rx, 2, PrefetchedBatchKind::Training) {
            Err(error) => assert_eq!(error, "decode failed"),
            Ok(_) => panic!("the decode failure must reach the trainer"),
        }

        let cancel = AtomicBool::new(true);
        let (prefetch_tx, prefetch_rx) = std_mpsc::sync_channel(1);
        let mut cancelled_calls = 0;
        prefetch_batches(
            |_| {
                cancelled_calls += 1;
                Ok(loaded_batch(1.0))
            },
            1,
            25,
            false,
            &cancel,
            &prefetch_tx,
        );
        assert_eq!(cancelled_calls, 0);
        assert!(
            receive_prefetched_batch(&prefetch_rx, 1, PrefetchedBatchKind::Training)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn checkpoint_persistence_accepts_only_matching_safetensors_artifacts() {
        use std::collections::HashMap;

        use safetensors::tensor::{Dtype, TensorView, serialize};

        let request = StartTrainingRequest::default();
        let manifest = serde_json::json!({
            "format": ACT_ARTIFACT_FORMAT,
            "artifact": "inference",
            "run_id": "train-1",
            "step": 10,
            "request": request,
            "model_config": {"type": "act"},
            "tensor_namespace": ACT_TENSOR_NAMESPACE,
        });
        let metadata = HashMap::from([
            ("format".to_owned(), "spiral-act".to_owned()),
            (
                "spiral_robotics".to_owned(),
                serde_json::to_string(&manifest).unwrap(),
            ),
        ]);
        let data = [0_u8; 4];
        let tensor = TensorView::new(Dtype::F32, vec![1], &data).unwrap();
        let payload = serialize([("weight", tensor)], Some(metadata)).unwrap();

        assert!(
            validate_checkpoint_artifact(
                "checkpoint-10.safetensors",
                &payload,
                &manifest,
                "train-1",
                10,
                &request,
            )
            .is_ok()
        );
        assert!(
            validate_checkpoint_artifact(
                "checkpoint-10.pt",
                &payload,
                &manifest,
                "train-1",
                10,
                &request,
            )
            .unwrap_err()
            .contains(".safetensors")
        );
        assert!(
            validate_checkpoint_artifact(
                "checkpoint-10.safetensors",
                b"pickle",
                &manifest,
                "train-1",
                10,
                &request,
            )
            .unwrap_err()
            .contains("not valid SafeTensors")
        );
    }
}
