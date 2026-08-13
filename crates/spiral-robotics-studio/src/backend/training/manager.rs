//! Rust-owned supervision for Spiral ADBC and Burn-native vision ACT.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc as std_mpsc};
use std::time::{Duration, Instant};

use adbc_driver_manager::ManagedConnection;
use spiral_robotics_contracts::api::{
    ConnectionTestResponse, MAX_TRAINING_HISTORY_RUNS, StartTrainingRequest, TrainingCheckpoint,
    TrainingMetricPoint, TrainingReadiness, TrainingRun, TrainingRunPhase, TrainingSnapshot,
    TrainingSource, TrainingUpdate,
};
use spiral_robotics_contracts::spiral;
use spiral_robotics_inference::act::{
    NativeActTrainer, VisionActCheckpoint, VisionActTrainingConfig,
};
use tokio::sync::{broadcast, mpsc};

use crate::backend::adapters::clock::SystemClock;
use crate::backend::application::checkpoints::{
    CheckpointPublication, CheckpointRepository, CheckpointSource,
    validate_checkpoint_artifact_size,
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
    self, ResolvedTrainingSource, RunStateChange, RuntimeEventWindow, TrainingCoordinatorPort,
    TrainingLaunch, TrainingRunIdentity, TrainingRunPort, TrainingRuntimeEvent,
    TrainingRuntimeOutcome, TrainingRuntimePort, TrainingRuntimeSession,
};
use crate::backend::application::training_population::NormalizationStats;
use crate::backend::application::training_runs::{
    RunPhase as ApplicationRunPhase, RunRecord, TrainingRunRegistry,
};
use crate::backend::storage::manager::StorageManager;
use crate::backend::storage::settings::StoredSpiralSettings;
use crate::backend::training::data::{LoadedActBatch, SpiralActLoader};
use crate::backend::training::resources::ResourceSampler;

const HISTORY_FILE: &str = "runs.json";
const STOP_GRACE: Duration = Duration::from_secs(30);
const PREFETCH_BUFFER_SIZE: usize = 1;
const LOADER_STALL: Duration = Duration::from_secs(2);
static RUN_IDS: StableIdSequence = StableIdSequence::new("train", 0);

#[derive(Clone)]
pub(crate) struct TrainingManager {
    inner: Arc<TrainingManagerInner>,
}

struct TrainingManagerInner {
    root: PathBuf,
    storage: Arc<StorageManager>,
    ffmpeg: String,
    clock: Arc<dyn Clock>,
    snapshot: Mutex<TrainingSnapshot>,
    updates: broadcast::Sender<TrainingUpdate>,
    controls: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
}

enum NativeEvent {
    Configured(Box<StartTrainingRequest>),
    Started(String),
    Metric(TrainingMetricPoint),
    Checkpoint(NativeCheckpoint),
    Log(String),
}

pub(crate) struct NativeCheckpoint {
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
    pub(crate) async fn open(
        root: PathBuf,
        storage: Arc<StorageManager>,
        ffmpeg: String,
    ) -> Result<Self, String> {
        Self::open_with_clock(root, storage, ffmpeg, Arc::new(SystemClock)).await
    }

