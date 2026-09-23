//! One project's training runs: their durable history, and one monitor task
//! per active run that mirrors its remote worker job into that history.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use spiral_robotics_common::training::{
    TrainingCheckpoint, TrainingConfig, TrainingMetricPoint, TrainingReadiness, TrainingRun,
    TrainingRunPhase, TrainingRuntimeInfo, TrainingSnapshot, TrainingSource, TrainingUpdate,
};
use spiral_robotics_training::{
    ActLayerCounts, ActModelDimensions, ActTrainingArchitecture, ActTrainingInput,
    ActTrainingJobConfig, SpiralJobConnection, SpiralServiceEndpoints, TrainingBackend,
    TrainingCheckpointSchedule, TrainingDataColumns, TrainingDataQuery, TrainingDataSelection,
    TrainingImageSize, TrainingInitialization, TrainingJobCursor, TrainingJobIdentity,
    TrainingJobPhase, TrainingJobPlan, TrainingJobRequest, TrainingJobSnapshot, TrainingOptimizer,
    TrainingProgress, TrainingRuntimeSelection, TrainingSchedule,
};

use crate::OperationError;
use crate::adapters::clock::SystemClock;
use crate::adapters::compute_runtime::HttpTrainingBackend;
use crate::clock::{Clock, StableIdSequence};
use crate::sync::MutexRecovery;
use crate::training::service::StartTrainingAdmission as _;
use spiral_robotics_storage::StorageManager;
use spiral_robotics_storage::checkpoints::CheckpointStore;

const STARTING_MESSAGE: &str = "Opening SpiralDB and starting the remote training job…";
const STOPPING_MESSAGE: &str = "Finishing the current Spiral read or optimizer step…";

#[derive(Clone)]
pub(crate) struct TrainingManager {
    inner: Arc<TrainingRuntime>,
}

/// Runs live in SQLite; only what a running process owns lives here.
struct TrainingRuntime {
    storage: Arc<StorageManager>,
    clock: Arc<dyn Clock>,
    run_ids: StableIdSequence,
    /// The last probe of the training runtime and Spiral, a live service
    /// state rather than history.
    readiness: Mutex<Option<TrainingReadiness>>,
    /// Keeps read-modify-write of stored runs from interleaving.
    writes: tokio::sync::Mutex<()>,
    /// One stop flag per run whose monitor task is alive.
    stops: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
}

