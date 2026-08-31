//! Backend-owned remote policy lifecycle and deadline-aware motor loop.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::FutureExt as _;
use image::imageops::FilterType;
use safetensors::SafeTensors;
use serde::Deserialize;
use spiral_robotics_api::rollout::{RolloutConfig, RolloutPhase, RolloutSnapshot};
use spiral_robotics_inference::{
    ActionChunk, ImageDimensions, ImageFrame, InferenceInput, Policy, PolicyLoader,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant as TokioInstant, MissedTickBehavior};

use crate::adapters::clock::SystemClock;
use crate::adapters::compute_runtime::HttpPolicyLoader;
use crate::adapters::hardware::HardwareApiProjection as _;
use crate::clock::{Clock, StableIdSequence};
use crate::robotics::cameras::{CameraManager, Frame};
use crate::robots::workspace::RobotWorkspace;
use crate::rollout::policy::{
    ArtifactIdentity, CameraBindingContract, ControlOwner, ExpectedPolicyOwner, JointObservation,
    JointObservations, ObservationFreshness, ObservationFreshnessCollection, PolicyDataContract,
    PolicyImageContract, PolicyModelContract, PolicyObservationContract, PolicySchemaContract,
    PreparedActionGoals, TargetRobotContract,
};
use crate::rollout::service::{
    CleanupResult, CompletionReason, ControlDecision, ControlObservation, ExecutionOutcome,
    InferenceTiming, PolicyCadence, RolloutAdmission, RolloutCoordinator, RolloutEventPort,
};
use crate::sync::MutexRecovery;
use spiral_robotics_hardware::{
    HardwareHandle, HardwareState, ModeSelection, PolicyGoals, RobotTelemetry,
};
use spiral_robotics_storage::checkpoints::{CheckpointArtifact, CheckpointKey, CheckpointStore};
use spiral_robotics_storage::{StorageManager, StudioPreferencesStore};

const INPUT_STALE_AFTER: Duration = Duration::from_millis(350);
const INITIAL_CAMERA_TIMEOUT: Duration = Duration::from_secs(5);
/// LeRobot's asynchronous ACT client blends an existing prediction with a
/// newer prediction for the same absolute timestep instead of replacing the
/// action queue. Its default weighted-average strategy keeps 30% of the old
/// prediction and accepts 70% of the new one.
const ACTION_BLEND_OLD_WEIGHT: f32 = 0.3;
const ACTION_BLEND_NEW_WEIGHT: f32 = 0.7;
/// LeRobot filters observations whose raw joint-state L2 distance from the
/// last processed observation is below one. For an SO-101 those raw units are
/// degrees for the five arm joints and a 0..100 position for the gripper.
const OBSERVATION_SIMILARITY_TOLERANCE: f32 = 1.0;
#[derive(Debug, Deserialize)]
struct ArtifactManifest {
    format: String,
    artifact: String,
    run_id: String,
    step: u64,
    data_contract: ArtifactDataContract,
    model_config: serde_json::Value,
    tensor_namespace: String,
}

#[derive(Debug, Deserialize)]
struct ArtifactDataContract {
    #[serde(flatten)]
    observations: ArtifactObservationContract,
    #[serde(flatten)]
    images: ArtifactImageContract,
    spiral_schema_version: u32,
}

#[derive(Debug, Deserialize)]
struct ArtifactObservationContract {
    value_space: String,
    robot_type: String,
    joint_names: Vec<String>,
    camera_streams: Vec<String>,
    sample_hz: f64,
}

#[derive(Debug, Deserialize)]
struct ArtifactImageContract {
    image_layout: String,
    image_normalization: String,
    frame_alignment: String,
}

impl ArtifactDataContract {
    fn into_policy_contract(self, supported_schema_version: u32) -> PolicyDataContract {
        PolicyDataContract::new(
            PolicyObservationContract::new(
                self.observations.value_space,
                self.observations.robot_type,
                self.observations.joint_names,
            )
            .with_capture(
                self.observations.camera_streams,
                self.observations.sample_hz,
            ),
            PolicyImageContract::new(
                self.images.image_layout,
                self.images.image_normalization,
                self.images.frame_alignment,
            ),
            PolicySchemaContract::new(self.spiral_schema_version, supported_schema_version),
        )
    }
}

struct ActiveRollout {
    session: String,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

struct RolloutRun {
    session: String,
    config: RolloutConfig,
    stop: watch::Receiver<bool>,
}

impl RolloutRun {
    fn new(session: String, config: RolloutConfig, stop: watch::Receiver<bool>) -> Self {
        Self {
            session,
            config,
            stop,
        }
    }
}

impl ActiveRollout {
    fn new(session: String, stop: watch::Sender<bool>, task: JoinHandle<()>) -> Self {
        Self {
            session,
            stop,
            task,
        }
    }

    async fn stop_and_join(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

struct WatchRolloutEvents {
    state: watch::Sender<RolloutSnapshot>,
    persistence: watch::Sender<RolloutSnapshot>,
}

impl RolloutEventPort for WatchRolloutEvents {
    fn publish(&self, snapshot: &RolloutSnapshot) {
        self.state.send_replace(snapshot.clone());
        self.persistence.send_replace(snapshot.clone());
    }
}

struct CameraInput {
    name: String,
    receiver: watch::Receiver<Option<Arc<Frame>>>,
}

struct CapturedImage {
    name: String,
    width: usize,
    height: usize,
    frame: Arc<Frame>,
}

struct InferenceObservation {
    state: Vec<f32>,
    /// State in LeRobot's raw SO-101 units, retained only for its server-side
    /// near-duplicate observation filter. Model input remains normalized.
    similarity_state: Vec<f32>,
    images: Vec<CapturedImage>,
    /// Absolute policy timestep whose corresponding action is chunk index zero.
    requested_timestep: u64,
    /// Oldest concrete sensor input, used for end-to-end latency metrics.
    oldest_input_at: Instant,
}

struct InferenceObservationRequest {
    state: Vec<f32>,
    similarity_state: Vec<f32>,
    requested_timestep: u64,
    requested_at: Instant,
}

impl InferenceObservationRequest {
    fn new(state: Vec<f32>, similarity_state: Vec<f32>) -> Self {
        Self {
            state,
            similarity_state,
            requested_timestep: 0,
            requested_at: Instant::now(),
        }
    }

    fn requested_at(mut self, requested_timestep: u64, requested_at: Instant) -> Self {
        self.requested_timestep = requested_timestep;
        self.requested_at = requested_at;
        self
    }
}

struct ObservationCapture<'a> {
    cameras: &'a [CameraInput],
    specifications: &'a [spiral_robotics_inference::ImageInputSpecification],
}

impl<'a> ObservationCapture<'a> {
    fn new(
        cameras: &'a [CameraInput],
        specifications: &'a [spiral_robotics_inference::ImageInputSpecification],
    ) -> Self {
        Self {
            cameras,
            specifications,
        }
    }
}

/// LeRobot's asynchronous policy server accepts observations through a
/// `Queue(maxsize=1)`: a newly arrived observation replaces an older pending
/// one while inference is occupied. Keep that behavior explicit so queued
/// inference can never accumulate stale camera frames.
#[derive(Default)]
struct LatestObservationSlot {
    pending: Option<InferenceObservation>,
}

impl LatestObservationSlot {
    fn replace(&mut self, observation: InferenceObservation) {
        self.pending = Some(observation);
    }

