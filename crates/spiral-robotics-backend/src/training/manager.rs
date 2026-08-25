//! Rust-owned supervision for Spiral ADBC and an injected training backend.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use spiral_robotics_api::settings::ConnectionTestResponse;
use spiral_robotics_api::training::{
    ActArchitectureConfig, ActAttentionConfig, ActInputConfig, MAX_TRAINING_HISTORY_RUNS,
    StartTrainingRequest, TrainingCheckpoint, TrainingMetricPoint, TrainingMetricProgress,
    TrainingReadiness, TrainingResourceMetrics, TrainingRun, TrainingRunHistory as ApiRunHistory,
    TrainingRunIdentity as ApiRunIdentity, TrainingRunLifecycle, TrainingRunPhase,
    TrainingRuntimeInfo, TrainingSnapshot, TrainingSource, TrainingStageTimings, TrainingUpdate,
};
use spiral_robotics_training::{
    ActLayerCounts, ActModelDimensions, ActTrainingArchitecture, ActTrainingInput,
    ActTrainingJobConfig, SpiralJobConnection, SpiralServiceEndpoints, TrainingBackend,
    TrainingCheckpointSchedule, TrainingDataColumns, TrainingDataQuery, TrainingDataSelection,
    TrainingImageSize, TrainingInitialization, TrainingJobCursor, TrainingJobIdentity,
    TrainingJobPhase, TrainingJobPlan, TrainingJobRequest, TrainingJobSnapshot, TrainingOptimizer,
    TrainingProgress, TrainingRuntimeSelection, TrainingSchedule,
};
use tokio::sync::broadcast;

use crate::adapters::clock::SystemClock;
use crate::clock::{Clock, StableIdSequence};
use crate::training::execution::{
    self as training_execution, RunStateChange, TrainingCoordinatorPort, TrainingLaunch,
    TrainingRunIdentity, TrainingRunPort,
};
use crate::training::runs::{RunPhase as CapabilityRunPhase, RunRecord, TrainingRunRegistry};
use crate::training::service::{
    ActTrainingInputSettings as CapabilityActInputSettings,
    ActTrainingLayerSettings as CapabilityActLayerSettings,
    ActTrainingModelSettings as CapabilityActModelSettings,
    ActTrainingSettings as CapabilityActTrainingSettings,
    StartTrainingCommand as CapabilityStartTrainingCommand,
    TrainingCheckpoint as CapabilityTrainingCheckpoint,
    TrainingCheckpointSettings as CapabilityCheckpointSettings,
    TrainingCommandIdentity as CapabilityCommandIdentity,
    TrainingDataSettings as CapabilityDataSettings,
    TrainingExecutionSettings as CapabilityExecutionSettings,
    TrainingMetric as CapabilityTrainingMetric,
    TrainingOptimizerSettings as CapabilityOptimizerSettings,
    TrainingPolicy as CapabilityTrainingPolicy,
    TrainingProgressSettings as CapabilityProgressSettings,
    TrainingReadiness as CapabilityTrainingReadiness, TrainingRun as CapabilityTrainingRun,
    TrainingRunPhase as CapabilityTrainingRunPhase, TrainingSource as CapabilityTrainingSource,
};
use spiral_robotics_core::OperationError;
use spiral_robotics_storage::StorageManager;
use spiral_robotics_storage::checkpoints::CheckpointStore;

#[derive(Clone)]
pub(crate) struct TrainingManager {
    inner: Arc<TrainingManagerInner>,
}

struct TrainingManagerInner {
    storage: Arc<StorageManager>,
    training_backend: Arc<dyn TrainingBackend>,
    clock: Arc<dyn Clock>,
    run_ids: StableIdSequence,
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

struct RemoteJobSnapshot<'a> {
    run_id: &'a str,
    request: &'a StartTrainingRequest,
    snapshot: &'a TrainingJobSnapshot,
}

impl<'a> RemoteJobSnapshot<'a> {
    fn new(
        run_id: &'a str,
        request: &'a StartTrainingRequest,
        snapshot: &'a TrainingJobSnapshot,
    ) -> Self {
        Self {
            run_id,
            request,
            snapshot,
        }
    }
}