impl TrainingManager {
    /// Construct the manager without touching external services.
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            inner: Arc::new(TrainingRuntime {
                storage,
                clock: Arc::new(SystemClock),
                run_ids: StableIdSequence::new("train", 0),
                readiness: Mutex::new(None),
                writes: tokio::sync::Mutex::new(()),
                stops: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// The retained runs, read from storage. A run an earlier process left
    /// active gets its monitor back: remote jobs own execution, so a restarted
    /// Studio reattaches to them.
    async fn runs(&self) -> Result<Vec<TrainingRun>, OperationError> {
        let runs = self
            .inner
            .storage
            .training_history()
            .load()
            .await
            .map_err(OperationError::External)?;
        for run in runs.iter().filter(|run| run.phase().active()) {
            self.monitor(run);
        }
        Ok(runs)
    }

    pub(crate) async fn snapshot(&self) -> Result<TrainingSnapshot, OperationError> {
        let runs = self.runs().await?;
        Ok(TrainingSnapshot::new(self.readiness().await, runs))
    }

    pub(super) async fn catalog(&self) -> Result<TrainingSnapshot, OperationError> {
        Ok(self.snapshot().await?.catalog())
    }

    pub(super) async fn run(&self, id: &str) -> Result<TrainingRun, OperationError> {
        self.runs()
            .await?
            .into_iter()
            .find(|run| run.id() == id)
            .ok_or_else(|| OperationError::NotFound(format!("Training run {id:?} was not found")))
    }

    /// Whether a stored run is still active.
    pub(crate) async fn active(&self) -> Result<bool, OperationError> {
        Ok(self.runs().await?.iter().any(|run| run.phase().active()))
    }

    /// The last probe, or a first one.
    async fn readiness(&self) -> TrainingReadiness {
        let known = self.inner.readiness.recover_lock().clone();
        match known {
            Some(readiness) => readiness,
            None => self.refresh_readiness().await,
        }
    }

    pub(crate) async fn refresh_readiness(&self) -> TrainingReadiness {
        let next = self.probe_readiness().await;
        *self.inner.readiness.recover_lock() = Some(next.clone());
        next
    }

    pub(crate) async fn test_runtime(&self) -> (bool, String) {
        match self.probe_backend().await {
            Ok(info) => (true, format!("Connected · {}", info.display_summary())),
            Err(error) => (false, error),
        }
    }

    /// Admit one run on the runtime Settings selects now, and launch its
    /// monitor. The run keeps that runtime for its whole life.
    pub(super) async fn start(
        &self,
        config: TrainingConfig,
    ) -> Result<TrainingRun, OperationError> {
        if let Some(reason) = config.policy().mcap_training_blocker() {
            return Err(OperationError::Conflict(reason.into()));
        }
        let config = config.validated()?;
        let readiness = self.refresh_readiness().await;
        if !readiness.ready() {
            return Err(OperationError::Conflict(readiness.summary().to_owned()));
        }
        let endpoint = self
            .runtime_endpoint()
            .await
            .map_err(OperationError::External)?;
        let (id, created_ms) = self
            .inner
            .run_ids
            .next(self.inner.clock.as_ref())
            .into_parts();
        let run = TrainingRun::starting(id, config.clone())
            .with_source(TrainingSource::new(config.dataset_id()))
            .with_runtime_endpoint(endpoint)
            .with_started_ms(created_ms)
            .with_completion(None, Some(STARTING_MESSAGE.to_owned()));
        let admitted = run.clone();
        self.update_history(|history| {
            if let Some(active) = history.active_run() {
                return Err(format!(
                    "training run {:?} is already active; stop it before starting another",
                    active.id()
                ));
            }
            history.apply(TrainingUpdate::RunAdded {
                run: Box::new(admitted),
            });
            Ok(())
        })
        .await
        .map_err(OperationError::Conflict)?;
        self.monitor(&run);
        Ok(run)
    }

    /// Ask the run's worker to stop. The monitor is signalled even when the
    /// Stopping phase cannot be written.
    pub(super) async fn stop(&self, run_id: &str) -> Result<(), OperationError> {
        let Some(stop) = self.inner.stops.recover_lock().get(run_id).cloned() else {
            return Err(OperationError::Conflict(format!(
                "training run {run_id:?} is not active"
            )));
        };
        let stopping = self
            .update_run(run_id, |history| {
                history.enter_active_phase(
                    run_id,
                    TrainingRunPhase::Stopping,
                    Some(STOPPING_MESSAGE.to_owned()),
                )
            })
            .await;
        stop.store(true, Ordering::Release);
        stopping.map_err(OperationError::Conflict)
    }

    /// Remove checkpoint rows before the run itself, so a failed checkpoint
    /// deletion leaves the complete run visible and retryable instead of
    /// hiding orphaned model artifacts.
    pub(super) async fn delete(
        &self,
        run_id: &str,
        active_rollout: Option<&str>,
    ) -> Result<(), OperationError> {
        let history = TrainingSnapshot::new(TrainingReadiness::default(), self.runs().await?);
        let run_id = Self::admitted_deletion(&history, active_rollout, run_id)?;
        self.inner
            .storage
            .delete_run_checkpoints(run_id.clone())
            .await
            .map_err(OperationError::External)?;
        self.update_history(|history| {
            history.apply(TrainingUpdate::RunRemoved { run_id });
            Ok(())
        })
        .await
        .map_err(OperationError::External)
    }

    pub(crate) async fn shutdown(&self) {
        // A Studio shutdown only detaches the orchestration client. The remote worker owns
        // the job and must keep running so a restarted Studio process can reattach to it.
        self.inner.stops.recover_lock().clear();
    }

    /// Follow `run`'s job, unless a monitor already does.
    fn monitor(&self, run: &TrainingRun) {
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut stops = self.inner.stops.recover_lock();
            if stops.contains_key(run.id()) {
                return;
            }
            stops.insert(run.id().to_owned(), stop.clone());
        }
        let monitor = RemoteJobMonitor {
            manager: self.clone(),
            run_id: run.id().to_owned(),
            config: run.config().clone(),
            endpoint: run.runtime_endpoint().to_owned(),
            stop,
            stop_sent: false,
            mirrored: MirroredHistory::new(run),
        };
        tokio::spawn(async move { monitor.follow().await });
    }

    /// Change the retained runs as a whole: adding and removing runs keeps
    /// their order and bound.
    async fn update_history(
        &self,
        change: impl FnOnce(&mut TrainingSnapshot) -> Result<(), String>,
    ) -> Result<(), String> {
        let _write = self.inner.writes.lock().await;
        let runs = self.inner.storage.training_history().load().await?;
        let mut history = TrainingSnapshot::new(TrainingReadiness::default(), runs);
        change(&mut history)?;
        self.inner.storage.training_history().save(&history).await
    }

    /// Change one stored run. Updates name their run, so they apply to a
    /// history holding only that run.
    async fn update_run(
        &self,
        run_id: &str,
        change: impl FnOnce(&mut TrainingSnapshot) -> Result<(), String>,
    ) -> Result<(), String> {
        let _write = self.inner.writes.lock().await;
        let run = self
            .inner
            .storage
            .training_history()
            .load_run(run_id)
            .await?
            .ok_or_else(|| format!("training run {run_id:?} is not in history"))?;
        let mut history = TrainingSnapshot::new(TrainingReadiness::default(), vec![run]);
        change(&mut history)?;
        let run = history
            .runs()
            .first()
            .ok_or_else(|| format!("training run {run_id:?} was removed"))?;
        self.inner.storage.training_history().update_run(run).await
    }

    async fn probe_readiness(&self) -> TrainingReadiness {
        let backend_probe = self.probe_backend();
        let spiral_probe = async {
            tokio::time::timeout(Duration::from_secs(8), self.inner.storage.database().test())
                .await
                .unwrap_or_else(|_| {
                    (
                        false,
                        "the connection probe timed out after 8 seconds".to_owned(),
                    )
                })
        };
        let (backend, connection) = tokio::join!(backend_probe, spiral_probe);
        let runtime = backend.as_ref().ok().map(|info| {
            TrainingRuntimeInfo::new(info.id(), info.display_name(), info.runtime())
                .with_devices(info.devices().to_vec())
        });
        let spiral_ready = connection.0;
        let blocker = spiral_robotics_common::training::TrainingPolicy::Act.mcap_training_blocker();
        let ready = spiral_ready && backend.is_ok() && blocker.is_none();
        let summary = match (connection, backend, blocker) {
            ((false, message), _, _) => format!("Spiral is not ready for training: {message}"),
            (_, Err(error), _) => format!("Training runtime is not ready: {error}"),
            (_, Ok(_), Some(blocker)) => blocker.to_owned(),
            (_, Ok(backend), None) => format!(
                "Runtime ready · {probe} · Spiral project {} · namespace {} · query checked at run start",
                self.inner.storage.project_store().project_id(),
                self.inner.storage.project_store().table_prefix(),
                probe = backend.display_summary(),
            ),
        };
        TrainingReadiness::new(ready, spiral_ready, summary).with_runtime(runtime)
    }

    /// The runtime endpoint Settings selects now: what probes check and new
    /// runs start on. The blocking HTTP client is built inside the blocking
    /// task that uses it.
    async fn runtime_endpoint(&self) -> Result<String, String> {
        let preferences = self.inner.storage.preferences().load().await?;
        Ok(preferences.training().endpoint().to_owned())
    }

    async fn probe_backend(&self) -> Result<spiral_robotics_training::TrainingBackendInfo, String> {
        let endpoint = self.runtime_endpoint().await?;
        tokio::time::timeout(
            Duration::from_secs(8),
            tokio::task::spawn_blocking(move || HttpTrainingBackend::new(endpoint).health()),
        )
        .await
        .map_err(|_| "training backend health probe timed out after 8 seconds".to_owned())
        .and_then(|result| {
            result.map_err(|error| format!("training backend health task failed: {error}"))
        })
        .and_then(|result| result.map_err(|error| error.to_string()))
        .and_then(|info| {
            match info.compatibility_error(spiral_robotics_storage::SCHEMA_VERSION) {
                Some(error) => Err(error),
                None => Ok(info),
            }
        })
    }
}

/// One task's hold on the remote worker job behind an active run.
struct RemoteJobMonitor {
    manager: TrainingManager,
    run_id: String,
    config: TrainingConfig,
    /// Empty for a run recorded before runs kept their runtime.
    endpoint: String,
    stop: Arc<AtomicBool>,
    stop_sent: bool,
    mirrored: MirroredHistory,
}

/// What the stored run already holds of its job's history. A reattached job
/// replays everything the worker retains; only what is newer is mirrored.
struct MirroredHistory {
    metric_step: u64,
    checkpoint_step: u64,
    stored_logs: BTreeSet<String>,
    configured: bool,
    running: bool,
}

impl RemoteJobMonitor {
    /// Follow the job until it ends, mirroring its history into the run, then
    /// record how it ended. A run whose end cannot be written keeps its stop
    /// flag, so this process does not reattach to a run it cannot record.
    async fn follow(mut self) {
        let (phase, current_step, message) = match self.mirror_until_terminal().await {
            Ok(snapshot) => (
                snapshot.phase(),
                snapshot.current_step(),
                snapshot.message().unwrap_or_default().to_owned(),
            ),
            Err((current_step, error)) => (TrainingJobPhase::Failed, current_step, error),
        };
        let phase = match phase {
            TrainingJobPhase::Completed => TrainingRunPhase::Completed,
            TrainingJobPhase::Stopped => TrainingRunPhase::Stopped,
            TrainingJobPhase::Stopping => TrainingRunPhase::Stopping,
            TrainingJobPhase::Starting | TrainingJobPhase::Running | TrainingJobPhase::Failed => {
                TrainingRunPhase::Failed
            }
        };
        let manager = &self.manager;
        let ended = TrainingUpdate::RunState {
            run_id: self.run_id.clone(),
            phase,
            current_step,
            finished_ms: (!phase.active()).then(|| manager.inner.clock.now_ms()),
            message: (!message.is_empty()).then_some(message),
        };
        let recorded = manager
            .update_run(&self.run_id, |history| {
                history.apply(ended);
                Ok(())
            })
            .await;
        match recorded {
            Ok(()) => {
                manager.inner.stops.recover_lock().remove(&self.run_id);
            }
            Err(error) => {
                self.stop.store(true, Ordering::Release);
                tracing::error!(run = %self.run_id, "could not record how the training run ended: {error}");
            }
        }
    }