    pub(crate) async fn open_with_clock(
        root: PathBuf,
        storage: Arc<StorageManager>,
        ffmpeg: String,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, String> {
        fs::create_dir_all(&root)
            .map_err(|error| format!("could not create {}: {error}", root.display()))?;
        let root = fs::canonicalize(&root)
            .map_err(|error| format!("could not resolve {}: {error}", root.display()))?;
        let mut snapshot = load_snapshot(&root)?;
        let mut run_registry = registry(&snapshot)?;
        let recovered = run_registry.recover_after_restart();
        let retained = run_registry
            .retained_ids()
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>();
        snapshot.runs.retain(|run| retained.contains(&run.id));
        for transition in recovered {
            let run = snapshot
                .runs
                .iter_mut()
                .find(|run| run.id == transition.id)
                .expect("the registry was built from this snapshot");
            run.phase = TrainingRunPhase::Failed;
            run.finished_ms = Some(clock.now_ms());
            run.message = Some("The backend restarted while this run was active.".to_owned());
        }
        for run in &snapshot.runs {
            // Older history entries predate immutable training sources and are
            // intentionally not rollout-ready. New entries restore their exact
            // project and namespace before RolloutManager is constructed.
            if run.source != TrainingSource::default() {
                storage
                    .register_run_source(&run.id, checkpoint_source(&run.source))
                    .map_err(|error| format!("restoring training run {:?}: {error}", run.id))?;
            }
        }
        snapshot.readiness = readiness(&storage).await;
        persist_snapshot(&root, &snapshot)?;
        let (updates, _) = broadcast::channel(512);
        let manager = Self {
            inner: Arc::new(TrainingManagerInner {
                root,
                storage,
                ffmpeg,
                clock,
                snapshot: Mutex::new(snapshot),
                updates,
                controls: Mutex::new(BTreeMap::new()),
            }),
        };
        Ok(manager)
    }

    pub(crate) fn snapshot(&self) -> TrainingSnapshot {
        lock(&self.inner.snapshot).clone()
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
        let next = readiness(&self.inner.storage).await;
        if lock(&self.inner.snapshot).readiness != next {
            let _ = self.publish(
                TrainingUpdate::Readiness {
                    readiness: next.clone(),
                },
                false,
            );
        }
        next
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

    pub(crate) async fn shutdown(&self) {
        let controls = std::mem::take(&mut *lock(&self.inner.controls));
        for (_, cancel) in controls {
            cancel.store(true, Ordering::Release);
        }
        let _ = tokio::time::timeout(STOP_GRACE + Duration::from_secs(2), async {
            while lock(&self.inner.snapshot).active_run().is_some() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
    }

    async fn run_native(
        &self,
        launch: TrainingLaunch<StoredSpiralSettings>,
        cancel: Arc<AtomicBool>,
    ) {
        let run_id = launch.run_id;
        let request = contract_training_request(launch.command.clone());
        let source = contract_training_source(launch.source.clone());
        let settings = launch.source_handle;
        let mut session =
            TrainingRuntimeSession::new(run_id.clone(), launch.command, launch.source);
        let pretrained = if request.act.pretrained_backbone {
            match tokio::task::spawn_blocking(load_pretrained_backbone).await {
                Ok(Ok(bytes)) => Some(bytes),
                Ok(Err(error)) => {
                    training_execution::complete_runtime(self, session, Err(error));
                    return;
                }
                Err(error) => {
                    training_execution::complete_runtime(
                        self,
                        session,
                        Err(format!("pretrained backbone task failed: {error}")),
                    );
                    return;
                }
            }
        } else {
            None
        };

        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let training_request = request.clone();
        let training_source = source.clone();
        let training_run_id = run_id.clone();
        let training_cancel = cancel.clone();
        let training_clock = self.inner.clock.clone();
        let ffmpeg = self.inner.ffmpeg.clone();
        let storage = self.inner.storage.clone();
        let training_settings = settings.clone();
        let mut training = Box::pin(storage.run_training_adbc(
            settings.clone(),
            move |connection| {
                train_blocking(
                    connection,
                    &training_run_id,
                    &training_request,
                    &training_settings,
                    &training_source,
                    &ffmpeg,
                    training_clock,
                    training_cancel,
                    pretrained.as_deref(),
                    events_tx,
                )
            },
        ));
        let mut events_open = true;
        let terminal = loop {
            tokio::select! {
                result = &mut training => break result,
                event = events_rx.recv(), if events_open => {
                    match event {
                        None => events_open = false,
                        Some(event) => training_execution::accept_runtime_event(
                            self,
                            &mut session,
                            application_runtime_event(event),
                            RuntimeEventWindow::Live,
                        ).await,
                    }
                }
            }
        };
        while let Ok(event) = events_rx.try_recv() {
            training_execution::accept_runtime_event(
                self,
                &mut session,
                application_runtime_event(event),
                RuntimeEventWindow::Draining,
            )
            .await;
        }
        training_execution::complete_runtime(self, session, terminal);
    }

    #[allow(clippy::too_many_arguments)]
    async fn publish_checkpoint_bytes(
        &self,
        run_id: &str,
        request: &StartTrainingRequest,
        source: &TrainingSource,
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
            .publish_checkpoint(CheckpointPublication {
                source: checkpoint_source(source),
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
        self.publish(
            TrainingUpdate::Checkpoint {
                run_id: run_id.to_owned(),
                checkpoint: TrainingCheckpoint {
                    step,
                    created_ms,
                    validation_loss,
                    spiral_table: source.checkpoint_table.clone(),
                    byte_size,
                    inference_ready: true,
                    recommended_action_steps: request.chunk_size,
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
        persist_snapshot(&self.inner.root, &candidate)?;
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
        let _ = self.publish(
            TrainingUpdate::Log {
                run_id: run_id.to_owned(),
                line,
            },
            false,
        );
    }

    fn publish(&self, update: TrainingUpdate, persist: bool) -> Result<(), String> {
        let mut snapshot = lock(&self.inner.snapshot);
        let mut candidate = snapshot.clone();
        candidate.apply(update.clone());
        if persist {
            persist_snapshot(&self.inner.root, &candidate)?;
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
    type SourceHandle = StoredSpiralSettings;

    async fn refresh_training_readiness(&self) -> ApplicationTrainingReadiness {
        application_training_readiness(self.refresh_readiness().await)
    }

    fn resolve_training_source(
        &self,
        dataset_id: &str,
    ) -> Result<ResolvedTrainingSource<Self::SourceHandle>, ApplicationError> {
        let (handle, source) = self
            .inner
            .storage
            .training_source(dataset_id)
            .map_err(ApplicationError::conflict)?;
        Ok(ResolvedTrainingSource {
            source: application_training_source(source),
            handle,
        })
    }

    async fn validate_training_query(
        &self,
        handle: &Self::SourceHandle,
        command: &ApplicationStartTrainingCommand,
    ) -> Result<(), ApplicationError> {
        self.inner
            .storage
            .validate_training_query(
                handle.clone(),
                &command.query,
                &command.state_column,
                &command.action_column,
            )
            .await
            .map_err(ApplicationError::conflict)
    }

    fn next_training_identity(&self) -> TrainingRunIdentity {
        let identity = RUN_IDS.next(self.inner.clock.as_ref());
        TrainingRunIdentity {
            id: identity.value,
            created_ms: identity.created_ms,
        }
    }

    fn register_training_source(
        &self,
        run_id: &str,
        source: &ApplicationTrainingSource,
    ) -> Result<(), ApplicationError> {
        self.inner
            .storage
            .register_run_source(
                run_id,
                checkpoint_source(&contract_training_source(source.clone())),
            )
            .map_err(ApplicationError::conflict)
    }

    fn unregister_training_source(&self, run_id: &str, source: &ApplicationTrainingSource) {
        self.inner.storage.unregister_run_source(
            run_id,
            &checkpoint_source(&contract_training_source(source.clone())),
        );
    }

    fn admit_training_run(&self, run: ApplicationTrainingRun) -> Result<(), ApplicationError> {
        // Registry admission, durable snapshot replacement, and publication
        // share one lock, so concurrent starts cannot both become active.
        let mut snapshot = lock(&self.inner.snapshot);
        let (candidate, update) =
            admitted_snapshot(&self.inner.root, &snapshot, contract_training_run(run))
                .map_err(ApplicationError::conflict)?;
        *snapshot = candidate;
        let _ = self.inner.updates.send(update);
        Ok(())
    }

    fn launch_training_runtime(
        &self,
        launch: TrainingLaunch<Self::SourceHandle>,
    ) -> Result<(), ApplicationError> {
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
            manager.run_native(launch, cancel).await;
        });
        Ok(())
    }
}

impl TrainingRuntimePort for TrainingManager {
    type CheckpointEnvelope = NativeCheckpoint;

    fn run_phase(&self, run_id: &str) -> Option<ApplicationTrainingRunPhase> {
        self.phase(run_id).map(application_training_phase)
    }

    fn persist_resolved_configuration(
        &self,
        run_id: &str,
        command: &ApplicationStartTrainingCommand,
    ) -> Result<(), String> {
        self.publish(
            TrainingUpdate::RunConfigured {
                run_id: run_id.to_owned(),
                config: Box::new(contract_training_request(command.clone())),
            },
            true,
        )
    }

    fn record_metric(&self, run_id: &str, metric: ApplicationTrainingMetric) {
        let _ = self.publish(
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

    async fn publish_runtime_checkpoint(
        &self,
        run_id: &str,
        command: &ApplicationStartTrainingCommand,
        source: &ApplicationTrainingSource,
        checkpoint: Self::CheckpointEnvelope,
    ) -> Result<(), String> {
        let NativeCheckpoint {
            step,
            created_ms,
            validation_loss,
            manifest,
            payload,
            acknowledged,
        } = checkpoint;
        let result = self
            .publish_checkpoint_bytes(
                run_id,
                &contract_training_request(command.clone()),
                &contract_training_source(source.clone()),
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
    connection: &mut ManagedConnection,
    run_id: &str,
    request: &StartTrainingRequest,
    settings: &StoredSpiralSettings,
    source: &TrainingSource,
    ffmpeg: &str,
    clock: Arc<dyn Clock>,
    cancel: Arc<AtomicBool>,
    pretrained: Option<&[u8]>,
    events: mpsc::UnboundedSender<NativeEvent>,
) -> Result<TrainingRuntimeOutcome, String> {
    let _ = events.send(NativeEvent::Log(
        "Opening the dedicated Spiral ADBC connection and indexing eligible ACT anchors…"
            .to_owned(),
    ));
    let mut loader = SpiralActLoader::open(connection, request, &settings.dataset, ffmpeg)?;
    let mut resolved_request = request.clone();
    resolved_request.act.camera_streams = loader.camera_streams().to_vec();
    let _ = events.send(NativeEvent::Configured(Box::new(resolved_request.clone())));
    let config = VisionActTrainingConfig {
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
    let mut trainer = NativeActTrainer::new(config, &request.device, pretrained)
        .map_err(|error| error.to_string())?;
    let _ = events.send(NativeEvent::Started(format!(
        "Vision ACT is running on Burn/{}: {}; {:.1}M parameters.",
        trainer.backend_name(),
        loader.summary(),
        trainer.parameter_count() as f64 / 1_000_000.0
    )));
    let _ = events.send(NativeEvent::Log(format!(
        "ACT inputs: state={}, action={}, image={}x{}, chunk={}, cameras={}. Spiral owns the normal decode path; Studio recovers unusable managed indexes from the original MP4 with equivalent resize and ImageNet normalization.",
        loader.state_dimensions(),
        loader.action_dimensions(),
        loader.image_width(),
        loader.image_height(),
        request.chunk_size,
        resolved_request.act.camera_streams.join(", ")
    )));
    let _ = events.send(NativeEvent::Log(
        "ACT input prefetch is active with one completed batch buffered while Metal trains."
            .to_owned(),
    ));

    let data_contract = ActDataContract::from_loader(&loader);
    let has_validation = loader.has_validation();
    let validation_every = request.checkpoint_every.saturating_div(4).clamp(25, 250);
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
            source,
            clock.as_ref(),
            cancel.as_ref(),
            &events,
            &data_contract,
            &mut trainer,
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

#[allow(clippy::too_many_arguments)]
fn train_prefetched(
    receiver: std_mpsc::Receiver<PrefetchMessage>,
    run_id: &str,
    request: &StartTrainingRequest,
    resolved_request: &StartTrainingRequest,
    source: &TrainingSource,
    clock: &dyn Clock,
    cancel: &AtomicBool,
    events: &mpsc::UnboundedSender<NativeEvent>,
    data_contract: &ActDataContract,
    trainer: &mut NativeActTrainer,
    validation_every: u64,
    has_validation: bool,
) -> Result<TrainingRuntimeOutcome, String> {
    let started = Instant::now();
    let mut metric_started = started;
    let mut examples_since_metric = 0_usize;
    let mut resources = ResourceSampler::new(trainer.backend_name() == "Metal");
    let mut last_validation = None;
    let mut last_checkpoint = 0;
    let mut completed_steps = 0;
    for step in 1..=request.steps {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let loader_wait_started = Instant::now();
        let Some(loaded) =
            receive_prefetched_batch(&receiver, step, PrefetchedBatchKind::Training)?
        else {
            break;
        };
        let loader_wait = loader_wait_started.elapsed();
        let loader_metrics = loaded.metrics;
        let loss = trainer
            .train_step(loaded.batch, step)
            .map_err(|error| error.to_string())?;
        completed_steps = step;
        examples_since_metric += request.batch_size;
        let mut event = Vec::new();
        let validation_loss = if has_validation && step % validation_every == 0 {
            let batch = receive_prefetched_batch(&receiver, step, PrefetchedBatchKind::Validation)?
                .ok_or_else(|| {
                    format!("ACT input prefetch stopped before validation at step {step}")
                })?;
            let loss = trainer
                .validation_step(batch.batch)
                .map_err(|error| error.to_string())?;
            last_validation = Some(loss.total);
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
        let usage = resources.sample();
        let _ = events.send(NativeEvent::Metric(TrainingMetricPoint {
            step,
            train_loss: loss.total,
            validation_loss,
            learning_rate: trainer.learning_rate(step),
            transfer_bytes_per_second: loader_metrics.source_bytes as f64
                / loader_metrics.seconds.max(1.0e-9),
            examples_per_second: examples_since_metric as f64 / elapsed,
            cpu_percent: usage.cpu_percent,
            gpu_percent: usage.gpu_percent,
            memory_bytes: usage.memory_bytes,
            elapsed_seconds: now.duration_since(started).as_secs_f64(),
            event: (!event.is_empty()).then(|| event.join(" · ")),
        }));
        if step == 1 {
            let _ = events.send(NativeEvent::Log(format!(
                "Loss components at step 1: L1={:.4}, KL={:.4}, pad={:.4}; decoded {} frame(s).",
                loss.l1, loss.kl, loss.pad, loader_metrics.requested_frames
            )));
        }
        metric_started = now;
        examples_since_metric = 0;
        if step % request.checkpoint_every == 0 {
            send_checkpoint(
                events,
                run_id,
                resolved_request,
                source,
                data_contract,
                trainer,
                step,
                last_validation,
                clock,
            )?;
            last_checkpoint = step;
        }
    }
    if completed_steps > 0 && last_checkpoint != completed_steps {
        send_checkpoint(
            events,
            run_id,
            resolved_request,
            source,
            data_contract,
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
    events: &mpsc::UnboundedSender<NativeEvent>,
    run_id: &str,
    request: &StartTrainingRequest,
    source: &TrainingSource,
    data_contract: &ActDataContract,
    trainer: &NativeActTrainer,
    step: u64,
    validation_loss: Option<f64>,
    clock: &dyn Clock,
) -> Result<(), String> {
    let manifest = checkpoint_manifest(run_id, request, source, data_contract, step);
    let manifest_json = serde_json::to_string(&manifest)
        .map_err(|error| format!("encoding ACT checkpoint manifest: {error}"))?;
    let payload = trainer
        .checkpoint(VisionActCheckpoint {
            manifest_json: &manifest_json,
            state_mean: &data_contract.stats.state_mean,
            state_std: &data_contract.stats.state_std,
            action_mean: &data_contract.stats.action_mean,
            action_std: &data_contract.stats.action_std,
        })
        .map_err(|error| error.to_string())?;
    let (acknowledged, result) = std::sync::mpsc::sync_channel(1);
    events
        .send(NativeEvent::Checkpoint(NativeCheckpoint {
            step,
            created_ms: clock.now_ms(),
            validation_loss,
            manifest,
            payload,
            acknowledged,
        }))
        .map_err(|_| "training event channel closed before checkpoint publication".to_owned())?;
    result
        .recv()
        .map_err(|_| "checkpoint publisher stopped before acknowledging the artifact".to_owned())?
}

fn checkpoint_manifest(
    run_id: &str,
    request: &StartTrainingRequest,
    source: &TrainingSource,
    data_contract: &ActDataContract,
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
        "n_action_steps": request.chunk_size,
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
        "source": {
            "project": source.project,
            "dataset": source.dataset,
        },
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
            "ml": "burn 0.21",
            "transport": "adbc 0.24 / arrow 58.4",
            "language": "rust",
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

fn load_pretrained_backbone() -> Result<Vec<u8>, String> {
    spiral_robotics_inference::act::imagenet_resnet18_weights()
        .map_err(|error| format!("loading torchvision ResNet18 IMAGENET1K_V1: {error}"))
}

fn runtime_description() -> String {
    #[cfg(target_os = "macos")]
    let acceleration = if spiral_robotics_inference::act::metal_available() {
        "Metal + CPU"
    } else {
        "CPU"
    };
    #[cfg(not(target_os = "macos"))]
    let acceleration = "CPU";
    format!("Burn 0.21 · {acceleration} · ADBC Arrow tensors verified at run start")
}

fn checkpoint_source(source: &TrainingSource) -> CheckpointSource {
    CheckpointSource {
        project: source.project.clone(),
        namespace: source.dataset.clone(),
        dataset_id: source.dataset_id.clone(),
    }
}

async fn readiness(storage: &StorageManager) -> TrainingReadiness {
    let spiral = storage.training_source("default");
    let connection = if spiral.is_ok() {
        Some(
            tokio::time::timeout(Duration::from_secs(8), storage.test_spiral())
                .await
                .unwrap_or_else(|_| ConnectionTestResponse {
                    ok: false,
                    message: "the connection probe timed out after 8 seconds".to_owned(),
                }),
        )
    } else {
        None
    };
    let spiral_ready = connection.as_ref().is_some_and(|result| result.ok);
    let ready = spiral_ready;
    let summary = match (spiral, connection) {
        (Err(error), _) => error,
        (_, Some(result)) if !result.ok => {
            format!("Spiral is not ready for training: {}", result.message)
        }
        (Ok((_, source)), Some(_)) => format!(
            "Runtime ready · {probe} · Spiral project {} · namespace {} · query checked at run start",
            source.project,
            source.dataset,
            probe = runtime_description(),
        ),
        _ => "Spiral readiness could not be determined.".to_owned(),
    };
    TrainingReadiness {
        ready,
        spiral_ready,
        summary,
    }
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
        return Err("the native trainer must publish a .safetensors checkpoint".to_owned());
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

fn application_training_source(source: TrainingSource) -> ApplicationTrainingSource {
    ApplicationTrainingSource {
        project: source.project,
        dataset: source.dataset,
        dataset_id: source.dataset_id,
    }
}

fn contract_training_source(source: ApplicationTrainingSource) -> TrainingSource {
    let checkpoint_table = spiral::relation(&source.dataset, spiral::Table::TrainingCheckpoints);
    TrainingSource {
        project: source.project,
        dataset: source.dataset,
        dataset_id: source.dataset_id,
        checkpoint_table,
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
        elapsed_seconds: metric.elapsed_seconds,
        event: metric.event,
    }
}

fn contract_training_checkpoint(
    checkpoint: ApplicationTrainingCheckpoint,
    spiral_table: &str,
) -> TrainingCheckpoint {
    TrainingCheckpoint {
        step: checkpoint.step,
        created_ms: checkpoint.created_ms,
        validation_loss: checkpoint.validation_loss,
        spiral_table: spiral_table.to_owned(),
        byte_size: checkpoint.byte_size,
        inference_ready: checkpoint.inference_ready,
        recommended_action_steps: checkpoint.recommended_action_steps,
        sample_hz: checkpoint.sample_hz,
    }
}

fn contract_training_run(run: ApplicationTrainingRun) -> TrainingRun {
    let checkpoint_table =
        spiral::relation(&run.source.dataset, spiral::Table::TrainingCheckpoints);
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
            .map(|checkpoint| contract_training_checkpoint(checkpoint, &checkpoint_table))
            .collect(),
        logs: run.logs,
    }
}

fn application_runtime_event(event: NativeEvent) -> TrainingRuntimeEvent<NativeCheckpoint> {
    match event {
        NativeEvent::Configured(command) => {
            TrainingRuntimeEvent::Configured(Box::new(application_training_command(*command)))
        }
        NativeEvent::Started(message) => TrainingRuntimeEvent::Started(message),
        NativeEvent::Metric(metric) => {
            TrainingRuntimeEvent::Metric(application_training_metric(metric))
        }
        NativeEvent::Checkpoint(checkpoint) => TrainingRuntimeEvent::Checkpoint(checkpoint),
        NativeEvent::Log(message) => TrainingRuntimeEvent::Log(message),
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

fn load_snapshot(root: &Path) -> Result<TrainingSnapshot, String> {
    let path = root.join(HISTORY_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TrainingSnapshot::default());
        }
        Err(error) => {
            return Err(format!(
                "could not read training history {}: {error}",
                path.display()
            ));
        }
    };
    serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "training history {} is malformed and was left untouched: {error}",
            path.display()
        )
    })
}

fn persist_snapshot(root: &Path, snapshot: &TrainingSnapshot) -> Result<(), String> {
    let path = root.join(HISTORY_FILE);
    let temporary = root.join("runs.json.tmp");
    let mut bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|error| format!("could not encode training history: {error}"))?;
    bytes.push(b'\n');

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| {
            format!(
                "could not create temporary training history {}: {error}",
                temporary.display()
            )
        })?;
    file.write_all(&bytes).map_err(|error| {
        format!(
            "could not write temporary training history {}: {error}",
            temporary.display()
        )
    })?;
    file.sync_all().map_err(|error| {
        format!(
            "could not sync temporary training history {}: {error}",
            temporary.display()
        )
    })?;
    drop(file);
    fs::rename(&temporary, &path).map_err(|error| {
        format!(
            "could not publish training history {}: {error}",
            path.display()
        )
    })?;
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "could not sync training history directory {}: {error}",
                root.display()
            )
        })
}

fn admitted_snapshot(
    root: &Path,
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
    persist_snapshot(root, &candidate)?;
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

    fn loaded_batch(marker: f32) -> LoadedActBatch {
        LoadedActBatch {
            batch: spiral_robotics_inference::act::VisionActBatch {
                batch_size: 1,
                states: vec![marker],
                images: vec![marker],
                actions: vec![marker],
                action_is_pad: vec![false],
            },
            metrics: crate::backend::training::data::LoaderMetrics {
                seconds: f64::from(marker),
                source_bytes: 4,
                requested_frames: 1,
            },
        }
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
    fn missing_history_is_empty_but_malformed_history_is_preserved_and_reported() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            load_snapshot(directory.path()).unwrap(),
            TrainingSnapshot::default()
        );

        let path = directory.path().join(HISTORY_FILE);
        let malformed = b"{ definitely not a training snapshot";
        fs::write(&path, malformed).unwrap();
        let error = load_snapshot(directory.path()).unwrap_err();
        assert!(error.contains("malformed"), "{error}");
        assert_eq!(fs::read(path).unwrap(), malformed);
    }

    #[test]
    fn history_publication_is_atomic_and_syncs_through_a_same_directory_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = TrainingSnapshot {
            runs: vec![run("persisted")],
            ..TrainingSnapshot::default()
        };

        persist_snapshot(directory.path(), &snapshot).unwrap();

        assert_eq!(load_snapshot(directory.path()).unwrap(), snapshot);
        assert!(!directory.path().join("runs.json.tmp").exists());
    }

    #[test]
    fn failed_history_write_preserves_the_previous_file_and_does_not_admit_the_run() {
        let directory = tempfile::tempdir().unwrap();
        let original = TrainingSnapshot::default();
        persist_snapshot(directory.path(), &original).unwrap();
        let original_bytes = fs::read(directory.path().join(HISTORY_FILE)).unwrap();
        fs::create_dir(directory.path().join("runs.json.tmp")).unwrap();

        let error = admitted_snapshot(directory.path(), &original, run("not-admitted"))
            .expect_err("a blocked temporary path must fail publication");

        assert!(error.contains("temporary training history"), "{error}");
        assert_eq!(original, TrainingSnapshot::default());
        assert_eq!(
            fs::read(directory.path().join(HISTORY_FILE)).unwrap(),
            original_bytes
        );
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
            persistence_failure_updates(&snapshot, "active", 99, "disk full").unwrap();
        snapshot.apply(log);
        snapshot.apply(state);

        let failed = &snapshot.runs[0];
        assert_eq!(failed.phase, TrainingRunPhase::Failed);
        assert_eq!(failed.current_step, 12);
        assert_eq!(failed.finished_ms, Some(99));
        assert!(failed.message.as_deref().unwrap().contains("disk full"));
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
    fn checkpoint_publication_accepts_only_matching_safetensors_artifacts() {
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
