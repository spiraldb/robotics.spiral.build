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
use crate::clock::{Clock, StableIdSequence};
use crate::project_admission::ProjectActivity;
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

struct TrainingRuntime {
    storage: Arc<StorageManager>,
    backend: Arc<dyn TrainingBackend>,
    clock: Arc<dyn Clock>,
    run_ids: StableIdSequence,
    history: TrainingHistory,
    /// One stop flag per run whose monitor task is alive.
    stops: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
}

/// The retained runs. Every durable change is written before it becomes
/// visible, and `writes` keeps changes from interleaving with a write.
struct TrainingHistory {
    loaded: tokio::sync::OnceCell<()>,
    current: Mutex<TrainingSnapshot>,
    writes: tokio::sync::Mutex<()>,
}

impl TrainingManager {
    /// Construct the in-memory manager without touching external services.
    pub(crate) fn new(storage: Arc<StorageManager>, backend: Arc<dyn TrainingBackend>) -> Self {
        Self {
            inner: Arc::new(TrainingRuntime {
                storage,
                backend,
                clock: Arc::new(SystemClock),
                run_ids: StableIdSequence::new("train", 0),
                history: TrainingHistory {
                    loaded: tokio::sync::OnceCell::new(),
                    current: Mutex::new(TrainingSnapshot::default()),
                    writes: tokio::sync::Mutex::new(()),
                },
                stops: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// Load history once on the first Training or Rollout request, and resume
    /// monitoring the runs it left active. Remote jobs own execution, so a
    /// restarted Studio reattaches to them.
    pub(crate) async fn ensure_loaded(&self, activity: ProjectActivity) -> Result<(), String> {
        let history = &self.inner.history;
        history
            .loaded
            .get_or_try_init(|| async {
                let runs = self.inner.storage.load_training_history().await?;
                {
                    let _write = history.writes.lock().await;
                    let mut current = history.current.recover_lock();
                    // Readiness is live service state, not durable history.
                    *current = TrainingSnapshot::new(current.readiness().clone(), runs);
                }
                for run in self
                    .snapshot()
                    .runs()
                    .iter()
                    .filter(|run| run.phase().active())
                {
                    self.monitor(run.id(), run.config().clone(), activity.clone());
                }
                self.refresh_readiness().await;
                Ok::<(), String>(())
            })
            .await
            .map(|_| ())
    }

    pub(crate) fn snapshot(&self) -> TrainingSnapshot {
        self.inner.history.current.recover_lock().clone()
    }

    pub(super) fn catalog(&self) -> TrainingSnapshot {
        self.inner.history.current.recover_lock().catalog()
    }

    pub(super) fn run(&self, id: &str) -> Result<TrainingRun, OperationError> {
        self.inner
            .history
            .current
            .recover_lock()
            .runs()
            .iter()
            .find(|run| run.id() == id)
            .cloned()
            .ok_or_else(|| OperationError::NotFound(format!("Training run {id:?} was not found")))
    }

    pub(crate) async fn refresh_readiness(&self) -> TrainingReadiness {
        let next = self.readiness().await;
        self.record(TrainingUpdate::Readiness {
            readiness: next.clone(),
        })
        .await;
        next
    }

    pub(crate) async fn test_runtime(&self) -> (bool, String) {
        match self.probe_backend().await {
            Ok(info) => (true, format!("Connected · {}", info.display_summary())),
            Err(error) => (false, error),
        }
    }

    /// Admit one run and launch its monitor. The project stays admitted, through
    /// `activity`, until the monitor ends.
    pub(super) async fn start(
        &self,
        config: TrainingConfig,
        activity: ProjectActivity,
    ) -> Result<TrainingRun, OperationError> {
        if let Some(reason) = config.policy().mcap_training_blocker() {
            return Err(OperationError::Conflict(reason.into()));
        }
        let config = config.validated()?;
        let readiness = self.refresh_readiness().await;
        if !readiness.ready() {
            return Err(OperationError::Conflict(readiness.summary().to_owned()));
        }
        let (id, created_ms) = self
            .inner
            .run_ids
            .next(self.inner.clock.as_ref())
            .into_parts();
        let run = TrainingRun::starting(id, config.clone())
            .with_source(TrainingSource::new(config.dataset_id()))
            .with_started_ms(created_ms)
            .with_completion(None, Some(STARTING_MESSAGE.to_owned()));
        let admitted = run.clone();
        self.persist(|snapshot| {
            if let Some(active) = snapshot.active_run() {
                return Err(format!(
                    "training run {:?} is already active; stop it before starting another",
                    active.id()
                ));
            }
            snapshot.apply(TrainingUpdate::RunAdded {
                run: Box::new(admitted),
            });
            Ok(())
        })
        .await
        .map_err(OperationError::Conflict)?;
        self.monitor(run.id(), config, activity);
        Ok(run)
    }

    /// Ask the run's worker to stop. The monitor is signalled even when the
    /// Stopping phase cannot be persisted.
    pub(super) async fn stop(&self, run_id: &str) -> Result<(), OperationError> {
        let Some(stop) = self.inner.stops.recover_lock().get(run_id).cloned() else {
            return Err(OperationError::Conflict(format!(
                "training run {run_id:?} is not active"
            )));
        };
        let stopping = self
            .persist(|snapshot| {
                snapshot.enter_active_phase(
                    run_id,
                    TrainingRunPhase::Stopping,
                    Some(STOPPING_MESSAGE.to_owned()),
                )
            })
            .await;
        stop.store(true, Ordering::Release);
        stopping.map_err(|error| {
            self.fail_unpersisted(run_id, &error);
            OperationError::Conflict(error)
        })
    }

    /// Remove checkpoint rows before the run itself, so a failed checkpoint
    /// deletion leaves the complete run visible and retryable instead of
    /// hiding orphaned model artifacts.
    pub(super) async fn delete(
        &self,
        run_id: &str,
        active_rollout: Option<&str>,
    ) -> Result<(), OperationError> {
        let run_id = Self::admitted_deletion(&self.snapshot(), active_rollout, run_id)?;
        self.inner
            .storage
            .delete_run_checkpoints(run_id.clone())
            .await
            .map_err(OperationError::External)?;
        self.persist(|snapshot| {
            snapshot.apply(TrainingUpdate::RunRemoved { run_id });
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

    fn monitor(&self, run_id: &str, config: TrainingConfig, activity: ProjectActivity) {
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut stops = self.inner.stops.recover_lock();
            if stops.contains_key(run_id) {
                return;
            }
            stops.insert(run_id.to_owned(), stop.clone());
        }
        let monitor = RemoteJobMonitor {
            manager: self.clone(),
            run_id: run_id.to_owned(),
            config,
            stop,
            stop_sent: false,
        };
        tokio::spawn(async move {
            let _activity = activity;
            monitor.follow().await;
        });
    }

    /// Change the history in memory only; the next durable change writes it.
    async fn record(&self, update: TrainingUpdate) {
        let _write = self.inner.history.writes.lock().await;
        self.inner.history.current.recover_lock().apply(update);
    }

    /// Change the history, and write it before the change becomes visible.
    async fn persist(
        &self,
        change: impl FnOnce(&mut TrainingSnapshot) -> Result<(), String>,
    ) -> Result<(), String> {
        let _write = self.inner.history.writes.lock().await;
        let mut candidate = self.snapshot();
        change(&mut candidate)?;
        self.inner.storage.save_training_history(&candidate).await?;
        *self.inner.history.current.recover_lock() = candidate;
        Ok(())
    }

    /// The history can no longer be written, so stop the run and keep the
    /// failure visible in memory. Retrying the same failed write here would
    /// hide the failure behind recursion.
    fn fail_unpersisted(&self, run_id: &str, error: &str) {
        if let Some(stop) = self.inner.stops.recover_lock().get(run_id) {
            stop.store(true, Ordering::Release);
        }
        PersistenceFailure::new(run_id, self.inner.clock.now_ms(), error)
            .apply_to(&mut self.inner.history.current.recover_lock());
    }

    async fn readiness(&self) -> TrainingReadiness {
        let backend_probe = self.probe_backend();
        let spiral_probe = async {
            tokio::time::timeout(Duration::from_secs(8), self.inner.storage.test_spiral())
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
                self.inner.storage.project_id(),
                self.inner.storage.table_prefix(),
                probe = backend.display_summary(),
            ),
        };
        TrainingReadiness::new(ready, spiral_ready, summary).with_runtime(runtime)
    }

    async fn probe_backend(&self) -> Result<spiral_robotics_training::TrainingBackendInfo, String> {
        let backend = self.inner.backend.clone();
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
    stop: Arc<AtomicBool>,
    stop_sent: bool,
}

/// What the stored run already holds of its job's history. A reattached job
/// replays everything the worker retains; only what is newer is mirrored.
struct MirroredHistory {
    metric_step: u64,
    checkpoint_step: u64,
    stored_logs: BTreeSet<String>,
    configured: bool,
}

impl RemoteJobMonitor {
    /// Follow the job until it ends, mirroring its history into the run, then
    /// record how it ended.
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
        if let Err(error) = manager
            .persist(|snapshot| {
                snapshot.apply(ended);
                Ok(())
            })
            .await
        {
            manager.fail_unpersisted(&self.run_id, &error);
        }
        manager.inner.stops.recover_lock().remove(&self.run_id);
    }

    /// The job's terminal snapshot, or the step it reached and why it failed.
    async fn mirror_until_terminal(&mut self) -> Result<TrainingJobSnapshot, (u64, String)> {
        let mut snapshot = self.open().await.map_err(|error| (0, error))?;
        let stored = self.manager.run(&self.run_id).ok();
        let mut mirrored = MirroredHistory::new(stored.as_ref());
        loop {
            self.mirror(&snapshot, &mut mirrored)
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
        let (backend, run_id, request) = (
            self.manager.inner.backend.clone(),
            self.run_id.clone(),
            self.job_request(),
        );
        tokio::task::spawn_blocking(move || {
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
            let (backend, run_id) = (self.manager.inner.backend.clone(), self.run_id.clone());
            let polled = tokio::task::spawn_blocking(move || {
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

    /// Mirror what `snapshot` adds to the run: the resolved configuration once,
    /// the first Running phase, and newer metrics, checkpoints, and logs.
    async fn mirror(
        &self,
        snapshot: &TrainingJobSnapshot,
        mirrored: &mut MirroredHistory,
    ) -> Result<(), String> {
        let (manager, run_id) = (&self.manager, self.run_id.as_str());
        if snapshot.job_id() != run_id {
            return Err("training runtime returned the wrong job ID".to_owned());
        }
        if !mirrored.configured
            && let Some(resolved) = snapshot.resolved()
        {
            let mut config = self.config.clone();
            config
                .act_mut()
                .set_camera_streams(resolved.camera_streams().to_vec());
            manager
                .persist(|history| {
                    history.apply(TrainingUpdate::RunConfigured {
                        run_id: run_id.to_owned(),
                        config: Box::new(config),
                    });
                    Ok(())
                })
                .await?;
            mirrored.configured = true;
        }
        if snapshot.phase() == TrainingJobPhase::Running
            && manager
                .run(run_id)
                .is_ok_and(|run| run.phase() == TrainingRunPhase::Starting)
        {
            manager
                .persist(|history| {
                    history.enter_active_phase(
                        run_id,
                        TrainingRunPhase::Running,
                        snapshot.message().map(str::to_owned),
                    )
                })
                .await?;
        }
        if let Some(gap) = snapshot.history_gap() {
            self.log(format!("Training worker history was compacted: {gap}"))
                .await;
        }
        let metric_floor = mirrored.metric_step;
        for metric in snapshot
            .metrics()
            .iter()
            .filter(|metric| metric.step() > metric_floor)
        {
            manager
                .record(TrainingUpdate::Metric {
                    run_id: run_id.to_owned(),
                    point: Box::new(
                        TrainingMetricPoint::new(metric.step(), metric.train_loss())
                            .with_learning_rate(metric.learning_rate())
                            .with_validation_loss(metric.validation_loss())
                            .with_timeline(
                                metric.elapsed_seconds(),
                                metric.event().map(str::to_owned),
                            )
                            .with_throughput(0.0, metric.examples_per_second())
                            .with_utilization(metric.cpu_percent(), metric.gpu_percent())
                            .with_memory_bytes(metric.memory_bytes())
                            .with_loader_ms(metric.loader_ms())
                            .with_runtime_timings(metric.device_copy_ms(), metric.model_ms()),
                    ),
                })
                .await;
            mirrored.metric_step = metric.step();
        }
        let checkpoint_floor = mirrored.checkpoint_step;
        for checkpoint in snapshot
            .checkpoints()
            .iter()
            .filter(|checkpoint| checkpoint.step() > checkpoint_floor)
        {
            let checkpoint_update = TrainingUpdate::Checkpoint {
                run_id: run_id.to_owned(),
                checkpoint: TrainingCheckpoint::new(checkpoint.step(), checkpoint.created_ms())
                    .with_validation_loss(checkpoint.validation_loss())
                    .with_artifact(checkpoint.byte_size(), true)
                    .with_rollout(
                        checkpoint.recommended_action_steps(),
                        checkpoint.sample_hz(),
                    ),
            };
            manager
                .persist(|history| {
                    history.apply(checkpoint_update);
                    Ok(())
                })
                .await?;
            mirrored.checkpoint_step = checkpoint.step();
        }
        for log in snapshot.logs() {
            if !mirrored.stored_logs.contains(log.line()) {
                self.log(log.line().to_owned()).await;
            }
        }
        // Only the first, full replay can repeat what was already stored.
        mirrored.stored_logs.clear();
        Ok(())
    }

    async fn log(&self, line: String) {
        self.manager
            .record(TrainingUpdate::Log {
                run_id: self.run_id.clone(),
                line,
            })
            .await;
    }

    fn job_request(&self) -> TrainingJobRequest {
        let (storage, config) = (&self.manager.inner.storage, &self.config);
        let credentials = storage.spiral_credentials();
        let spiral = SpiralJobConnection::new(
            SpiralServiceEndpoints::new(storage.spiral_endpoint(), storage.spiral_spfs_endpoint()),
            storage.project_id(),
            storage.table_prefix(),
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
    fn new(stored: Option<&TrainingRun>) -> Self {
        Self {
            metric_step: stored
                .and_then(|run| run.metrics().iter().map(|metric| metric.step()).max())
                .unwrap_or(0),
            checkpoint_step: stored
                .and_then(|run| {
                    run.checkpoints()
                        .iter()
                        .map(|checkpoint| checkpoint.step())
                        .max()
                })
                .unwrap_or(0),
            stored_logs: stored
                .map(|run| run.logs().iter().cloned().collect())
                .unwrap_or_default(),
            configured: false,
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

    fn apply_to(self, snapshot: &mut TrainingSnapshot) {
        let Some(current_step) = snapshot
            .runs()
            .iter()
            .find(|run| run.id() == self.run_id)
            .map(TrainingRun::current_step)
        else {
            return;
        };
        let message = format!(
            "Training history could not be persisted, so execution was stopped: {}",
            self.error
        );
        snapshot.apply(TrainingUpdate::Log {
            run_id: self.run_id.to_owned(),
            line: message.clone(),
        });
        snapshot.apply(TrainingUpdate::RunState {
            run_id: self.run_id.to_owned(),
            phase: TrainingRunPhase::Failed,
            current_step,
            finished_ms: Some(self.finished_ms),
            message: Some(message),
        });
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

    #[test]
    fn runtime_persistence_failure_becomes_a_visible_failed_run_and_log() {
        let active = run("active", TrainingRunPhase::Running, 12);
        let mut snapshot = TrainingSnapshot::new(TrainingReadiness::default(), vec![active]);

        PersistenceFailure::new("active", 99, "Spiral unavailable").apply_to(&mut snapshot);

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