    fn take(&mut self) -> Option<InferenceObservation> {
        self.pending.take()
    }
}

trait ObservationSimilarity {
    fn is_similar_to(&self, previous: &[f32]) -> bool;
}

impl ObservationSimilarity for [f32] {
    fn is_similar_to(&self, previous: &[f32]) -> bool {
        self.len() == previous.len()
            && self
                .iter()
                .zip(previous)
                .map(|(candidate, previous)| (candidate - previous).powi(2))
                .sum::<f32>()
                < OBSERVATION_SIMILARITY_TOLERANCE.powi(2)
    }
}

struct DecodedImage {
    name: String,
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

struct CompletedInference {
    chunk: ActionChunk,
    requested_timestep: u64,
    oldest_input_at: Instant,
}

struct InferenceTaskOutput {
    policy: Box<dyn Policy>,
    result: Result<CompletedInference, String>,
}

type InferenceTask = JoinHandle<InferenceTaskOutput>;

/// A contiguous queue of actions keyed by the rollout's absolute policy clock.
///
/// This is the local equivalent of LeRobot's async `TimedAction` queue. A
/// completed chunk is aligned by its observation timestep, its already elapsed
/// prefix is discarded, and every still-future overlap is blended. Keeping the
/// full model horizon gives the actor enough reserve to continue at 30 Hz while
/// the next image decode and forward pass run in the background.
#[derive(Debug)]
struct TimedActionQueue {
    next_timestep: u64,
    action_dimensions: usize,
    actions: VecDeque<Vec<f32>>,
}

impl TimedActionQueue {
    fn from_initial_chunk(chunk: &ActionChunk) -> Result<Self, String> {
        let action_dimensions = chunk.action_dimensions();
        let actions = chunk.actions()?;
        if actions.is_empty() {
            return Err("policy returned an empty initial action chunk".to_owned());
        }
        Ok(Self {
            next_timestep: 0,
            action_dimensions,
            actions: actions.into(),
        })
    }

    #[cfg(test)]
    fn next_timestep(&self) -> u64 {
        self.next_timestep
    }

    fn latest_action_timestep(&self) -> Result<u64, String> {
        self.next_timestep.checked_sub(1).ok_or_else(|| {
            "cannot label an observation before the first action has executed".to_owned()
        })
    }

    fn remaining(&self) -> usize {
        self.actions.len()
    }

    fn merge(&mut self, requested_timestep: u64, chunk: &ActionChunk) -> Result<(), String> {
        if chunk.action_dimensions() != self.action_dimensions {
            return Err(format!(
                "policy action width changed from {} to {} during rollout",
                self.action_dimensions,
                chunk.action_dimensions()
            ));
        }
        if requested_timestep > self.next_timestep {
            return Err(format!(
                "policy returned actions for future timestep {requested_timestep} while timestep {} is due",
                self.next_timestep
            ));
        }
        let incoming = chunk.actions()?;
        let elapsed = self
            .next_timestep
            .saturating_sub(requested_timestep)
            .try_into()
            .map_err(|_| "policy inference delay exceeds the action index range".to_owned())?;
        // LeRobot accepts an entirely stale response as an empty queue. Its
        // control loop keeps the servo's last target active and sends one
        // must-go observation to recover instead of terminating the rollout.
        let future = incoming.get(elapsed..).unwrap_or(&[]);

        let current = &self.actions;
        self.actions = future
            .iter()
            .enumerate()
            .map(|(offset, incoming)| {
                current.get(offset).map_or_else(
                    || incoming.clone(),
                    |existing| existing.blended_with(incoming),
                )
            })
            .collect();
        Ok(())
    }

    fn pop(&mut self) -> Result<Vec<f32>, String> {
        let action = self.actions.pop_front().ok_or_else(|| {
            "asynchronous inference did not replenish the action queue before it was exhausted"
                .to_owned()
        })?;
        self.next_timestep = self.next_timestep.saturating_add(1);
        Ok(action)
    }
}

trait ActionChunkRows {
    fn actions(&self) -> Result<Vec<Vec<f32>>, String>;
}

impl ActionChunkRows for ActionChunk {
    fn actions(&self) -> Result<Vec<Vec<f32>>, String> {
        (0..self.steps())
            .map(|step| {
                self.action(step)
                    .map(<[f32]>::to_vec)
                    .ok_or_else(|| "policy returned an incomplete action sequence".to_owned())
            })
            .collect()
    }
}

trait ActionValues {
    fn blended_with(&self, incoming: &[f32]) -> Vec<f32>;
}

impl ActionValues for [f32] {
    fn blended_with(&self, incoming: &[f32]) -> Vec<f32> {
        debug_assert_eq!(self.len(), incoming.len());
        self.iter()
            .zip(incoming)
            .map(|(old, new)| ACTION_BLEND_OLD_WEIGHT * old + ACTION_BLEND_NEW_WEIGHT * new)
            .collect()
    }
}

trait ActionChunkSize {
    fn refill_threshold(self) -> usize;
}

impl ActionChunkSize for usize {
    fn refill_threshold(self) -> usize {
        self / 2
    }
}

pub(crate) struct RolloutHardwareResources {
    hardware: Arc<HardwareHandle>,
    cameras: Arc<CameraManager>,
    workspace: Arc<RobotWorkspace>,
}

impl RolloutHardwareResources {
    pub(crate) fn new(
        hardware: Arc<HardwareHandle>,
        cameras: Arc<CameraManager>,
        workspace: Arc<RobotWorkspace>,
    ) -> Self {
        Self {
            hardware,
            cameras,
            workspace,
        }
    }
}

struct RolloutLifecycle {
    state: watch::Sender<RolloutSnapshot>,
    coordinator: Mutex<RolloutCoordinator>,
    active: Mutex<Option<ActiveRollout>>,
    session_ids: StableIdSequence,
}

impl RolloutLifecycle {
    fn take_active(&self) -> Option<ActiveRollout> {
        self.active.recover_lock().take()
    }
}

pub(crate) struct RolloutManager {
    storage: Arc<StorageManager>,
    hardware: RolloutHardwareResources,
    preferences: Arc<StudioPreferencesStore>,
    clock: Arc<dyn Clock>,
    lifecycle: RolloutLifecycle,
}

impl RolloutManager {
    pub(crate) fn new(
        storage: Arc<StorageManager>,
        hardware: RolloutHardwareResources,
        preferences: Arc<StudioPreferencesStore>,
    ) -> Arc<Self> {
        // Persisted state is hydrated after the listener starts. Constructing
        // the manager must not put Spiral I/O on the server's critical path.
        let initial = RolloutSnapshot::default();
        let (persistence, mut changes) = watch::channel(initial.clone());
        let persistence_storage = storage.clone();
        tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                let snapshot = changes.borrow_and_update().clone();
                if let Err(error) = persistence_storage.save_rollout_session(&snapshot).await {
                    tracing::error!("could not persist rollout state: {error}");
                }
            }
        });
        let (state, receiver) = watch::channel(initial);
        drop(receiver);
        let events = Arc::new(WatchRolloutEvents {
            state: state.clone(),
            persistence,
        });
        Arc::new(Self {
            storage,
            hardware,
            preferences,
            clock: Arc::new(SystemClock),
            lifecycle: RolloutLifecycle {
                state,
                coordinator: Mutex::new(RolloutCoordinator::new(events)),
                active: Mutex::new(None),
                session_ids: StableIdSequence::new("rollout", 1),
            },
        })
    }