struct RemoteJobCompletion {
    run_id: String,
    phase: TrainingJobPhase,
    current_step: u64,
    message: String,
}

impl RemoteJobCompletion {
    fn new(run_id: String, phase: TrainingJobPhase, current_step: u64) -> Self {
        Self {
            run_id,
            phase,
            current_step,
            message: String::new(),
        }
    }

    fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }
}

struct TrainingPhaseUpdate {
    run_id: String,
    phase: TrainingRunPhase,
    finished_ms: Option<i64>,
    message: Option<String>,
}

impl TrainingPhaseUpdate {
    fn new(run_id: String, phase: TrainingRunPhase) -> Self {
        Self {
            run_id,
            phase,
            finished_ms: None,
            message: None,
        }
    }

    fn with_completion(mut self, finished_ms: Option<i64>, message: Option<String>) -> Self {
        self.finished_ms = finished_ms;
        self.message = message;
        self
    }
}

struct PersistenceFailure<'a> {
    run_id: &'a str,
    finished_ms: i64,
    error: &'a str,
}

impl<'a> PersistenceFailure<'a> {
    fn new(run_id: &'a str, finished_ms: i64, error: &'a str) -> Self {
        Self {
            run_id,
            finished_ms,
            error,
        }
    }

    fn updates(
        self,
        snapshot: &TrainingSnapshot,
    ) -> Result<(TrainingUpdate, TrainingUpdate), String> {
        let run = snapshot
            .runs()
            .iter()
            .find(|run| run.id() == self.run_id)
            .ok_or_else(|| format!("training run {:?} is not in history", self.run_id))?;
        let message = format!(
            "Training history could not be persisted, so execution was stopped: {}",
            self.error
        );
        Ok((
            TrainingUpdate::Log {
                run_id: self.run_id.to_owned(),
                line: message.clone(),
            },
            TrainingUpdate::RunState {
                run_id: self.run_id.to_owned(),
                phase: TrainingRunPhase::Failed,
                current_step: run.current_step(),
                finished_ms: Some(self.finished_ms),
                message: Some(message),
            },
        ))
    }
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
                run_ids: StableIdSequence::new("train", 0),
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
        let retained_runs = snapshot
            .runs()
            .iter()
            .filter(|run| retained.contains(run.id()))
            .cloned()
            .collect();
        // Readiness is live service state, not durable history. Preserve the
        // concurrently computed value if probes finish before hydration.
        let mut current = lock(&self.inner.snapshot);
        snapshot = TrainingSnapshot::new(current.readiness().clone(), retained_runs);
        *current = snapshot.clone();
        drop(current);
        let _ = self.inner.updates.send(TrainingUpdate::Snapshot {
            snapshot: Box::new(snapshot.clone()),
        });
        // A worker-owned job continues while Studio is down. Re-submit the
        // immutable ID (the runtime treats POST as idempotent) and resume
        // polling its replayable state instead of marking the run failed.
        for run in snapshot.runs().iter().filter(|run| run.phase().active()) {
            let run = run.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            lock(&self.inner.controls).insert(run.id().to_owned(), cancel.clone());
            let manager = self.clone();
            tokio::spawn(async move {
                manager
                    .run_backend(
                        TrainingLaunch::new(
                            run.id(),
                            capability_training_command(run.config().clone()),
                        ),
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
        if lock(&self.inner.snapshot).readiness() != &next {
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
            Ok(info) => {
                ConnectionTestResponse::new(true, format!("Connected · {}", info.display_summary()))
            }
            Err(error) => ConnectionTestResponse::new(false, error),
        }
    }

    pub(crate) async fn start(&self, request: StartTrainingRequest) -> Result<TrainingRun, String> {
        training_execution::coordinate_training_start(self, capability_training_command(request))
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
            .runs()
            .iter()
            .find(|run| run.id() == run_id)
            .cloned()
            .ok_or_else(|| format!("training run {run_id:?} is no longer available"))?;
        if run.phase().active() {
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
            .runs()
            .iter()
            .find(|candidate| candidate.id() == run_id)
            .ok_or_else(|| format!("training run {run_id:?} is no longer available"))?;
        if current.phase().active() {
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
        let (run_id, command) = launch.into_parts();
        let request = contract_training_request(command);
        let credentials = self.inner.storage.spiral_credentials();
        let spiral = SpiralJobConnection::new(
            SpiralServiceEndpoints::new(
                self.inner.storage.spiral_endpoint(),
                self.inner.storage.spiral_spfs_endpoint(),
            ),
            self.inner.storage.project_id(),
            self.inner.storage.namespace(),
        )
        .with_credentials(
            credentials
                .as_ref()
                .map(|credentials| credentials.client_id().to_owned()),
            credentials
                .as_ref()
                .map(|credentials| credentials.client_secret().to_owned()),
        );
        let data = TrainingDataSelection::new(
            spiral,
            TrainingDataQuery::new(
                request.sql().to_owned(),
                TrainingDataColumns::new(
                    request.state_column().to_owned(),
                    request.action_column().to_owned(),
                ),
                spiral_robotics_storage::SCHEMA_VERSION,
            ),
        );
        let initialization = if request.act().pretrained_backbone() {
            TrainingInitialization::PretrainedBackbone
        } else {
            TrainingInitialization::Random
        };
        let schedule = TrainingSchedule::new(
            TrainingProgress::new(
                request.steps(),
                request.batch_size(),
                request.validation_split(),
            ),
            TrainingOptimizer::new(request.learning_rate(), request.seed()),
            TrainingCheckpointSchedule::new(request.checkpoint_every(), request.chunk_size()),
        );
        let act = ActTrainingJobConfig::new(
            ActTrainingInput::new(
                request.act().camera_streams().to_vec(),
                TrainingImageSize::new(request.act().image_width(), request.act().image_height()),
                request.act().video_decoder().to_owned(),
            ),
            ActTrainingArchitecture::new(
                ActModelDimensions::new(
                    request.act().hidden_dim(),
                    request.act().latent_dim(),
                    request.act().feedforward_dim(),
                ),
                request.act().attention_heads(),
                ActLayerCounts::new(
                    request.act().encoder_layers(),
                    request.act().vae_encoder_layers(),
                    request.act().decoder_layers(),
                ),
            ),
            request.act().kl_weight(),
        );
        let job_request = TrainingJobRequest::new(
            TrainingJobIdentity::new(run_id.clone(), request.run_name().to_owned()),
            data,
            TrainingJobPlan::new(
                TrainingRuntimeSelection::new(request.device().to_owned(), initialization),
                schedule,
                act,
            ),
        );
        let backend = self.inner.training_backend.clone();
        let existing_run_id = run_id.clone();
        let initial = tokio::task::spawn_blocking(move || {
            match backend.job_snapshot(&existing_run_id, TrainingJobCursor::default()) {
                Ok(snapshot) => Ok(snapshot),
                Err(_) => backend.create_job(job_request),
            }
        })
        .await;
        let mut snapshot = match initial {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                self.finish_remote_job(
                    RemoteJobCompletion::new(run_id.clone(), TrainingJobPhase::Failed, 0)
                        .with_message(error.to_string()),
                );
                return;
            }
            Err(error) => {
                self.finish_remote_job(
                    RemoteJobCompletion::new(run_id.clone(), TrainingJobPhase::Failed, 0)
                        .with_message(format!("training job launch task stopped: {error}")),
                );
                return;
            }
        };
        let persisted_run = lock(&self.inner.snapshot)
            .runs()
            .iter()
            .find(|run| run.id() == run_id)
            .cloned();
        let metric_step = persisted_run
            .as_ref()
            .and_then(|run| run.metrics().iter().map(|metric| metric.step()).max())
            .unwrap_or(0);
        let checkpoint_step = persisted_run
            .as_ref()
            .and_then(|run| {
                run.checkpoints()
                    .iter()
                    .map(|checkpoint| checkpoint.step())
                    .max()
            })
            .unwrap_or(0);
        let persisted_logs = persisted_run
            .map(|run| {
                run.logs()
                    .iter()
                    .cloned()
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
            if let Err(error) = self.apply_remote_job_snapshot(
                RemoteJobSnapshot::new(&run_id, &request, &snapshot),
                &mut replay,
            ) {
                self.finish_remote_job(
                    RemoteJobCompletion::new(
                        run_id.clone(),
                        TrainingJobPhase::Failed,
                        snapshot.current_step(),
                    )
                    .with_message(error),
                );
                return;
            }
            if snapshot.phase().is_terminal() {
                self.finish_remote_job(
                    RemoteJobCompletion::new(
                        run_id.clone(),
                        snapshot.phase(),
                        snapshot.current_step(),
                    )
                    .with_message(snapshot.message().unwrap_or_default()),
                );
                return;
            }
            let backend = self.inner.training_backend.clone();
            let next_run_id = run_id.clone();
            let should_stop = cancel.load(Ordering::Acquire) && !stop_sent;
            let cursor = snapshot.cursor();
            tokio::time::sleep(remote_poll_delay(poll_failures)).await;
            let next = tokio::task::spawn_blocking(move || {
                if should_stop {
                    backend.stop_job(&next_run_id, cursor)
                } else {
                    backend.job_snapshot(&next_run_id, cursor)
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
                        RemoteJobCompletion::new(
                            run_id.clone(),
                            TrainingJobPhase::Failed,
                            snapshot.current_step(),
                        )
                        .with_message(format!("training job poll task stopped: {error}")),
                    );
                    return;
                }
            };
        }
    }

    fn apply_remote_job_snapshot(
        &self,
        remote: RemoteJobSnapshot<'_>,
        replay: &mut RemoteReplayState,
    ) -> Result<(), String> {
        let RemoteJobSnapshot {
            run_id,
            request,
            snapshot,
        } = remote;
        if snapshot.job_id() != run_id {
            return Err("training runtime returned the wrong job ID".to_owned());
        }
        if !replay.configured
            && let Some(resolved) = snapshot.resolved()
        {
            let resolved_request = contract_training_request(
                capability_training_command(request.clone())
                    .with_camera_streams(resolved.camera_streams().to_vec()),
            );
            self.apply_update(
                TrainingUpdate::RunConfigured {
                    run_id: run_id.to_owned(),
                    config: Box::new(resolved_request),
                },
                true,
            )?;
            replay.configured = true;
        }
        if snapshot.phase() == TrainingJobPhase::Running
            && self.phase(run_id) == Some(TrainingRunPhase::Starting)
        {
            self.set_phase(
                TrainingPhaseUpdate::new(run_id.to_owned(), TrainingRunPhase::Running)
                    .with_completion(None, snapshot.message().map(str::to_owned)),
            )?;
        }
        if let Some(gap) = snapshot.history_gap() {
            self.push_log(
                run_id,
                format!("Training worker history was compacted: {gap}"),
            );
        }
        let previous_metric_step = replay.metric_step;
        for metric in snapshot
            .metrics()
            .iter()
            .filter(|metric| metric.step() > previous_metric_step)
        {
            self.apply_update(
                TrainingUpdate::Metric {
                    run_id: run_id.to_owned(),
                    point: Box::new(TrainingMetricPoint::new(
                        TrainingMetricProgress::new(
                            metric.step(),
                            metric.train_loss(),
                            metric.learning_rate(),
                        )
                        .with_validation_loss(metric.validation_loss())
                        .with_timeline(metric.elapsed_seconds(), metric.event().map(str::to_owned)),
                        TrainingResourceMetrics::new(0.0, metric.examples_per_second())
                            .with_utilization(metric.cpu_percent(), metric.gpu_percent())
                            .with_memory_bytes(metric.memory_bytes()),
                        TrainingStageTimings::new(metric.loader_ms(), None, None)
                            .with_runtime(metric.device_copy_ms(), metric.model_ms()),
                    )),
                },
                false,
            )?;
            replay.metric_step = metric.step();
        }
        let previous_checkpoint_step = replay.checkpoint_step;
        for checkpoint in snapshot
            .checkpoints()
            .iter()
            .filter(|checkpoint| checkpoint.step() > previous_checkpoint_step)
        {
            self.inner.storage.invalidate_idle_connections();
            self.apply_update(
                TrainingUpdate::Checkpoint {
                    run_id: run_id.to_owned(),
                    checkpoint: TrainingCheckpoint::new(checkpoint.step(), checkpoint.created_ms())
                        .with_validation_loss(checkpoint.validation_loss())
                        .with_artifact(checkpoint.byte_size(), true)
                        .with_rollout(
                            checkpoint.recommended_action_steps(),
                            checkpoint.sample_hz(),
                        ),
                },
                true,
            )?;
            replay.checkpoint_step = checkpoint.step();
        }
        for log in snapshot.logs() {
            if !replay.persisted_logs.contains(log.line()) {
                self.push_log(run_id, log.line().to_owned());
            }
        }
        Ok(())
    }

    fn finish_remote_job(&self, completion: RemoteJobCompletion) {
        let phase = match completion.phase {
            TrainingJobPhase::Completed => TrainingRunPhase::Completed,
            TrainingJobPhase::Stopped => TrainingRunPhase::Stopped,
            TrainingJobPhase::Stopping => TrainingRunPhase::Stopping,
            TrainingJobPhase::Starting | TrainingJobPhase::Running | TrainingJobPhase::Failed => {
                TrainingRunPhase::Failed
            }
        };
        let update = TrainingUpdate::RunState {
            run_id: completion.run_id.clone(),
            phase,
            current_step: completion.current_step,
            finished_ms: (!phase.active()).then(|| self.inner.clock.now_ms()),
            message: (!completion.message.is_empty()).then_some(completion.message),
        };
        let _ = self.apply_update(update, true);
        lock(&self.inner.controls).remove(&completion.run_id);
    }

    fn phase(&self, run_id: &str) -> Option<TrainingRunPhase> {
        lock(&self.inner.snapshot)
            .runs()
            .iter()
            .find(|run| run.id() == run_id)
            .map(|run| run.phase())
    }

    fn set_phase(&self, update: TrainingPhaseUpdate) -> Result<(), String> {
        let TrainingPhaseUpdate {
            run_id,
            phase,
            finished_ms,
            message,
        } = update;
        let run_id = run_id.as_str();
        let mut snapshot = lock(&self.inner.snapshot);
        let current_step = snapshot
            .runs()
            .iter()
            .find(|run| run.id() == run_id)
            .map_or(0, |run| run.current_step());
        let mut registry = registry(&snapshot)?;
        registry.transition(run_id, capability_phase(phase))?;
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
            PersistenceFailure::new(run_id, self.inner.clock.now_ms(), error).updates(&snapshot)
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
            TrainingPhaseUpdate::new(run_id.to_owned(), contract_training_phase(change.phase()))
                .with_completion(change.finished_ms(), change.message().map(str::to_owned)),
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
    async fn refresh_training_readiness(&self) -> CapabilityTrainingReadiness {
        capability_training_readiness(self.refresh_readiness().await)
    }

    fn resolve_training_source(
        &self,
        dataset_id: &str,
    ) -> Result<CapabilityTrainingSource, OperationError> {
        Ok(CapabilityTrainingSource::new(dataset_id))
    }

    async fn validate_training_query(
        &self,
        _command: &CapabilityStartTrainingCommand,
    ) -> Result<(), OperationError> {
        // The worker owns the Spiral connection and validates the immutable
        // query while resolving its data contract. Studio does not execute the
        // training query or read a training batch.
        Ok(())
    }

    fn next_training_identity(&self) -> TrainingRunIdentity {
        let (id, created_ms) = self
            .inner
            .run_ids
            .next(self.inner.clock.as_ref())
            .into_parts();
        TrainingRunIdentity::new(id, created_ms)
    }

    fn admit_training_run(&self, run: CapabilityTrainingRun) -> Result<(), OperationError> {
        // Registry admission, durable snapshot replacement, and event delivery
        // share one lock, so concurrent starts cannot both become active.
        let mut snapshot = lock(&self.inner.snapshot);
        let (candidate, update) = admitted_snapshot(
            self.inner.storage.as_ref(),
            &snapshot,
            contract_training_run(run),
        )
        .map_err(OperationError::conflict)?;
        *snapshot = candidate;
        let _ = self.inner.updates.send(update);
        Ok(())
    }

    fn launch_training_runtime(&self, launch: TrainingLaunch) -> Result<(), OperationError> {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut controls = lock(&self.inner.controls);
            if controls.contains_key(launch.run_id()) {
                return Err(OperationError::Internal(format!(
                    "training runtime {:?} was already launched",
                    launch.run_id()
                )));
            }
            controls.insert(launch.run_id().to_owned(), cancel.clone());
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
            .unwrap_or_else(|_| {
                ConnectionTestResponse::new(false, "the connection probe timed out after 8 seconds")
            })
    };
    let (backend, connection) = tokio::join!(backend_probe, spiral_probe);
    let runtime = backend.as_ref().ok().map(|info| {
        TrainingRuntimeInfo::new(info.id(), info.display_name(), info.runtime())
            .with_devices(info.devices().to_vec())
    });
    let spiral_ready = connection.ok();
    let ready = spiral_ready && backend.is_ok();
    let summary = match (connection, backend) {
        (result, _) if !result.ok() => {
            format!("Spiral is not ready for training: {}", result.message())
        }
        (_, Err(error)) => format!("Training runtime is not ready: {error}"),
        (_, Ok(backend)) => format!(
            "Runtime ready · {probe} · Spiral project {} · namespace {} · query checked at run start",
            storage.project_id(),
            storage.namespace(),
            probe = backend.display_summary(),
        ),
    };
    TrainingReadiness::new(ready, spiral_ready, summary).with_runtime(runtime)
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
    .and_then(
        |info| match info.compatibility_error(spiral_robotics_storage::SCHEMA_VERSION) {
            Some(error) => Err(error),
            None => Ok(info),
        },
    )
}

fn capability_training_command(request: StartTrainingRequest) -> CapabilityStartTrainingCommand {
    let identity = CapabilityCommandIdentity::new(
        request.run_name(),
        request.dataset_id(),
        match request.policy() {
            spiral_robotics_api::training::TrainingPolicy::Act => CapabilityTrainingPolicy::Act,
        },
    );
    let data = CapabilityDataSettings::new(
        request.sql(),
        request.state_column(),
        request.action_column(),
    );
    let act = CapabilityActTrainingSettings::new(
        CapabilityActInputSettings::new(
            request.act().camera_streams().to_vec(),
            (request.act().image_width(), request.act().image_height()),
            request.act().video_decoder(),
        )
        .with_pretrained_backbone(request.act().pretrained_backbone()),
        CapabilityActModelSettings::new(
            request.act().hidden_dim(),
            request.act().latent_dim(),
            request.act().attention_heads(),
        )
        .with_feedforward(request.act().feedforward_dim(), request.act().kl_weight()),
        CapabilityActLayerSettings::new(
            request.act().encoder_layers(),
            request.act().vae_encoder_layers(),
            request.act().decoder_layers(),
        ),
    );
    let execution = CapabilityExecutionSettings::new(
        request.device(),
        CapabilityProgressSettings::new(
            request.steps(),
            request.batch_size(),
            request.validation_split(),
        ),
        act,
    )
    .with_schedule(
        CapabilityOptimizerSettings::new(request.learning_rate(), request.seed()),
        CapabilityCheckpointSettings::new(request.checkpoint_every(), request.chunk_size()),
    );
    CapabilityStartTrainingCommand::new(identity, data, execution)
}

fn contract_training_request(command: CapabilityStartTrainingCommand) -> StartTrainingRequest {
    let selection = spiral_robotics_api::training::TrainingDataSelection::new(
        command.run_name(),
        command.dataset_id(),
        command.query(),
    )
    .with_policy(match command.policy() {
        CapabilityTrainingPolicy::Act => spiral_robotics_api::training::TrainingPolicy::Act,
    })
    .with_columns(command.state_column(), command.action_column())
    .with_device(command.device());
    let optimization = spiral_robotics_api::training::TrainingOptimizationConfig::new(
        command.steps(),
        command.batch_size(),
        command.learning_rate(),
    )
    .with_validation_split(command.validation_split())
    .with_checkpoint_schedule(command.checkpoint_every(), command.chunk_size())
    .with_seed(command.seed());
    let act = spiral_robotics_api::training::ActTrainingConfig::new(
        ActInputConfig::new(
            command.act().camera_streams().to_vec(),
            (command.act().image_width(), command.act().image_height()),
            command.act().video_decoder(),
        )
        .with_pretrained_backbone(command.act().pretrained_backbone()),
        ActArchitectureConfig::new(
            command.act().hidden_dim(),
            command.act().latent_dim(),
            spiral_robotics_api::training::ActLayerCounts::new(
                command.act().encoder_layers(),
                command.act().vae_encoder_layers(),
                command.act().decoder_layers(),
            ),
        ),
        ActAttentionConfig::new(
            command.act().attention_heads(),
            command.act().feedforward_dim(),
            command.act().kl_weight(),
        ),
    );
    StartTrainingRequest::new(selection, optimization, act)
}

fn contract_training_source(source: CapabilityTrainingSource) -> TrainingSource {
    TrainingSource::new(source.dataset_id())
}

fn contract_training_phase(phase: CapabilityTrainingRunPhase) -> TrainingRunPhase {
    match phase {
        CapabilityTrainingRunPhase::Starting => TrainingRunPhase::Starting,
        CapabilityTrainingRunPhase::Running => TrainingRunPhase::Running,
        CapabilityTrainingRunPhase::Stopping => TrainingRunPhase::Stopping,
        CapabilityTrainingRunPhase::Completed => TrainingRunPhase::Completed,
        CapabilityTrainingRunPhase::Stopped => TrainingRunPhase::Stopped,
        CapabilityTrainingRunPhase::Failed => TrainingRunPhase::Failed,
    }
}

fn capability_training_readiness(readiness: TrainingReadiness) -> CapabilityTrainingReadiness {
    CapabilityTrainingReadiness::new(
        readiness.ready(),
        readiness.spiral_ready(),
        readiness.summary(),
    )
    .with_runtime(readiness.runtime().map(|runtime| {
        crate::training::service::TrainingRuntimeInfo::new(
            runtime.id(),
            runtime.display_name(),
            runtime.runtime(),
        )
        .with_devices(runtime.devices().to_vec())
    }))
}

fn contract_training_metric(metric: CapabilityTrainingMetric) -> TrainingMetricPoint {
    TrainingMetricPoint::new(
        TrainingMetricProgress::new(metric.step(), metric.train_loss(), metric.learning_rate())
            .with_validation_loss(metric.validation_loss())
            .with_timeline(metric.elapsed_seconds(), metric.event().map(str::to_owned)),
        TrainingResourceMetrics::new(
            metric.transfer_bytes_per_second(),
            metric.examples_per_second(),
        )
        .with_utilization(metric.cpu_percent(), metric.gpu_percent())
        .with_memory_bytes(metric.memory_bytes()),
        TrainingStageTimings::new(
            metric.loader_ms(),
            metric.batch_encode_ms(),
            metric.upload_ms(),
        )
        .with_runtime(metric.device_copy_ms(), metric.model_ms()),
    )
}

fn contract_training_checkpoint(checkpoint: CapabilityTrainingCheckpoint) -> TrainingCheckpoint {
    TrainingCheckpoint::new(checkpoint.step(), checkpoint.created_ms())
        .with_validation_loss(checkpoint.validation_loss())
        .with_artifact(checkpoint.byte_size(), checkpoint.inference_ready())
        .with_rollout(
            checkpoint.recommended_action_steps(),
            checkpoint.sample_hz(),
        )
}

fn contract_training_run(run: CapabilityTrainingRun) -> TrainingRun {
    TrainingRun::new(
        ApiRunIdentity::new(
            run.id(),
            contract_training_request(run.config().clone()),
            contract_training_source(run.source().clone()),
        ),
        TrainingRunLifecycle::new(
            contract_training_phase(run.phase()),
            run.current_step(),
            run.started_ms(),
        )
        .with_completion(run.finished_ms(), run.message().map(str::to_owned)),
        ApiRunHistory::new(
            run.metrics()
                .iter()
                .cloned()
                .map(contract_training_metric)
                .collect(),
            run.checkpoints()
                .iter()
                .cloned()
                .map(contract_training_checkpoint)
                .collect(),
            run.logs().to_vec(),
        ),
    )
}

fn capability_phase(phase: TrainingRunPhase) -> CapabilityRunPhase {
    match phase {
        TrainingRunPhase::Starting => CapabilityRunPhase::Starting,
        TrainingRunPhase::Running => CapabilityRunPhase::Running,
        TrainingRunPhase::Stopping => CapabilityRunPhase::Stopping,
        TrainingRunPhase::Completed => CapabilityRunPhase::Completed,
        TrainingRunPhase::Stopped => CapabilityRunPhase::Stopped,
        TrainingRunPhase::Failed => CapabilityRunPhase::Failed,
    }
}

fn registry(snapshot: &TrainingSnapshot) -> Result<TrainingRunRegistry, String> {
    TrainingRunRegistry::new(
        MAX_TRAINING_HISTORY_RUNS,
        snapshot
            .runs()
            .iter()
            .map(|run| RunRecord::new(run.id(), capability_phase(run.phase()))),
    )
}

fn admitted_snapshot(
    storage: &StorageManager,
    snapshot: &TrainingSnapshot,
    run: TrainingRun,
) -> Result<(TrainingSnapshot, TrainingUpdate), String> {
    let mut registry = registry(snapshot)?;
    registry.admit(RunRecord::new(run.id(), capability_phase(run.phase())))?;
    let retained = registry
        .retained_ids()
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let update = TrainingUpdate::RunAdded { run: Box::new(run) };
    let mut candidate = snapshot.clone();
    candidate.apply(update.clone());
    let removed = candidate
        .runs()
        .iter()
        .filter(|run| !retained.contains(run.id()))
        .map(|run| run.id().to_owned())
        .collect::<Vec<_>>();
    for run_id in removed {
        candidate.apply(TrainingUpdate::RunRemoved { run_id });
    }
    storage.save_training_snapshot(&candidate)?;
    Ok((candidate, update))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, phase: TrainingRunPhase, current_step: u64) -> TrainingRun {
        TrainingRun::new(
            spiral_robotics_api::training::TrainingRunIdentity::new(
                id,
                StartTrainingRequest::default(),
                TrainingSource::default(),
            ),
            spiral_robotics_api::training::TrainingRunLifecycle::new(phase, current_step, 1),
            spiral_robotics_api::training::TrainingRunHistory::default(),
        )
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
        let active = run("active", TrainingRunPhase::Running, 12);
        let mut snapshot = TrainingSnapshot::new(TrainingReadiness::default(), vec![active]);

        let (log, state) = PersistenceFailure::new("active", 99, "Spiral unavailable")
            .updates(&snapshot)
            .unwrap();
        snapshot.apply(log);
        snapshot.apply(state);

        let failed = &snapshot.runs()[0];
        assert_eq!(failed.phase(), TrainingRunPhase::Failed);
        assert_eq!(failed.current_step(), 12);
        assert_eq!(failed.finished_ms(), Some(99));
        assert!(failed.message().unwrap().contains("Spiral unavailable"));
        assert!(
            failed
                .logs()
                .last()
                .unwrap()
                .contains("execution was stopped")
        );
    }
}