    /// The job's terminal snapshot, or the step it reached and why it failed.
    async fn mirror_until_terminal(&mut self) -> Result<TrainingJobSnapshot, (u64, String)> {
        if self.endpoint.is_empty() {
            self.endpoint = self
                .manager
                .runtime_endpoint()
                .await
                .map_err(|error| (0, error))?;
        }
        let mut snapshot = self.open().await.map_err(|error| (0, error))?;
        loop {
            self.mirror(&snapshot)
                .await
                .map_err(|error| (snapshot.current_step(), error))?;
            if snapshot.phase().is_terminal() {
                return Ok(snapshot);
            }
            snapshot = self
                .next(snapshot.cursor())
                .await
                .map_err(|error| (snapshot.current_step(), error))?;
        }
    }

    /// Reattach to the run's job, or create it when the worker has none.
    async fn open(&self) -> Result<TrainingJobSnapshot, String> {
        let (endpoint, run_id, request) = (
            self.endpoint.clone(),
            self.run_id.clone(),
            self.job_request(),
        );
        tokio::task::spawn_blocking(move || {
            let backend = HttpTrainingBackend::new(endpoint);
            backend
                .job_snapshot(&run_id, TrainingJobCursor::default())
                .or_else(|_| backend.create_job(request))
        })
        .await
        .map_err(|error| format!("training job launch task stopped: {error}"))?
        .map_err(|error| error.to_string())
    }