    pub(crate) async fn reload_from_storage(&self) -> Result<(), String> {
        if self.lifecycle.active.recover_lock().is_some() {
            return Err("cannot reload rollout history while a session is active".to_owned());
        }
        let snapshot = self
            .storage
            .load_rollout_session()
            .await?
            .unwrap_or_default();
        // Install under the same lock `start()` holds for admission, so a
        // session admitted while storage was being read is never overwritten
        // by this older snapshot.
        let active = self.lifecycle.active.recover_lock();
        if active.is_some() {
            return Err(
                "a rollout session started while reloading; keeping the live session".to_owned(),
            );
        }
        self.lifecycle
            .state
            .send_replace(snapshot.restored_after_restart());
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> RolloutSnapshot {
        self.lifecycle.state.borrow().clone()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<RolloutSnapshot> {
        self.lifecycle.state.subscribe()
    }

    pub(crate) async fn start(
        self: &Arc<Self>,
        config: RolloutConfig,
    ) -> Result<RolloutSnapshot, String> {
        let mut active = self.lifecycle.active.recover_lock();
        let (session, created_ms) = self
            .lifecycle
            .session_ids
            .next(self.clock.as_ref())
            .into_parts();
        let (stop, stop_receiver) = watch::channel(false);
        self.lifecycle
            .coordinator
            .recover_lock()
            .admit(RolloutAdmission::new(
                session.clone(),
                created_ms,
                config.clone(),
            ))?;
        // Spawn and publish the task handle while holding the same mutex used
        // by shutdown. Once an active session is observable its cleanup task
        // is therefore always joinable; there is no `active + task=None`
        // window for graceful shutdown to miss.
        let manager = self.clone();
        let run = RolloutRun::new(session.clone(), config, stop_receiver);
        let task = tokio::spawn(async move {
            manager.run_and_finish(run).await;
        });
        debug_assert!(active.is_none(), "rollout admission is exclusive");
        *active = Some(ActiveRollout::new(session, stop, task));
        Ok(self.snapshot())
    }

    pub(crate) async fn stop(&self) -> Result<(), String> {
        let active = self.lifecycle.active.recover_lock();
        let Some(active) = active.as_ref() else {
            return Err("no policy rollout is active".to_owned());
        };
        self.lifecycle.coordinator.recover_lock().request_stop()?;
        active
            .stop
            .send(true)
            .map_err(|_| "the rollout task already stopped".to_owned())
    }

    pub(crate) async fn shutdown(&self) {
        let task = {
            let Some(active) = self.lifecycle.take_active() else {
                return;
            };
            let _ = self.lifecycle.coordinator.recover_lock().request_stop();
            active
        };
        task.stop_and_join().await;
    }

    async fn run_and_finish(self: Arc<Self>, run: RolloutRun) {
        let session = run.session.clone();
        let outcome = match std::panic::AssertUnwindSafe(self.run_session(run))
            .catch_unwind()
            .await
        {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => ExecutionOutcome::Failed(error),
            Err(_) => ExecutionOutcome::Failed("policy executor panicked unexpectedly".to_owned()),
        };
        let ticket = self
            .lifecycle
            .coordinator
            .recover_lock()
            .begin_cleanup(&session, outcome);
        // Hardware cleanup is unconditional. Even an internal lifecycle bug
        // must not strand the arm under policy ownership.
        let cleanup = self.hardware.hardware.stop_policy_session(&session).await;
        let mut active = self.lifecycle.active.recover_lock();
        match ticket {
            Ok(ticket) => {
                if let Err(error) = self
                    .lifecycle
                    .coordinator
                    .recover_lock()
                    .finish_cleanup(ticket, CleanupResult::new(self.clock.now_ms(), cleanup))
                {
                    tracing::error!(session, "rollout cleanup could not be finalized: {error}");
                }
            }
            Err(error) => {
                tracing::error!(session, "rollout cleanup was not admitted: {error}");
                if let Err(cleanup) = cleanup {
                    tracing::error!(session, "emergency hardware cleanup also failed: {cleanup}");
                }
            }
        }
        if active.as_ref().is_some_and(|item| item.session == session) {
            active.take();
        }
    }

    async fn run_session(&self, run: RolloutRun) -> Result<ExecutionOutcome, String> {
        let RolloutRun {
            session,
            config,
            mut stop,
        } = run;
        let session = session.as_str();
        let artifact = config.load_checkpoint(self.storage.as_ref()).await;
        if *stop.borrow() {
            return self
                .lifecycle
                .coordinator
                .recover_lock()
                .stopped_outcome(session);
        }
        let artifact = artifact?;
        let manifest = artifact.manifest()?;
        manifest.validate_for(&config, artifact.config_json())?;
        let (config_json, payload) = artifact.into_parts();
        let model_artifact =
            spiral_robotics_inference::ModelArtifact::from_bytes(config_json, payload)
                .map_err(|error| format!("checkpoint identity is invalid: {error}"))?;
        let policy_loader = HttpPolicyLoader::new(self.preferences.get().training().endpoint());
        let policy = tokio::task::spawn_blocking(move || policy_loader.load(model_artifact))
            .await
            .map_err(|error| format!("policy loader stopped unexpectedly: {error}"))?
            .map_err(|error| format!("checkpoint could not be loaded remotely: {error}"))?;
        let specification = policy.specification().clone();
        let hardware_state = self.hardware.hardware.state();
        let record = self
            .hardware
            .workspace
            .get(config.robot())
            .await
            .map_err(|error| error.to_string())?;
        let connection = hardware_state
            .connections()
            .get(config.robot())
            .cloned()
            .unwrap_or_default();
        let summary = record.summary(connection.to_api());
        let contract = manifest
            .data_contract
            .into_policy_contract(spiral_robotics_storage::SCHEMA_VERSION)
            .validated_with(
                &PolicyModelContract::new(
                    specification.state_dimensions(),
                    specification.action_dimensions(),
                    specification
                        .image_inputs()
                        .iter()
                        .map(|image| image.name().to_owned())
                        .collect(),
                )
                .with_action_shape(
                    specification.action_chunk_size(),
                    specification.recommended_action_steps(),
                ),
                &TargetRobotContract::new(
                    summary.id(),
                    summary.robot_type().map(str::to_owned),
                    summary
                        .motors()
                        .iter()
                        .map(|motor| motor.name().to_owned())
                        .collect(),
                )
                .with_cameras(
                    summary
                        .cameras()
                        .iter()
                        .map(|binding| CameraBindingContract::new(binding.name(), binding.id()))
                        .collect(),
                )
                .with_availability(summary.connection().connected(), summary.calibrated())
                .with_problem(summary.problem().map(str::to_owned)),
            )
            .map_err(|error| error.to_string())?;
        let joint_names = contract.joint_names().to_vec();
        let period = contract.sample_period();
        let sample_hz = contract.sample_hz();
        let reobservation_steps = specification
            .action_chunk_size()
            .saturating_sub(specification.action_chunk_size().refill_threshold());
        let mut cameras = Vec::with_capacity(contract.camera_bindings().len());
        for binding in contract.camera_bindings() {
            cameras.push(CameraInput {
                name: binding.stream().to_owned(),
                receiver: self.hardware.cameras.subscribe(binding.camera_id())?,
            });
        }

        if !self
            .lifecycle
            .coordinator
            .recover_lock()
            .warming_up(session)?
        {
            return self
                .lifecycle
                .coordinator
                .recover_lock()
                .stopped_outcome(session);
        }
        let (policy, warmup) = tokio::task::spawn_blocking(move || {
            let mut policy = policy;
            let warmup = policy.warm_up();
            (policy, warmup)
        })
        .await
        .map_err(|error| format!("policy warm-up stopped unexpectedly: {error}"))?;
        let warmup = warmup.map_err(|error| format!("policy setup failed: {error}"))?;
        self.lifecycle.coordinator.recover_lock().record_warmup(
            session,
            InferenceTiming::new(warmup.total().milliseconds(), warmup.model().milliseconds()),
        )?;
        cameras.wait_until_fresh(&mut stop).await?;
        if *stop.borrow() {
            return self
                .lifecycle
                .coordinator
                .recover_lock()
                .stopped_outcome(session);
        }
        if !self
            .lifecycle
            .coordinator
            .recover_lock()
            .awaiting_control(session)?
        {
            return self
                .lifecycle
                .coordinator
                .recover_lock()
                .stopped_outcome(session);
        }

        self.hardware
            .hardware
            .set_mode(ModeSelection::Policy {
                robot: config.robot().to_owned(),
                session: session.to_owned(),
            })
            .await?;
        let mut hardware_updates = self.hardware.hardware.subscribe();
        match PolicyAdmissionWait::new(
            &mut hardware_updates,
            &mut stop,
            ExpectedPolicyOwner::new(config.robot(), session),
        )
        .wait()
        .await?
        {
            PolicyAdmission::Admitted => {}
            PolicyAdmission::Stopped => {
                return self
                    .lifecycle
                    .coordinator
                    .recover_lock()
                    .stopped_outcome(session);
            }
            PolicyAdmission::Replaced(mode) => {
                if let Some(outcome) = self.classify_control(session, &mode)? {
                    return Ok(outcome);
                }
                return Err("policy admission ended without a control transition".to_owned());
            }
        }
        if !self
            .lifecycle
            .coordinator
            .recover_lock()
            .running(session, PolicyCadence::new(sample_hz, reobservation_steps))?
        {
            return self
                .lifecycle
                .coordinator
                .recover_lock()
                .stopped_outcome(session);
        }

        // The first chunk is prepared after policy admission but before the
        // rollout clock starts. The follower holds its admitted pose during
        // this one-time fill; once actions start, every later inference runs
        // concurrently with dispatch from the current action queue.
        let initial_hardware = self.hardware.hardware.state();
        if let Some(outcome) = self.classify_control(session, &initial_hardware)? {
            return Ok(outcome);
        }
        let initial_robot = initial_hardware.policy_robot(config.robot())?;
        initial_robot.require_healthy_for_policy()?;
        cameras.require_fresh()?;
        let initial_state = initial_robot.policy_observation_state(&joint_names)?;
        let initial_similarity_state = initial_robot.lerobot_similarity_state(&joint_names)?;
        let initial_observation =
            ObservationCapture::new(&cameras, specification.image_inputs()).capture(
                InferenceObservationRequest::new(initial_state, initial_similarity_state),
            )?;
        let mut last_processed_similarity_state = initial_observation.similarity_state.clone();
        let mut initial_task = initial_observation.spawn_inference(policy);
        let initial_output = loop {
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return self.lifecycle.coordinator.recover_lock().stopped_outcome(session);
                    }
                }
                output = &mut initial_task => break output,
            }
        };
        let (initial_policy, initial) = InferenceTaskOutput::resolve_joined(initial_output)?;
        let initial_latency = initial.age();
        self.lifecycle.coordinator.recover_lock().record_inference(
            session,
            InferenceTiming::new(
                initial_latency.milliseconds(),
                initial.chunk.stats().model().milliseconds(),
            ),
        )?;

