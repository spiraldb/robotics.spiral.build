//! Rust-owned supervision for Spiral ADBC and an injected training backend.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use spiral_robotics_contracts::api::{
    ConnectionTestResponse, MAX_TRAINING_HISTORY_RUNS, StartTrainingRequest, TrainingCheckpoint,
    TrainingMetricPoint, TrainingReadiness, TrainingRun, TrainingRunPhase, TrainingRuntimeInfo,
    TrainingSnapshot, TrainingSource, TrainingUpdate,
};
use spiral_robotics_training::{
    ActTrainingJobConfig, SpiralJobConnection, TrainingBackend, TrainingInitialization,
    TrainingJobCursor, TrainingJobPhase, TrainingJobRequest, TrainingJobSnapshot,
};
use tokio::sync::broadcast;

use crate::backend::adapters::clock::SystemClock;
use crate::backend::storage::manager::StorageManager;
use spiral_robotics_studio_application::checkpoints::CheckpointRepository;
use spiral_robotics_studio_application::clock::{Clock, StableIdSequence};
use spiral_robotics_studio_application::error::ApplicationError;
use spiral_robotics_studio_application::training::{
    ActTrainingSettings as ApplicationActTrainingSettings,
    StartTrainingCommand as ApplicationStartTrainingCommand,
    TrainingCheckpoint as ApplicationTrainingCheckpoint,
    TrainingMetric as ApplicationTrainingMetric, TrainingPolicy as ApplicationTrainingPolicy,
    TrainingReadiness as ApplicationTrainingReadiness, TrainingRun as ApplicationTrainingRun,
    TrainingRunPhase as ApplicationTrainingRunPhase, TrainingSource as ApplicationTrainingSource,
};
use spiral_robotics_studio_application::training_execution::{
    self, RunStateChange, TrainingCoordinatorPort, TrainingLaunch, TrainingRunIdentity,
    TrainingRunPort,
};
use spiral_robotics_studio_application::training_runs::{
    RunPhase as ApplicationRunPhase, RunRecord, TrainingRunRegistry,
};

static RUN_IDS: StableIdSequence = StableIdSequence::new("train", 0);

#[derive(Clone)]
pub(crate) struct TrainingManager {
    inner: Arc<TrainingManagerInner>,
}

struct TrainingManagerInner {
    storage: Arc<StorageManager>,
    training_backend: Arc<dyn TrainingBackend>,
    clock: Arc<dyn Clock>,
    snapshot: Mutex<TrainingSnapshot>,
    updates: broadcast::Sender<TrainingUpdate>,
    controls: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
}

struct RemoteReplayState {
    metric_step: u64,
    checkpoint_step: u64,
    persisted_logs: std::collections::BTreeSet<String>,
    configured: bool,
}

impl TrainingManager {
    /// Construct the in-memory manager without touching external services.
    pub(crate) fn new(
        storage: Arc<StorageManager>,
        training_backend: Arc<dyn TrainingBackend>,
    ) -> Self {
        Self::new_with_clock(storage, training_backend, Arc::new(SystemClock))
    }

    fn new_with_clock(
        storage: Arc<StorageManager>,
        training_backend: Arc<dyn TrainingBackend>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let (updates, _) = broadcast::channel(512);
        Self {
            inner: Arc::new(TrainingManagerInner {
                storage,
                training_backend,
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
            spiral_schema_version: spiral_robotics_contracts::spiral::SCHEMA_VERSION,
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
        let initial = tokio::task::spawn_blocking(move || {
            match backend.job(&existing_run_id, TrainingJobCursor::default()) {
                Ok(snapshot) => Ok(snapshot),
                Err(_) => backend.create_job(job_request),
            }
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
        let metric_step = persisted_run
            .as_ref()
            .and_then(|run| run.metrics.iter().map(|metric| metric.step).max())
            .unwrap_or(0);
        let checkpoint_step = persisted_run
            .as_ref()
            .and_then(|run| {
                run.checkpoints
                    .iter()
                    .map(|checkpoint| checkpoint.step)
                    .max()
            })
            .unwrap_or(0);
        let persisted_logs = persisted_run
            .map(|run| {
                run.logs
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>()
            })
            .unwrap_or_default();
        let mut replay = RemoteReplayState {
            metric_step,
            checkpoint_step,
            persisted_logs,
            configured: false,
        };
        let mut stop_sent = false;
        let mut poll_failures = 0_u64;
        loop {
            if let Err(error) =
                self.apply_remote_job_snapshot(&run_id, &request, &snapshot, &mut replay)
            {
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
            let cursor = snapshot.cursor;
            tokio::time::sleep(remote_poll_delay(poll_failures)).await;
            let next = tokio::task::spawn_blocking(move || {
                if should_stop {
                    backend.stop_job(&next_run_id, cursor)
                } else {
                    backend.job(&next_run_id, cursor)
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

    fn apply_remote_job_snapshot(
        &self,
        run_id: &str,
        request: &StartTrainingRequest,
        snapshot: &TrainingJobSnapshot,
        replay: &mut RemoteReplayState,
    ) -> Result<(), String> {
        if snapshot.job_id != run_id {
            return Err("training runtime returned the wrong job ID".to_owned());
        }
        if !replay.configured
            && let Some(resolved) = &snapshot.resolved
        {
            let mut resolved_request = request.clone();
            resolved_request.act.camera_streams = resolved.camera_streams.clone();
            self.apply_update(
                TrainingUpdate::RunConfigured {
                    run_id: run_id.to_owned(),
                    config: Box::new(resolved_request),
                },
                true,
            )?;
            replay.configured = true;
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
        if let Some(gap) = &snapshot.history_gap {
            self.push_log(
                run_id,
                format!("Training worker history was compacted: {gap}"),
            );
        }
        let previous_metric_step = replay.metric_step;
        for metric in snapshot
            .metrics
            .iter()
            .filter(|metric| metric.step > previous_metric_step)
        {
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
            replay.metric_step = metric.step;
        }
        let previous_checkpoint_step = replay.checkpoint_step;
        for checkpoint in snapshot
            .checkpoints
            .iter()
            .filter(|checkpoint| checkpoint.step > previous_checkpoint_step)
        {
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
            replay.checkpoint_step = checkpoint.step;
        }
        for log in &snapshot.logs {
            if !replay.persisted_logs.contains(&log.line) {
                self.push_log(run_id, log.line.clone());
            }
        }
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

fn remote_poll_delay(consecutive_failures: u64) -> Duration {
    if consecutive_failures == 0 {
        return Duration::from_millis(500);
    }
    let exponent = consecutive_failures.saturating_sub(1).min(5) as u32;
    Duration::from_secs(1_u64 << exponent).min(Duration::from_secs(30))
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
        match info.compatibility_error(spiral_robotics_contracts::spiral::SCHEMA_VERSION) {
            Some(error) => Err(error),
            None => Ok(info),
        }
    })
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
            spiral_robotics_studio_application::training::TrainingRuntimeInfo {
                id: runtime.id,
                display_name: runtime.display_name,
                runtime: runtime.runtime,
                devices: runtime.devices,
            }
        }),
        summary: readiness.summary,
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
    fn remote_polling_backs_off_but_remains_bounded() {
        assert_eq!(remote_poll_delay(0), Duration::from_millis(500));
        assert_eq!(remote_poll_delay(1), Duration::from_secs(1));
        assert_eq!(remote_poll_delay(4), Duration::from_secs(8));
        assert_eq!(remote_poll_delay(100), Duration::from_secs(30));
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
}