    /// The job's next snapshot after `cursor`. A briefly unreachable worker is
    /// retried with bounded backoff; once stop is asked for, the next request
    /// asks the worker to stop.
    async fn next(&mut self, cursor: TrainingJobCursor) -> Result<TrainingJobSnapshot, String> {
        let mut failures = 0_u64;
        loop {
            tokio::time::sleep(RemotePollBackoff::for_failures(failures)).await;
            let stopping = self.stop.load(Ordering::Acquire) && !self.stop_sent;
            let (endpoint, run_id) = (self.endpoint.clone(), self.run_id.clone());
            let polled = tokio::task::spawn_blocking(move || {
                let backend = HttpTrainingBackend::new(endpoint);
                if stopping {
                    backend.stop_job(&run_id, cursor)
                } else {
                    backend.job_snapshot(&run_id, cursor)
                }
            })
            .await
            .map_err(|error| format!("training job poll task stopped: {error}"))?;
            match polled {
                Ok(snapshot) => {
                    self.stop_sent |= stopping;
                    if failures > 0 {
                        self.log("Reconnected to the remote training worker.".to_owned())
                            .await;
                    }
                    return Ok(snapshot);
                }
                Err(error) => {
                    failures += 1;
                    if failures == 1 {
                        self.log(format!(
                            "Remote training worker is temporarily unavailable; retrying: {error}"
                        ))
                        .await;
                    }
                }
            }
        }
    }