        let mut policy = Some(initial_policy);
        let mut actions = TimedActionQueue::from_initial_chunk(&initial.chunk)?;
        // LeRobot's async client requests observations once queue_size /
        // action_chunk_size <= 0.5. Integer queue sizes make this floor(N / 2).
        let refill_threshold = initial.chunk.steps().refill_threshold();
        let mut inference = None::<InferenceTask>;
        // Equivalent to LeRobot's one-element server observation queue: while
        // a forward pass is busy, every newer eligible observation replaces
        // the previous pending one.
        let mut pending_observation = LatestObservationSlot::default();
        // LeRobot arms one must-go observation whenever an action chunk is
        // received. It is consumed only if that chunk runs completely dry.
        let mut must_go_when_empty = true;
        let deadline =
            TokioInstant::now() + Duration::from_secs(u64::from(config.duration_seconds()));
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_hardware_sequence = 0_u64;
        let mut hardware_changed_at = Instant::now();

        loop {
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return self.lifecycle.coordinator.recover_lock().stopped_outcome(session);
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Ok(ExecutionOutcome::Completed(CompletionReason::DurationReached {
                        duration_seconds: config.duration_seconds(),
                    }));
                }
                _ = ticker.tick() => {}
            }
            let cycle_started = Instant::now();
            let hardware = self.hardware.hardware.state();
            if hardware.sequence() != last_hardware_sequence {
                last_hardware_sequence = hardware.sequence();
                hardware_changed_at = Instant::now();
            } else if hardware_changed_at.elapsed() > INPUT_STALE_AFTER {
                return Err("stopped policy: hardware telemetry became stale".to_owned());
            }
            if let Some(outcome) = self.classify_control(session, &hardware)? {
                return Ok(outcome);
            }
            let robot = hardware.policy_robot(config.robot())?;
            robot.require_healthy_for_policy()?;
            cameras.require_fresh()?;

            // Polling an already-finished blocking task is immediate. While it
            // is still running, this branch does nothing and the buffered queue
            // continues to feed one action on every policy-clock tick.
            if inference.as_ref().is_some_and(JoinHandle::is_finished) {
                let output = inference
                    .take()
                    .expect("a finished inference task is present")
                    .await;
                let (returned_policy, completed) = InferenceTaskOutput::resolve_joined(output)?;
                let latency = completed.age();
                self.lifecycle.coordinator.recover_lock().record_inference(
                    session,
                    InferenceTiming::new(
                        latency.milliseconds(),
                        completed.chunk.stats().model().milliseconds(),
                    ),
                )?;
                actions.merge(completed.requested_timestep, &completed.chunk)?;
                must_go_when_empty = true;
                policy = Some(returned_policy);
            }

            let action_executed = if actions.remaining() > 0 {
                let action = actions.pop()?;
                let prepared = PreparedActionGoals::from_action(&joint_names, &action)
                    .map_err(|error| format!("stopped policy: {error}"))?;
                if let Some(clamp) = prepared.clamps().first() {
                    let additional = prepared.clamps().len().saturating_sub(1);
                    let warning = if additional == 0 {
                        format!(
                            "Policy output for joint {:?} exceeded normalized [-1, 1]; the command was clamped to the limit.",
                            clamp.joint()
                        )
                    } else {
                        format!(
                            "Policy output for joint {:?} and {additional} other joint{} exceeded normalized [-1, 1]; the commands were clamped to the limits.",
                            clamp.joint(),
                            if additional == 1 { "" } else { "s" }
                        )
                    };
                    if self
                        .lifecycle
                        .coordinator
                        .recover_lock()
                        .record_action_clamp_warning(session, warning)?
                    {
                        tracing::warn!(
                            joint = %clamp.joint(),
                            index = clamp.index(),
                            requested = clamp.requested(),
                            applied = clamp.applied(),
                            additional_clamps = additional,
                            "clamped policy action outside normalized range"
                        );
                    }
                }

                let goals =
                    spiral_robotics_hardware::JointGoals::try_from(prepared.goals().clone())
                        .map_err(|error| error.to_string())?;
                let goals = PolicyGoals::new(config.robot().to_owned(), session.to_owned(), goals)
                    .map_err(|error| error.to_string())?;
                self.hardware.hardware.set_policy_goals(goals).await?;
                self.lifecycle
                    .coordinator
                    .recover_lock()
                    .record_command(session, cycle_started.elapsed() > period)?;
                true
            } else {
                // The independent hardware loop continues ramping toward the
                // last accepted target while inference catches up.
                false
            };

            // LeRobot executes the action first, records it as latest_action,
            // and only then captures an observation carrying that timestep.
            // Preserve that ordering and keep replacing the one-slot pending
            // observation while the remote policy worker is occupied.
            let must_go = actions.remaining() == 0 && must_go_when_empty;
            if must_go || (action_executed && actions.remaining() <= refill_threshold) {
                let refreshed = self.hardware.hardware.state();
                if let Some(outcome) = self.classify_control(session, &refreshed)? {
                    return Ok(outcome);
                }
                let refreshed_robot = refreshed.policy_robot(config.robot())?;
                refreshed_robot.require_healthy_for_policy()?;
                cameras.require_fresh()?;
                let state = refreshed_robot.policy_observation_state(&joint_names)?;
                let similarity_state = refreshed_robot.lerobot_similarity_state(&joint_names)?;
                let latest_action = actions.latest_action_timestep()?;
                let observation = ObservationCapture::new(&cameras, specification.image_inputs())
                    .capture(
                    InferenceObservationRequest::new(state, similarity_state)
                        .requested_at(latest_action, Instant::now()),
                )?;
                if must_go
                    || !observation
                        .similarity_state
                        .is_similar_to(&last_processed_similarity_state)
                {
                    pending_observation.replace(observation);
                }
                if must_go {
                    must_go_when_empty = false;
                }
            }
            if policy.is_some()
                && let Some(observation) = pending_observation.take()
            {
                last_processed_similarity_state.clone_from(&observation.similarity_state);
                let next_policy = policy.take().expect("an idle policy is available");
                debug_assert!(inference.is_none());
                inference = Some(observation.spawn_inference(next_policy));
            }
        }
    }

    fn classify_control(
        &self,
        session: &str,
        hardware: &HardwareState,
    ) -> Result<Option<ExecutionOutcome>, String> {
        let owner = hardware.mode().control_owner();
        match self.lifecycle.coordinator.recover_lock().observe_control(
            session,
            ControlObservation::new(&owner, hardware.mode_error()),
        )? {
            ControlDecision::Continue => Ok(None),
            ControlDecision::Finish(reason) => Ok(Some(ExecutionOutcome::Completed(reason))),
            ControlDecision::Fail(error) => Ok(Some(ExecutionOutcome::Failed(error))),
        }
    }
}

trait RestoredRolloutSnapshot {
    fn restored_after_restart(self) -> Self;
}

impl RestoredRolloutSnapshot for RolloutSnapshot {
    fn restored_after_restart(self) -> Self {
        if self.phase().active() {
            let finished_ms = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
                    .unwrap_or(0),
            );
            return Self::new(
                self.session_id().map(str::to_owned),
                self.run_id().map(str::to_owned),
            )
            .with_robot(self.robot().map(str::to_owned))
            .with_checkpoint(self.checkpoint_step())
            .with_duration(self.duration_seconds())
            .with_lifecycle(RolloutPhase::Failed, self.started_ms())
            .with_completion(
                finished_ms,
                Some("Studio stopped before this rollout completed.".to_owned()),
            )
            .with_warning(Some(
                "No policy session was resumed after restart.".to_owned(),
            ))
            .with_metrics(self.metrics().clone());
        }
        self
    }
}

/// Wait until the control thread publishes the Policy mode it already accepted.
///
/// Command replies are sent while the fixed-rate control thread is draining its
/// queue, before that cycle's state is published. Reading `HardwareHandle::state`
/// immediately after `set_mode` can therefore still return the previous Idle
/// snapshot. Admission is complete only when the exact robot/session pair is
/// visible in an authoritative published state.
enum PolicyAdmission {
    Admitted,
    Stopped,
    Replaced(Box<HardwareState>),
}

struct PolicyAdmissionWait<'a> {
    hardware: &'a mut watch::Receiver<HardwareState>,
    stop: &'a mut watch::Receiver<bool>,
    expected: ExpectedPolicyOwner<'a>,
}

impl<'a> PolicyAdmissionWait<'a> {
    fn new(
        hardware: &'a mut watch::Receiver<HardwareState>,
        stop: &'a mut watch::Receiver<bool>,
        expected: ExpectedPolicyOwner<'a>,
    ) -> Self {
        Self {
            hardware,
            stop,
            expected,
        }
    }

    async fn wait(self) -> Result<PolicyAdmission, String> {
        let deadline = TokioInstant::now() + INPUT_STALE_AFTER;
        loop {
            if *self.stop.borrow() {
                return Ok(PolicyAdmission::Stopped);
            }
            let state = self.hardware.borrow().clone();
            if matches!(
                state.mode(),
                ModeSelection::Policy { robot, session }
                    if robot == self.expected.robot() && session == self.expected.session()
            ) {
                return Ok(PolicyAdmission::Admitted);
            }

            tokio::select! {
                changed = self.stop.changed() => {
                    if changed.is_err() || *self.stop.borrow() {
                        return Ok(PolicyAdmission::Stopped);
                    }
                }
                changed = self.hardware.changed() => {
                    if changed.is_err() {
                        return Err(
                            "stopped policy: hardware telemetry closed before policy control was admitted"
                                .to_owned(),
                        );
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Ok(PolicyAdmission::Replaced(Box::new(
                        self.hardware.borrow().clone(),
                    )));
                }
            }
        }
    }
}

trait ModeSelectionPolicy {
    fn control_owner(&self) -> ControlOwner;
    fn control_description(&self) -> String;
}

impl ModeSelectionPolicy for ModeSelection {
    fn control_owner(&self) -> ControlOwner {
        match self {
            Self::Policy { robot, session } => ControlOwner::Policy {
                robot: robot.clone(),
                session: session.clone(),
            },
            selection => ControlOwner::Other(selection.control_description()),
        }
    }