    /// Write what `snapshot` adds to the run in one change: the resolved
    /// configuration once, the first Running phase, and newer metrics,
    /// checkpoints, and logs.
    async fn mirror(&mut self, snapshot: &TrainingJobSnapshot) -> Result<(), String> {
        let run_id = self.run_id.as_str();
        if snapshot.job_id() != run_id {
            return Err("training runtime returned the wrong job ID".to_owned());
        }
        let mut updates = Vec::new();
        let configure = !self.mirrored.configured && snapshot.resolved().is_some();
        if let Some(resolved) = snapshot.resolved().filter(|_| configure) {
            let mut config = self.config.clone();
            config
                .act_mut()
                .set_camera_streams(resolved.camera_streams().to_vec());
            updates.push(TrainingUpdate::RunConfigured {
                run_id: run_id.to_owned(),
                config: Box::new(config),
            });
        }
        if let Some(gap) = snapshot.history_gap() {
            updates.push(TrainingUpdate::Log {
                run_id: run_id.to_owned(),
                line: format!("Training worker history was compacted: {gap}"),
            });
        }
        let metric_floor = self.mirrored.metric_step;
        let mut metric_step = metric_floor;
        for metric in snapshot
            .metrics()
            .iter()
            .filter(|metric| metric.step() > metric_floor)
        {
            updates.push(TrainingUpdate::Metric {
                run_id: run_id.to_owned(),
                point: Box::new(
                    TrainingMetricPoint::new(metric.step(), metric.train_loss())
                        .with_learning_rate(metric.learning_rate())
                        .with_validation_loss(metric.validation_loss())
                        .with_timeline(metric.elapsed_seconds(), metric.event().map(str::to_owned))
                        .with_throughput(0.0, metric.examples_per_second())
                        .with_utilization(metric.cpu_percent(), metric.gpu_percent())
                        .with_memory_bytes(metric.memory_bytes())
                        .with_loader_ms(metric.loader_ms())
                        .with_runtime_timings(metric.device_copy_ms(), metric.model_ms()),
                ),
            });
            metric_step = metric.step();
        }
        let checkpoint_floor = self.mirrored.checkpoint_step;
        let mut checkpoint_step = checkpoint_floor;
        for checkpoint in snapshot
            .checkpoints()
            .iter()
            .filter(|checkpoint| checkpoint.step() > checkpoint_floor)
        {
            updates.push(TrainingUpdate::Checkpoint {
                run_id: run_id.to_owned(),
                checkpoint: TrainingCheckpoint::new(checkpoint.step(), checkpoint.created_ms())
                    .with_validation_loss(checkpoint.validation_loss())
                    .with_artifact(checkpoint.byte_size(), true)
                    .with_rollout(
                        checkpoint.recommended_action_steps(),
                        checkpoint.sample_hz(),
                    ),
            });
            checkpoint_step = checkpoint.step();
        }
        for log in snapshot.logs() {
            if !self.mirrored.stored_logs.contains(log.line()) {
                updates.push(TrainingUpdate::Log {
                    run_id: run_id.to_owned(),
                    line: log.line().to_owned(),
                });
            }
        }
        let running = snapshot.phase() == TrainingJobPhase::Running && !self.mirrored.running;
        if running || !updates.is_empty() {
            let message = snapshot.message().map(str::to_owned);
            self.manager
                .update_run(run_id, |history| {
                    // A run already asked to stop stays Stopping.
                    if running
                        && history
                            .runs()
                            .first()
                            .is_some_and(|run| run.phase() == TrainingRunPhase::Starting)
                    {
                        history.enter_active_phase(run_id, TrainingRunPhase::Running, message)?;
                    }
                    for update in updates {
                        history.apply(update);
                    }
                    Ok(())
                })
                .await?;
        }
        self.mirrored.configured |= configure;
        self.mirrored.running |= running;
        self.mirrored.metric_step = metric_step;
        self.mirrored.checkpoint_step = checkpoint_step;
        // Only the first, full replay can repeat what was already stored.
        self.mirrored.stored_logs.clear();
        Ok(())
    }