    fn control_description(&self) -> String {
        match self {
            Self::Idle => "Idle".to_owned(),
            Self::Manual { robot } => format!("Manual on {robot}"),
            Self::Teleoperate { leader, follower } => {
                format!("Teleoperation from {leader} to {follower}")
            }
            Self::Replay { robot, episode } => {
                format!("Replay of {episode} on {robot}")
            }
            Self::Policy { robot, session } => {
                format!("Policy session {session} on {robot}")
            }
        }
    }
}

trait RolloutCheckpointRequest {
    async fn load_checkpoint(
        &self,
        checkpoint_store: &dyn CheckpointStore,
    ) -> Result<CheckpointArtifact, String>;
}

impl RolloutCheckpointRequest for RolloutConfig {
    async fn load_checkpoint(
        &self,
        checkpoint_store: &dyn CheckpointStore,
    ) -> Result<CheckpointArtifact, String> {
        checkpoint_store
            .load_checkpoint(CheckpointKey::new(
                self.run_id().to_owned(),
                self.checkpoint_step(),
            ))
            .await
    }
}

trait CheckpointManifest {
    fn manifest(&self) -> Result<ArtifactManifest, String>;
}

impl CheckpointManifest for CheckpointArtifact {
    fn manifest(&self) -> Result<ArtifactManifest, String> {
        let (_, metadata) = SafeTensors::read_metadata(self.payload())
            .map_err(|error| format!("checkpoint metadata is invalid: {error}"))?;
        let manifest = metadata
            .metadata()
            .as_ref()
            .and_then(|metadata| metadata.get("spiral_robotics"))
            .ok_or_else(|| "checkpoint has no Spiral Robotics manifest".to_owned())?;
        serde_json::from_str(manifest)
            .map_err(|error| format!("checkpoint manifest is invalid: {error}"))
    }
}

impl ArtifactManifest {
    fn validate_for(
        &self,
        rollout_config: &RolloutConfig,
        model_config_bytes: &[u8],
    ) -> Result<(), String> {
        ArtifactIdentity::new(&self.format, &self.artifact, &self.run_id)
            .with_artifact(self.step, &self.tensor_namespace)
            .validate_for(rollout_config.run_id(), rollout_config.checkpoint_step())
            .map_err(|error| error.to_string())?;
        let stored_config: serde_json::Value = serde_json::from_slice(model_config_bytes)
            .map_err(|error| format!("stored native model config is invalid: {error}"))?;
        if stored_config != self.model_config {
            return Err(
                "stored native model config does not match the checkpoint manifest".to_owned(),
            );
        }
        Ok(())
    }
}

trait HardwarePolicyTelemetry {
    fn policy_robot(&self, robot: &str) -> Result<&RobotTelemetry, String>;
}

impl HardwarePolicyTelemetry for HardwareState {
    fn policy_robot(&self, robot: &str) -> Result<&RobotTelemetry, String> {
        self.robots()
            .iter()
            .find(|candidate| candidate.id() == robot)
            .ok_or_else(|| "stopped policy: follower telemetry disappeared".to_owned())
    }
}

trait PolicyRobotTelemetry {
    fn require_healthy_for_policy(&self) -> Result<(), String>;
    fn policy_observation_state(&self, joint_names: &[String]) -> Result<Vec<f32>, String>;
    fn lerobot_similarity_state(&self, joint_names: &[String]) -> Result<Vec<f32>, String>;
}

impl PolicyRobotTelemetry for RobotTelemetry {
    fn require_healthy_for_policy(&self) -> Result<(), String> {
        if let Some(error) = &self.error() {
            return Err(format!("stopped policy: follower reported {error}"));
        }
        if let Some(joint) = self
            .joints()
            .iter()
            .find(|joint| !joint.faults().is_empty())
        {
            return Err(format!(
                "stopped policy: joint {:?} reported a motor fault",
                joint.name()
            ));
        }
        Ok(())
    }

    fn policy_observation_state(&self, joint_names: &[String]) -> Result<Vec<f32>, String> {
        self.joints()
            .iter()
            .map(|joint| JointObservation::new(joint.name(), joint.normalized()))
            .collect::<Vec<_>>()
            .ordered_state(joint_names)
            .map_err(|error| format!("stopped policy: {error}"))
    }

    /// Reconstruct the raw joint units used by LeRobot's SO-101 client solely
    /// for its near-duplicate observation filter.
    fn lerobot_similarity_state(&self, joint_names: &[String]) -> Result<Vec<f32>, String> {
        joint_names
            .iter()
            .map(|name| {
                let joint = self
                    .joints()
                    .iter()
                    .find(|joint| joint.name() == *name)
                    .ok_or_else(|| {
                        format!("stopped policy: joint {name:?} disappeared from live telemetry")
                    })?;
                let value = if name == "gripper" {
                    joint.normalized().map(|value| (value + 1.0) * 50.0)
                } else {
                    joint.degrees()
                };
                value
                    .filter(|value| value.is_finite())
                    .map(|value| value as f32)
                    .ok_or_else(|| {
                        format!(
                            "stopped policy: joint {name:?} has no finite LeRobot-space telemetry"
                        )
                    })
            })
            .collect()
    }
}

impl ObservationCapture<'_> {
    fn capture(self, request: InferenceObservationRequest) -> Result<InferenceObservation, String> {
        let InferenceObservationRequest {
            state,
            similarity_state,
            requested_timestep,
            requested_at,
        } = request;
        if self.cameras.len() != self.specifications.len() {
            return Err(format!(
                "policy camera contract changed from {} inputs to {} specifications",
                self.cameras.len(),
                self.specifications.len()
            ));
        }
        let now = Instant::now();
        let mut oldest_input_at = requested_at;
        let images = self
            .cameras
            .iter()
            .zip(self.specifications)
            .map(|(camera, specification)| {
                if camera.name != specification.name() {
                    return Err(format!(
                        "policy camera order changed from {:?} to {:?}",
                        camera.name,
                        specification.name()
                    ));
                }
                let frame = camera
                    .receiver
                    .borrow()
                    .clone()
                    .ok_or_else(|| format!("camera {:?} has no frame", camera.name))?;
                if now.saturating_duration_since(frame.captured_at()) > INPUT_STALE_AFTER {
                    return Err(format!("camera {:?} became stale", camera.name));
                }
                oldest_input_at = oldest_input_at.min(frame.captured_at());
                Ok(CapturedImage {
                    name: camera.name.clone(),
                    width: specification.width(),
                    height: specification.height(),
                    frame,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(InferenceObservation {
            state,
            similarity_state,
            images,
            requested_timestep,
            oldest_input_at,
        })
    }
}

impl InferenceObservation {
    fn spawn_inference(self, policy: Box<dyn Policy>) -> InferenceTask {
        tokio::task::spawn_blocking(move || {
            let mut policy = policy;
            let result = self.infer_with(policy.as_mut());
            InferenceTaskOutput { policy, result }
        })
    }

    fn infer_with(self, policy: &mut dyn Policy) -> Result<CompletedInference, String> {
        let Self {
            state,
            similarity_state: _,
            images,
            requested_timestep,
            oldest_input_at,
        } = self;
        let decoded = images
            .into_iter()
            .map(CapturedImage::decode)
            .collect::<Result<Vec<_>, _>>()?;
        let views = decoded
            .iter()
            .map(|image| {
                ImageFrame::packed_rgb8(
                    &image.name,
                    ImageDimensions::new(image.width, image.height),
                    &image.pixels,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("preparing camera input: {error}"))?;
        let chunk = policy
            .infer(InferenceInput::new(&state, &views))
            .map_err(|error| format!("policy inference failed: {error}"))?;
        Ok(CompletedInference {
            chunk,
            requested_timestep,
            oldest_input_at,
        })
    }
}

impl CapturedImage {
    fn decode(self) -> Result<DecodedImage, String> {
        let image = image::load_from_memory(self.frame.bytes())
            .map_err(|error| format!("decoding camera {:?}: {error}", self.name))?
            .to_rgb8();
        let image = image::imageops::resize(
            &image,
            self.width as u32,
            self.height as u32,
            FilterType::Triangle,
        );
        Ok(DecodedImage {
            name: self.name,
            width: self.width,
            height: self.height,
            pixels: image.into_raw(),
        })
    }
}

impl InferenceTaskOutput {
    fn resolve_joined(
        output: Result<Self, tokio::task::JoinError>,
    ) -> Result<(Box<dyn Policy>, CompletedInference), String> {
        let output = output.map_err(|error| format!("policy inference worker stopped: {error}"))?;
        output.result.map(|inference| (output.policy, inference))
    }
}

impl CompletedInference {
    fn age(&self) -> Duration {
        Instant::now().saturating_duration_since(self.oldest_input_at)
    }
}

trait CameraInputs {
    fn require_fresh(&self) -> Result<(), String>;
    async fn wait_until_fresh(&mut self, stop: &mut watch::Receiver<bool>) -> Result<(), String>;
}

impl CameraInputs for [CameraInput] {
    fn require_fresh(&self) -> Result<(), String> {
        self.iter()
            .map(|camera| {
                ObservationFreshness::new(
                    camera.name.clone(),
                    camera
                        .receiver
                        .borrow()
                        .as_ref()
                        .map(|frame| frame.captured_at().elapsed()),
                )
            })
            .collect::<Vec<_>>()
            .require_fresh(INPUT_STALE_AFTER)
            .map_err(|error| error.to_string())
    }

    async fn wait_until_fresh(&mut self, stop: &mut watch::Receiver<bool>) -> Result<(), String> {
        let started = Instant::now();
        loop {
            if *stop.borrow() {
                return Ok(());
            }
            if self.require_fresh().is_ok() {
                return Ok(());
            }
            if started.elapsed() >= INITIAL_CAMERA_TIMEOUT {
                return Err(
                    "camera streams did not produce fresh frames within five seconds".to_owned(),
                );
            }
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return Ok(());
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
        }
    }
}

trait Milliseconds {
    fn milliseconds(self) -> f64;
}

impl Milliseconds for Duration {
    fn milliseconds(self) -> f64 {
        self.as_secs_f64() * 1_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionChunkSize, ActiveRollout, ArtifactDataContract, ArtifactImageContract,
        ArtifactManifest, ArtifactObservationContract, CameraInput, CameraInputs,
        CompletedInference, INPUT_STALE_AFTER, InferenceObservation, InferenceTaskOutput,
        LatestObservationSlot, ObservationSimilarity, RolloutCheckpointRequest, TimedActionQueue,
    };
    use crate::rollout::policy::{
        ACT_ARTIFACT_FORMAT, ACT_TENSOR_NAMESPACE, CameraBindingContract, PolicyDataContract,
        PolicyImageContract, PolicyModelContract, PolicyObservationContract, PolicySchemaContract,
        TargetRobotContract,
    };
    use bytes::Bytes;
    use spiral_robotics_api::rollout::RolloutConfig;
    use spiral_robotics_inference::{
        ActionChunk, ActionChunkShape, ActionChunkSpecification, InferenceInput, InferenceStats,
        ObservationSpecification, Policy, PolicyKind, PolicySpecification,
    };
    use spiral_robotics_storage::checkpoints::{
        CheckpointArtifact, CheckpointFuture, CheckpointKey, CheckpointStore,
    };
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    struct FakeCheckpointStore {
        loads: Mutex<Vec<CheckpointKey>>,
        artifact: CheckpointArtifact,
    }

    struct SlowPolicy {
        specification: PolicySpecification,
        latency: Duration,
    }

    impl SlowPolicy {
        fn new(latency: Duration) -> Self {
            Self {
                specification: PolicySpecification::new(
                    PolicyKind::Act,
                    ObservationSpecification::new(1, Vec::new()),
                    ActionChunkSpecification::new(1, 100, 20),
                ),
                latency,
            }
        }
    }

    impl Policy for SlowPolicy {
        fn specification(&self) -> &PolicySpecification {
            &self.specification
        }

        fn warm_up(&mut self) -> spiral_robotics_inference::InferenceResult<InferenceStats> {
            Ok(InferenceStats::default())
        }

        fn infer(
            &mut self,
            _input: InferenceInput<'_>,
        ) -> spiral_robotics_inference::InferenceResult<ActionChunk> {
            std::thread::sleep(self.latency);
            ActionChunk::try_new(
                vec![0.0; self.specification.action_chunk_size()],
                ActionChunkShape::try_new(
                    self.specification.action_chunk_size(),
                    self.specification.action_dimensions(),
                )?,
                InferenceStats::default()
                    .with_model(self.latency)
                    .with_total(self.latency),
            )
        }
    }

    #[test]
    fn async_action_queue_discards_stale_prefix_and_blends_overlap() {
        let initial = ActionChunk::try_new(
            (0..100).map(|step| step as f32).collect(),
            ActionChunkShape::try_new(100, 1).unwrap(),
            InferenceStats::default(),
        )
        .unwrap();
        let mut queue = TimedActionQueue::from_initial_chunk(&initial).unwrap();
        assert!(queue.latest_action_timestep().is_err());
        for expected in 0..35 {
            assert_eq!(queue.pop().unwrap(), vec![expected as f32]);
        }
        assert_eq!(queue.latest_action_timestep().unwrap(), 34);

        // This observation was captured at timestep 20 and completed at 35,
        // so predictions 0..14 are stale. Timestep 35 overlaps the original
        // plan and is blended; the tail beyond timestep 99 is appended.
        let incoming = ActionChunk::try_new(
            (0..100).map(|step| 2_000.0 + step as f32).collect(),
            ActionChunkShape::try_new(100, 1).unwrap(),
            InferenceStats::default(),
        )
        .unwrap();
        queue.merge(20, &incoming).unwrap();
        assert_eq!(queue.next_timestep(), 35);
        assert_eq!(queue.remaining(), 85);
        assert_eq!(queue.pop().unwrap(), vec![0.3 * 35.0 + 0.7 * 2_015.0]);
        for _ in 36..100 {
            queue.pop().unwrap();
        }
        assert_eq!(queue.pop().unwrap(), vec![2_080.0]);
    }

    #[test]
    fn async_action_queue_accepts_a_stale_chunk_as_empty_for_must_go_recovery() {
        let chunk = ActionChunk::try_new(
            vec![0.0; 100],
            ActionChunkShape::try_new(100, 1).unwrap(),
            InferenceStats::default(),
        )
        .unwrap();
        let mut queue = TimedActionQueue::from_initial_chunk(&chunk).unwrap();
        for _ in 0..100 {
            queue.pop().unwrap();
        }
        queue.merge(0, &chunk).unwrap();
        assert_eq!(queue.remaining(), 0);
        assert_eq!(queue.latest_action_timestep().unwrap(), 99);
    }

    #[test]
    fn action_queue_refills_at_lerobot_default_half_chunk_threshold() {
        assert_eq!(100_usize.refill_threshold(), 50);
        assert_eq!(99_usize.refill_threshold(), 49);
        assert_eq!(10_usize.refill_threshold(), 5);
    }

    #[test]
    fn pending_observation_slot_keeps_only_the_newest_timestep() {
        let mut slot = LatestObservationSlot::default();
        let observation = |requested_timestep| InferenceObservation {
            state: vec![requested_timestep as f32],
            similarity_state: vec![requested_timestep as f32],
            images: Vec::new(),
            requested_timestep,
            oldest_input_at: Instant::now(),
        };

        slot.replace(observation(49));
        slot.replace(observation(57));

        let pending = slot.take().expect("the newest observation stays pending");
        assert_eq!(pending.requested_timestep, 57);
        assert!(slot.take().is_none());
    }

    #[test]
    fn lerobot_observation_similarity_uses_strict_unit_l2_tolerance() {
        assert!([0.5, 0.5].is_similar_to(&[0.0, 0.0]));
        assert!(![1.0, 0.0].is_similar_to(&[0.0, 0.0]));
        assert!(![0.0].is_similar_to(&[0.0, 0.0]));
    }

    #[test]
    fn latency_metrics_include_sensor_age_before_model_execution() {
        let now = Instant::now();
        let inference = CompletedInference {
            chunk: ActionChunk::try_new(
                vec![0.0; 100],
                ActionChunkShape::try_new(100, 1).unwrap(),
                InferenceStats::default(),
            )
            .unwrap(),
            requested_timestep: 0,
            oldest_input_at: now - INPUT_STALE_AFTER - Duration::from_nanos(1),
        };

        assert!(inference.age() > INPUT_STALE_AFTER);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_inference_does_not_pause_policy_action_ticks() {
        let latency = Duration::from_millis(120);
        let now = Instant::now();
        let observation = InferenceObservation {
            state: vec![0.0],
            similarity_state: vec![0.0],
            images: Vec::new(),
            requested_timestep: 0,
            oldest_input_at: now,
        };
        let mut inference = observation.spawn_inference(Box::new(SlowPolicy::new(latency)));
        let mut ticker = tokio::time::interval(Duration::from_millis(10));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let initial = ActionChunk::try_new(
            vec![0.0; 100],
            ActionChunkShape::try_new(100, 1).unwrap(),
            InferenceStats::default(),
        )
        .unwrap();
        let mut actions = TimedActionQueue::from_initial_chunk(&initial).unwrap();
        let mut commands = Vec::new();

        let output = loop {
            tokio::select! {
                output = &mut inference => break output,
                _ = ticker.tick() => {
                    actions.pop().expect("the current chunk keeps feeding actions");
                    commands.push(Instant::now());
                }
            }
        };
        InferenceTaskOutput::resolve_joined(output)
            .expect("slow inference completes on its worker");

        assert!(
            commands.len() >= 8,
            "a 120 ms inference should leave room for many 10 ms action ticks, found {}",
            commands.len()
        );
        assert!(commands.windows(2).all(|window| {
            window[1].saturating_duration_since(window[0]) < Duration::from_millis(60)
        }));
    }

    #[test]
    fn shutdown_waits_for_task_publication_and_cleanup() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let handle = runtime.handle().clone();
        let active = Arc::new(Mutex::new(None));
        let (publisher_holds_lock, publisher_is_released) = mpsc::sync_channel(0);
        let (release_publisher, publisher_release) = mpsc::sync_channel(0);
        let (cleanup_finished, cleanup_observed) = mpsc::sync_channel(0);

        let publisher = std::thread::spawn({
            let active = active.clone();
            move || {
                let mut guard = active.lock().unwrap();
                publisher_holds_lock.send(()).unwrap();
                publisher_release.recv().unwrap();
                let (stop, mut stopped) = tokio::sync::watch::channel(false);
                let task = handle.spawn(async move {
                    stopped.changed().await.unwrap();
                    cleanup_finished.send(()).unwrap();
                });
                debug_assert!(guard.is_none());
                *guard = Some(ActiveRollout::new("session-1".to_owned(), stop, task));
            }
        });

        publisher_is_released.recv().unwrap();
        let (shutdown_attempted, attempt_observed) = mpsc::sync_channel(0);
        let (shutdown_finished, shutdown_observed) = mpsc::sync_channel(0);
        let shutdown = std::thread::spawn({
            let active = active.clone();
            move || {
                shutdown_attempted.send(()).unwrap();
                let active = active.lock().unwrap().take().expect("published rollout");
                runtime.block_on(active.stop_and_join());
                shutdown_finished.send(()).unwrap();
            }
        });

        attempt_observed.recv().unwrap();
        assert!(
            shutdown_observed.try_recv().is_err(),
            "shutdown cannot pass the registration barrier"
        );
        release_publisher.send(()).unwrap();
        cleanup_observed.recv().unwrap();
        shutdown_observed.recv().unwrap();
        publisher.join().unwrap();
        shutdown.join().unwrap();
        assert!(active.lock().unwrap().is_none());
    }

    impl CheckpointStore for FakeCheckpointStore {
        fn load_checkpoint(&self, key: CheckpointKey) -> CheckpointFuture<'_, CheckpointArtifact> {
            self.loads.lock().unwrap().push(key);
            let artifact = self.artifact.clone();
            Box::pin(async move { Ok(artifact) })
        }

        fn delete_run_checkpoints(&self, _run_id: String) -> CheckpointFuture<'_, ()> {
            Box::pin(async { Err("rollout must not delete checkpoints".to_owned()) })
        }
    }

    #[tokio::test]
    async fn rollout_loads_the_selected_run_and_step_without_settings_access() {
        let checkpoint_store = FakeCheckpointStore {
            loads: Mutex::new(Vec::new()),
            artifact: CheckpointArtifact::new(
                br#"{"type":"act"}"#.to_vec(),
                b"opaque model bytes".to_vec(),
            )
            .unwrap(),
        };
        let config = RolloutConfig::new("run-from-project-a", 20_000, "follower").with_duration(30);

        let loaded = config
            .load_checkpoint(&checkpoint_store)
            .await
            .expect("the fake checkpoint_store has no Settings dependency");

        assert_eq!(loaded, checkpoint_store.artifact);
        assert_eq!(
            *checkpoint_store.loads.lock().unwrap(),
            [CheckpointKey::new("run-from-project-a".to_owned(), 20_000)]
        );
    }

    #[test]
    fn camera_roles_resolve_persisted_training_stream_ids() {
        let contract = PolicyDataContract::new(
            PolicyObservationContract::new(
                "centered_normalized",
                "so101_follower",
                vec!["shoulder".to_owned()],
            )
            .with_capture(vec!["follower.camera.wrist".to_owned()], 60.0),
            PolicyImageContract::new("chw", "imagenet", "fixed_fps"),
            PolicySchemaContract::new(
                spiral_robotics_storage::SCHEMA_VERSION,
                spiral_robotics_storage::SCHEMA_VERSION,
            ),
        )
        .validated_with(
            &PolicyModelContract::new(1, 1, vec!["follower.camera.wrist".to_owned()])
                .with_action_shape(10, 1),
            &TargetRobotContract::new(
                "follower",
                Some("so101_follower".to_owned()),
                vec!["shoulder".to_owned()],
            )
            .with_cameras(vec![CameraBindingContract::new("wrist", "camera-1")])
            .with_availability(true, true),
        )
        .unwrap();
        assert_eq!(contract.camera_bindings()[0].camera_id(), "camera-1");
    }

    #[test]
    fn rollout_accepts_the_native_training_artifact_contract() {
        let model_config = serde_json::json!({"type": "act"});
        let rollout_config = RolloutConfig::new("run-1", 10, "follower").with_duration(30);
        let manifest = ArtifactManifest {
            format: ACT_ARTIFACT_FORMAT.to_owned(),
            artifact: "inference".to_owned(),
            run_id: rollout_config.run_id().to_owned(),
            step: rollout_config.checkpoint_step(),
            data_contract: ArtifactDataContract {
                observations: ArtifactObservationContract {
                    value_space: "centered_normalized".to_owned(),
                    robot_type: "so101_follower".to_owned(),
                    joint_names: vec!["shoulder".to_owned()],
                    camera_streams: vec!["wrist".to_owned()],
                    sample_hz: 60.0,
                },
                images: ArtifactImageContract {
                    image_layout: "chw".to_owned(),
                    image_normalization: "imagenet".to_owned(),
                    frame_alignment: "fixed_fps".to_owned(),
                },
                spiral_schema_version: spiral_robotics_storage::SCHEMA_VERSION,
            },
            model_config: model_config.clone(),
            tensor_namespace: ACT_TENSOR_NAMESPACE.to_owned(),
        };
        let config = serde_json::to_vec(&model_config).unwrap();
        assert!(manifest.validate_for(&rollout_config, &config).is_ok());
    }

    #[test]
    fn a_stale_camera_is_rejected_before_an_action_is_sent() {
        let frame = Arc::new(crate::robotics::cameras::Frame::new(
            Bytes::from_static(b"jpeg"),
            1,
            Instant::now() - INPUT_STALE_AFTER - Duration::from_millis(1),
        ));
        let (_sender, receiver) = tokio::sync::watch::channel(Some(frame));
        let cameras = [CameraInput {
            name: "follower.camera.wrist".to_owned(),
            receiver,
        }];
        assert!(cameras.require_fresh().unwrap_err().contains("stale"));
    }
}