    /// A log line on its own, such as a reconnect. A line that cannot be
    /// written is only traced; the next mirror write reports storage failure.
    async fn log(&self, line: String) {
        let run_id = self.run_id.clone();
        if let Err(error) = self
            .manager
            .update_run(&self.run_id, |history| {
                history.apply(TrainingUpdate::Log { run_id, line });
                Ok(())
            })
            .await
        {
            tracing::warn!(run = %self.run_id, "could not record a training log line: {error}");
        }
    }

    fn job_request(&self) -> TrainingJobRequest {
        let (storage, config) = (&self.manager.inner.storage, &self.config);
        let spiral = SpiralJobConnection::new(
            SpiralServiceEndpoints::new(
                storage.project_store().endpoint(),
                storage.project_store().spfs_endpoint(),
            ),
            storage.project_store().project_id(),
            storage.project_store().table_prefix(),
        );
        let data = TrainingDataSelection::new(
            spiral,
            TrainingDataQuery::new(
                config.sql().to_owned(),
                TrainingDataColumns::new(
                    config.state_column().to_owned(),
                    config.action_column().to_owned(),
                ),
                spiral_robotics_storage::SCHEMA_VERSION,
            ),
        );
        let initialization = if config.act().pretrained_backbone() {
            TrainingInitialization::PretrainedBackbone
        } else {
            TrainingInitialization::Random
        };
        let schedule = TrainingSchedule::new(
            TrainingProgress::new(
                config.steps(),
                config.batch_size(),
                config.validation_split(),
            ),
            TrainingOptimizer::new(config.learning_rate(), config.seed()),
            TrainingCheckpointSchedule::new(config.checkpoint_every(), config.chunk_size()),
        );
        let act = ActTrainingJobConfig::new(
            ActTrainingInput::new(
                config.act().camera_streams().to_vec(),
                TrainingImageSize::new(config.act().image_width(), config.act().image_height()),
                config.act().video_decoder().to_owned(),
            ),
            ActTrainingArchitecture::new(
                ActModelDimensions::new(
                    config.act().hidden_dim(),
                    config.act().latent_dim(),
                    config.act().feedforward_dim(),
                ),
                config.act().attention_heads(),
                ActLayerCounts::new(
                    config.act().encoder_layers(),
                    config.act().vae_encoder_layers(),
                    config.act().decoder_layers(),
                ),
            ),
            config.act().kl_weight(),
        );
        TrainingJobRequest::new(
            TrainingJobIdentity::new(self.run_id.clone(), config.run_name().to_owned()),
            data,
            TrainingJobPlan::new(
                TrainingRuntimeSelection::new(config.device().to_owned(), initialization),
                schedule,
                act,
            ),
        )
    }
}

impl MirroredHistory {
    fn new(stored: &TrainingRun) -> Self {
        Self {
            metric_step: stored
                .metrics()
                .iter()
                .map(|metric| metric.step())
                .max()
                .unwrap_or(0),
            checkpoint_step: stored
                .checkpoints()
                .iter()
                .map(|checkpoint| checkpoint.step())
                .max()
                .unwrap_or(0),
            stored_logs: stored.logs().iter().cloned().collect(),
            configured: false,
            running: stored.phase() != TrainingRunPhase::Starting,
        }
    }
}

/// Phase changes of a run that is still active.
trait ActiveRunPhase {
    /// Move an active run to `phase`, keeping its progress; a run that has
    /// already ended is never revived.
    fn enter_active_phase(
        &mut self,
        run_id: &str,
        phase: TrainingRunPhase,
        message: Option<String>,
    ) -> Result<(), String>;
}

impl ActiveRunPhase for TrainingSnapshot {
    fn enter_active_phase(
        &mut self,
        run_id: &str,
        phase: TrainingRunPhase,
        message: Option<String>,
    ) -> Result<(), String> {
        let run = self
            .runs()
            .iter()
            .find(|run| run.id() == run_id)
            .ok_or_else(|| format!("training run {run_id:?} is not in history"))?;
        if !run.phase().active() {
            return Err(format!("training run {run_id:?} has already ended"));
        }
        let current_step = run.current_step();
        self.apply(TrainingUpdate::RunState {
            run_id: run_id.to_owned(),
            phase,
            current_step,
            finished_ms: None,
            message,
        });
        Ok(())
    }
}

struct RemotePollBackoff;

impl RemotePollBackoff {
    fn for_failures(consecutive_failures: u64) -> Duration {
        if consecutive_failures == 0 {
            return Duration::from_millis(500);
        }
        let exponent = consecutive_failures.saturating_sub(1).min(5) as u32;
        Duration::from_secs(1_u64 << exponent).min(Duration::from_secs(30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, phase: TrainingRunPhase, current_step: u64) -> TrainingRun {
        TrainingRun::starting(id, TrainingConfig::default())
            .with_source(TrainingSource::default())
            .with_phase(phase, current_step)
            .with_started_ms(1)
    }

    #[test]
    fn remote_polling_backs_off_but_remains_bounded() {
        assert_eq!(
            RemotePollBackoff::for_failures(0),
            Duration::from_millis(500)
        );
        assert_eq!(RemotePollBackoff::for_failures(1), Duration::from_secs(1));
        assert_eq!(RemotePollBackoff::for_failures(4), Duration::from_secs(8));
        assert_eq!(
            RemotePollBackoff::for_failures(100),
            Duration::from_secs(30)
        );
    }

    #[tokio::test]
    async fn every_change_is_stored_before_it_is_read() {
        let store = Arc::new(spiral_robotics_storage::ProjectStore::workflow_test());
        let studio =
            spiral_robotics_storage::StudioStore::in_memory().project(&store.connection_settings());
        let manager = TrainingManager::new(Arc::new(StorageManager::new(store, studio)));
        let ended = run("ended", TrainingRunPhase::Completed, 5)
            .with_runtime_endpoint("http://runtime.test");
        manager
            .update_history(|history| {
                history.apply(TrainingUpdate::RunAdded {
                    run: Box::new(ended),
                });
                Ok(())
            })
            .await
            .unwrap();
        manager
            .update_run("ended", |history| {
                history.apply(TrainingUpdate::Log {
                    run_id: "ended".to_owned(),
                    line: "kept".to_owned(),
                });
                Ok(())
            })
            .await
            .unwrap();
        let stored = manager.run("ended").await.unwrap();
        assert_eq!(stored.logs(), ["kept"]);
        assert_eq!(stored.runtime_endpoint(), "http://runtime.test");
        assert!(!manager.active().await.unwrap());
        assert!(manager.update_run("missing", |_| Ok(())).await.is_err());
    }

    #[test]
    fn an_ended_run_is_never_revived() {
        let mut snapshot = TrainingSnapshot::new(
            TrainingReadiness::default(),
            vec![
                run("complete", TrainingRunPhase::Completed, 10),
                run("live", TrainingRunPhase::Running, 3),
            ],
        );
        assert!(
            snapshot
                .enter_active_phase("complete", TrainingRunPhase::Stopping, None)
                .unwrap_err()
                .contains("already ended")
        );
        snapshot
            .enter_active_phase("live", TrainingRunPhase::Stopping, None)
            .unwrap();
        assert_eq!(snapshot.runs()[1].phase(), TrainingRunPhase::Stopping);
        assert_eq!(snapshot.runs()[1].current_step(), 3);
    }
}
