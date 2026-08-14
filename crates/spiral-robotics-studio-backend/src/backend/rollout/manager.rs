//! Backend-owned remote policy lifecycle and deadline-aware motor loop.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::FutureExt as _;
use image::imageops::FilterType;
use safetensors::SafeTensors;
use serde::Deserialize;
use spiral_robotics_contracts::api::{
    ModeSelection, ModeStatus, RobotTelemetry, RolloutMetrics as ContractRolloutMetrics,
    RolloutPhase as ContractRolloutPhase, RolloutSnapshot as ContractRolloutSnapshot,
    StartRolloutRequest,
};
use spiral_robotics_inference::{ActionChunk, ImageView, InferenceInput, Policy, PolicyLoader};
use spiral_robotics_training_http::HttpPolicyLoader;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant as TokioInstant, MissedTickBehavior};

use crate::backend::adapters::clock::SystemClock;
use crate::backend::application::checkpoints::{
    CheckpointArtifact, CheckpointKey, CheckpointRepository,
};
use crate::backend::application::clock::{Clock, StableIdSequence};
use crate::backend::application::policy_runtime::{
    ArtifactIdentity, CameraBindingContract, ControlOwner, JointObservation, ObservationFreshness,
    PolicyDataContract, PolicyModelContract, TargetRobotContract, prepare_action_goals,
    prepare_observation_state, require_fresh_observations, validate_artifact_identity,
    validate_rollout_contract,
};
use crate::backend::application::rollout::{
    CompletionReason, ControlDecision, ExecutionOutcome, RolloutCoordinator, RolloutEventPort,
    RolloutMetrics, RolloutPhase, RolloutRequest, RolloutSnapshot,
};
use crate::backend::robotics::cameras::{CameraManager, Frame};
use crate::backend::robotics::hardware::{HardwareHandle, HardwareState};
use crate::backend::robots::workspace::RobotWorkspace;
use crate::backend::storage::settings::DataSettingsStore;

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
static SESSION_IDS: StableIdSequence = StableIdSequence::new("rollout", 1);

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
    value_space: String,
    robot_type: String,
    joint_names: Vec<String>,
    camera_streams: Vec<String>,
    sample_hz: f64,
    image_layout: String,
    image_normalization: String,
    frame_alignment: String,
    spiral_schema_version: u32,
}

struct ActiveRollout {
    session: String,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

fn publish_active_rollout(
    active: &mut Option<ActiveRollout>,
    session: String,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
) {
    debug_assert!(active.is_none(), "rollout admission is exclusive");
    *active = Some(ActiveRollout {
        session,
        stop,
        task,
    });
}

fn take_active_rollout(active: &Mutex<Option<ActiveRollout>>) -> Option<ActiveRollout> {
    lock(active).take()
}

async fn stop_and_join_active(active: ActiveRollout) {
    let _ = active.stop.send(true);
    let _ = active.task.await;
}

struct WatchRolloutEvents {
    state: watch::Sender<ContractRolloutSnapshot>,
}

impl RolloutEventPort for WatchRolloutEvents {
    fn publish(&self, snapshot: &RolloutSnapshot) {
        self.state.send_replace(contract_snapshot(snapshot));
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

fn observations_similar(candidate: &[f32], previous: &[f32]) -> bool {
    candidate.len() == previous.len()
        && candidate
            .iter()
            .zip(previous)
            .map(|(candidate, previous)| (candidate - previous).powi(2))
            .sum::<f32>()
            < OBSERVATION_SIMILARITY_TOLERANCE.powi(2)
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
        let actions = chunk_actions(chunk)?;
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
        let incoming = chunk_actions(chunk)?;
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
                    |existing| blend_actions(existing, incoming),
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

fn chunk_actions(chunk: &ActionChunk) -> Result<Vec<Vec<f32>>, String> {
    (0..chunk.steps())
        .map(|step| {
            chunk
                .action(step)
                .map(<[f32]>::to_vec)
                .ok_or_else(|| "policy returned an incomplete action sequence".to_owned())
        })
        .collect()
}

fn blend_actions(existing: &[f32], incoming: &[f32]) -> Vec<f32> {
    debug_assert_eq!(existing.len(), incoming.len());
    existing
        .iter()
        .zip(incoming)
        .map(|(old, new)| ACTION_BLEND_OLD_WEIGHT * old + ACTION_BLEND_NEW_WEIGHT * new)
        .collect()
}

fn action_refill_threshold(chunk_steps: usize) -> usize {
    chunk_steps / 2
}

pub(crate) struct RolloutManager {
    checkpoints: Arc<dyn CheckpointRepository>,
    hardware: Arc<HardwareHandle>,
    cameras: Arc<CameraManager>,
    workspace: Arc<RobotWorkspace>,
    settings: Arc<DataSettingsStore>,
    clock: Arc<dyn Clock>,
    state: watch::Sender<ContractRolloutSnapshot>,
    coordinator: Mutex<RolloutCoordinator>,
    active: Mutex<Option<ActiveRollout>>,
}

impl RolloutManager {
    pub(crate) fn new<R>(
        checkpoints: Arc<R>,
        hardware: Arc<HardwareHandle>,
        cameras: Arc<CameraManager>,
        workspace: Arc<RobotWorkspace>,
        settings: Arc<DataSettingsStore>,
    ) -> Arc<Self>
    where
        R: CheckpointRepository + 'static,
    {
        Self::new_with_clock(
            checkpoints,
            hardware,
            cameras,
            workspace,
            settings,
            Arc::new(SystemClock),
        )
    }

    pub(crate) fn new_with_clock<R>(
        checkpoints: Arc<R>,
        hardware: Arc<HardwareHandle>,
        cameras: Arc<CameraManager>,
        workspace: Arc<RobotWorkspace>,
        settings: Arc<DataSettingsStore>,
        clock: Arc<dyn Clock>,
    ) -> Arc<Self>
    where
        R: CheckpointRepository + 'static,
    {
        let (state, receiver) = watch::channel(ContractRolloutSnapshot::default());
        drop(receiver);
        let events = Arc::new(WatchRolloutEvents {
            state: state.clone(),
        });
        Arc::new(Self {
            checkpoints,
            hardware,
            cameras,
            workspace,
            settings,
            clock,
            state,
            coordinator: Mutex::new(RolloutCoordinator::new(events)),
            active: Mutex::new(None),
        })
    }

    pub(crate) fn snapshot(&self) -> ContractRolloutSnapshot {
        self.state.borrow().clone()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<ContractRolloutSnapshot> {
        self.state.subscribe()
    }

    pub(crate) async fn start(
        self: &Arc<Self>,
        request: StartRolloutRequest,
    ) -> Result<ContractRolloutSnapshot, String> {
        let mut active = lock(&self.active);
        let identity = SESSION_IDS.next(self.clock.as_ref());
        let session = identity.value;
        let (stop, stop_receiver) = watch::channel(false);
        lock(&self.coordinator).admit(
            session.clone(),
            identity.created_ms,
            rollout_request(&request),
        )?;
        // Spawn and publish the task handle while holding the same mutex used
        // by shutdown. Once an active session is observable its cleanup task
        // is therefore always joinable; there is no `active + task=None`
        // window for graceful shutdown to miss.
        let manager = self.clone();
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            manager
                .run_and_finish(task_session, request, stop_receiver)
                .await;
        });
        publish_active_rollout(&mut active, session, stop, task);
        Ok(self.snapshot())
    }

    pub(crate) async fn stop(&self) -> Result<(), String> {
        let active = lock(&self.active);
        let Some(active) = active.as_ref() else {
            return Err("no policy rollout is active".to_owned());
        };
        let signal = lock(&self.coordinator).request_stop()?;
        debug_assert_eq!(signal.session, active.session);
        active
            .stop
            .send(true)
            .map_err(|_| "the rollout task already stopped".to_owned())
    }

    pub(crate) async fn shutdown(&self) {
        let task = {
            let Some(active) = take_active_rollout(&self.active) else {
                return;
            };
            let _ = lock(&self.coordinator).request_stop();
            active
        };
        stop_and_join_active(task).await;
    }

    async fn run_and_finish(
        self: Arc<Self>,
        session: String,
        request: StartRolloutRequest,
        stop: watch::Receiver<bool>,
    ) {
        let outcome = match std::panic::AssertUnwindSafe(self.run_session(&session, &request, stop))
            .catch_unwind()
            .await
        {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => ExecutionOutcome::Failed(error),
            Err(_) => ExecutionOutcome::Failed("policy executor panicked unexpectedly".to_owned()),
        };
        let ticket = lock(&self.coordinator).begin_cleanup(&session, outcome);
        // Hardware cleanup is unconditional. Even an internal lifecycle bug
        // must not strand the arm under policy ownership.
        let cleanup = self.hardware.stop_policy_session(&session).await;
        let mut active = lock(&self.active);
        match ticket {
            Ok(ticket) => {
                if let Err(error) =
                    lock(&self.coordinator).finish_cleanup(ticket, self.clock.now_ms(), cleanup)
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

    async fn run_session(
        &self,
        session: &str,
        request: &StartRolloutRequest,
        mut stop: watch::Receiver<bool>,
    ) -> Result<ExecutionOutcome, String> {
        let artifact = load_selected_checkpoint(self.checkpoints.as_ref(), request).await;
        if *stop.borrow() {
            return lock(&self.coordinator).stopped_outcome(session);
        }
        let artifact = artifact?;
        let manifest = parse_manifest(&artifact)?;
        validate_manifest(&manifest, request, &artifact.config_json)?;
        let model_artifact = spiral_robotics_inference::ModelArtifact::from_bytes(
            artifact.config_json,
            artifact.payload,
        )
        .map_err(|error| format!("checkpoint identity is invalid: {error}"))?;
        let policy_loader = HttpPolicyLoader::new(self.settings.get().training.endpoint);
        let policy = tokio::task::spawn_blocking(move || policy_loader.load(model_artifact))
            .await
            .map_err(|error| format!("policy loader stopped unexpectedly: {error}"))?
            .map_err(|error| format!("checkpoint could not be loaded remotely: {error}"))?;
        let specification = policy.specification().clone();
        let hardware_state = self.hardware.state();
        let record = self
            .workspace
            .get(&request.robot)
            .map_err(|error| error.to_string())?;
        let connection = hardware_state
            .connections
            .get(&request.robot)
            .cloned()
            .unwrap_or_default();
        let summary = record.summary(connection);
        let contract = validate_rollout_contract(
            PolicyDataContract {
                value_space: manifest.data_contract.value_space,
                robot_type: manifest.data_contract.robot_type,
                joint_names: manifest.data_contract.joint_names,
                camera_streams: manifest.data_contract.camera_streams,
                sample_hz: manifest.data_contract.sample_hz,
                image_layout: manifest.data_contract.image_layout,
                image_normalization: manifest.data_contract.image_normalization,
                frame_alignment: manifest.data_contract.frame_alignment,
                schema_version: manifest.data_contract.spiral_schema_version,
                supported_schema_version: spiral_robotics_contracts::spiral::SCHEMA_VERSION,
            },
            &PolicyModelContract {
                state_dimensions: specification.state_dimensions,
                action_dimensions: specification.action_dimensions,
                action_chunk_size: specification.action_chunk_size,
                recommended_action_steps: specification.recommended_action_steps,
                image_streams: specification
                    .image_inputs
                    .iter()
                    .map(|image| image.name.clone())
                    .collect(),
            },
            &TargetRobotContract {
                id: summary.id.clone(),
                robot_type: summary.robot_type.clone(),
                joint_names: summary
                    .motors
                    .iter()
                    .map(|motor| motor.name.clone())
                    .collect(),
                camera_bindings: summary
                    .cameras
                    .iter()
                    .map(|binding| CameraBindingContract {
                        role: binding.name.clone(),
                        camera_id: binding.id.clone(),
                    })
                    .collect(),
                connected: summary.connection.connected,
                calibrated: summary.calibrated,
                problem: summary.problem.clone(),
            },
        )
        .map_err(|error| error.to_string())?;
        let joint_names = contract.joint_names;
        let period = contract.sample_period;
        let sample_hz = contract.sample_hz;
        let reobservation_steps = specification
            .action_chunk_size
            .saturating_sub(action_refill_threshold(specification.action_chunk_size));
        let mut cameras = Vec::with_capacity(contract.camera_bindings.len());
        for binding in contract.camera_bindings {
            cameras.push(CameraInput {
                name: binding.stream,
                receiver: self.cameras.subscribe(&binding.camera_id)?,
            });
        }

        if !lock(&self.coordinator).warming_up(session)? {
            return lock(&self.coordinator).stopped_outcome(session);
        }
        let (policy, warmup) = tokio::task::spawn_blocking(move || {
            let mut policy = policy;
            let warmup = policy.warm_up();
            (policy, warmup)
        })
        .await
        .map_err(|error| format!("policy warm-up stopped unexpectedly: {error}"))?;
        let warmup = warmup.map_err(|error| format!("policy setup failed: {error}"))?;
        lock(&self.coordinator).record_warmup(
            session,
            duration_ms(warmup.total),
            duration_ms(warmup.model),
        )?;
        wait_for_camera_frames(&mut cameras, &mut stop).await?;
        if *stop.borrow() {
            return lock(&self.coordinator).stopped_outcome(session);
        }
        if !lock(&self.coordinator).awaiting_control(session)? {
            return lock(&self.coordinator).stopped_outcome(session);
        }

        self.hardware
            .set_mode(ModeSelection::Policy {
                robot: request.robot.clone(),
                session: session.to_owned(),
            })
            .await?;
        let mut hardware_updates = self.hardware.subscribe();
        match wait_for_policy_admission(&mut hardware_updates, &mut stop, &request.robot, session)
            .await?
        {
            PolicyAdmission::Admitted => {}
            PolicyAdmission::Stopped => {
                return lock(&self.coordinator).stopped_outcome(session);
            }
            PolicyAdmission::Replaced(mode) => {
                if let Some(outcome) = self.classify_control(session, &mode)? {
                    return Ok(outcome);
                }
                return Err("policy admission ended without a control transition".to_owned());
            }
        }
        if !lock(&self.coordinator).running(session, sample_hz, reobservation_steps)? {
            return lock(&self.coordinator).stopped_outcome(session);
        }

        // The first chunk is prepared after policy admission but before the
        // rollout clock starts. The follower holds its admitted pose during
        // this one-time fill; once actions start, every later inference runs
        // concurrently with dispatch from the current action queue.
        let initial_hardware = self.hardware.state();
        if let Some(outcome) = self.classify_control(session, &initial_hardware.mode)? {
            return Ok(outcome);
        }
        let initial_robot = policy_robot(&initial_hardware, &request.robot)?;
        require_healthy_policy_robot(initial_robot)?;
        require_fresh_cameras(&cameras)?;
        let initial_state = policy_observation_state(&joint_names, initial_robot)?;
        let initial_similarity_state = lerobot_similarity_state(&joint_names, initial_robot)?;
        let initial_observation = capture_inference_observation(
            initial_state,
            initial_similarity_state,
            &cameras,
            &specification.image_inputs,
            0,
            Instant::now(),
        )?;
        let mut last_processed_similarity_state = initial_observation.similarity_state.clone();
        let mut initial_task = spawn_inference(policy, initial_observation);
        let initial_output = loop {
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return lock(&self.coordinator).stopped_outcome(session);
                    }
                }
                output = &mut initial_task => break output,
            }
        };
        let (initial_policy, initial) = resolve_inference(initial_output)?;
        let initial_latency = completed_inference_age(&initial);
        lock(&self.coordinator).record_inference(
            session,
            duration_ms(initial_latency),
            duration_ms(initial.chunk.stats.model),
        )?;

        let mut policy = Some(initial_policy);
        let mut actions = TimedActionQueue::from_initial_chunk(&initial.chunk)?;
        // LeRobot's async client requests observations once queue_size /
        // action_chunk_size <= 0.5. Integer queue sizes make this floor(N / 2).
        let refill_threshold = action_refill_threshold(initial.chunk.steps());
        let mut inference = None::<InferenceTask>;
        // Equivalent to LeRobot's one-element server observation queue: while
        // a forward pass is busy, every newer eligible observation replaces
        // the previous pending one.
        let mut pending_observation = LatestObservationSlot::default();
        // LeRobot arms one must-go observation whenever an action chunk is
        // received. It is consumed only if that chunk runs completely dry.
        let mut must_go_when_empty = true;
        let deadline =
            TokioInstant::now() + Duration::from_secs(u64::from(request.duration_seconds));
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_hardware_sequence = 0_u64;
        let mut hardware_changed_at = Instant::now();

        loop {
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return lock(&self.coordinator).stopped_outcome(session);
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Ok(ExecutionOutcome::Completed(CompletionReason::DurationReached {
                        duration_seconds: request.duration_seconds,
                    }));
                }
                _ = ticker.tick() => {}
            }
            let cycle_started = Instant::now();
            let hardware = self.hardware.state();
            if hardware.telemetry.sequence != last_hardware_sequence {
                last_hardware_sequence = hardware.telemetry.sequence;
                hardware_changed_at = Instant::now();
            } else if hardware_changed_at.elapsed() > INPUT_STALE_AFTER {
                return Err("stopped policy: hardware telemetry became stale".to_owned());
            }
            if let Some(outcome) = self.classify_control(session, &hardware.mode)? {
                return Ok(outcome);
            }
            let robot = policy_robot(&hardware, &request.robot)?;
            require_healthy_policy_robot(robot)?;
            require_fresh_cameras(&cameras)?;

            // Polling an already-finished blocking task is immediate. While it
            // is still running, this branch does nothing and the buffered queue
            // continues to feed one action on every policy-clock tick.
            if inference.as_ref().is_some_and(JoinHandle::is_finished) {
                let output = inference
                    .take()
                    .expect("a finished inference task is present")
                    .await;
                let (returned_policy, completed) = resolve_inference(output)?;
                let latency = completed_inference_age(&completed);
                lock(&self.coordinator).record_inference(
                    session,
                    duration_ms(latency),
                    duration_ms(completed.chunk.stats.model),
                )?;
                actions.merge(completed.requested_timestep, &completed.chunk)?;
                must_go_when_empty = true;
                policy = Some(returned_policy);
            }

            let action_executed = if actions.remaining() > 0 {
                let action = actions.pop()?;
                let prepared = prepare_action_goals(&joint_names, &action)
                    .map_err(|error| format!("stopped policy: {error}"))?;
                if let Some(clamp) = prepared.clamps.first() {
                    let additional = prepared.clamps.len().saturating_sub(1);
                    let warning = if additional == 0 {
                        format!(
                            "Policy output for joint {:?} exceeded normalized [-1, 1]; the command was clamped to the limit.",
                            clamp.joint
                        )
                    } else {
                        format!(
                            "Policy output for joint {:?} and {additional} other joint{} exceeded normalized [-1, 1]; the commands were clamped to the limits.",
                            clamp.joint,
                            if additional == 1 { "" } else { "s" }
                        )
                    };
                    if lock(&self.coordinator).record_action_clamp_warning(session, warning)? {
                        tracing::warn!(
                            joint = %clamp.joint,
                            index = clamp.index,
                            requested = clamp.requested,
                            applied = clamp.applied,
                            additional_clamps = additional,
                            "clamped policy action outside normalized range"
                        );
                    }
                }

                self.hardware
                    .set_policy_goals(&request.robot, session, prepared.goals)
                    .await?;
                lock(&self.coordinator)
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
                let refreshed = self.hardware.state();
                if let Some(outcome) = self.classify_control(session, &refreshed.mode)? {
                    return Ok(outcome);
                }
                let refreshed_robot = policy_robot(&refreshed, &request.robot)?;
                require_healthy_policy_robot(refreshed_robot)?;
                require_fresh_cameras(&cameras)?;
                let state = policy_observation_state(&joint_names, refreshed_robot)?;
                let similarity_state = lerobot_similarity_state(&joint_names, refreshed_robot)?;
                let latest_action = actions.latest_action_timestep()?;
                let observation = capture_inference_observation(
                    state,
                    similarity_state,
                    &cameras,
                    &specification.image_inputs,
                    latest_action,
                    Instant::now(),
                )?;
                if must_go
                    || !observations_similar(
                        &observation.similarity_state,
                        &last_processed_similarity_state,
                    )
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
                inference = Some(spawn_inference(next_policy, observation));
            }
        }
    }

    fn classify_control(
        &self,
        session: &str,
        mode: &ModeStatus,
    ) -> Result<Option<ExecutionOutcome>, String> {
        let owner = control_owner(&mode.selection);
        match lock(&self.coordinator).observe_control(session, &owner, mode.error.as_deref())? {
            ControlDecision::Continue => Ok(None),
            ControlDecision::Finish(reason) => Ok(Some(ExecutionOutcome::Completed(reason))),
            ControlDecision::Fail(error) => Ok(Some(ExecutionOutcome::Failed(error))),
        }
    }
}

fn rollout_request(request: &StartRolloutRequest) -> RolloutRequest {
    RolloutRequest {
        run_id: request.run_id.clone(),
        checkpoint_step: request.checkpoint_step,
        robot: request.robot.clone(),
        duration_seconds: request.duration_seconds,
    }
}

fn contract_snapshot(snapshot: &RolloutSnapshot) -> ContractRolloutSnapshot {
    ContractRolloutSnapshot {
        phase: match snapshot.phase {
            RolloutPhase::Idle => ContractRolloutPhase::Idle,
            RolloutPhase::Loading => ContractRolloutPhase::Loading,
            RolloutPhase::WarmingUp => ContractRolloutPhase::WarmingUp,
            RolloutPhase::Running => ContractRolloutPhase::Running,
            RolloutPhase::Stopping => ContractRolloutPhase::Stopping,
            RolloutPhase::Failed => ContractRolloutPhase::Failed,
        },
        session_id: snapshot.session_id.clone(),
        run_id: snapshot.run_id.clone(),
        checkpoint_step: snapshot.checkpoint_step,
        robot: snapshot.robot.clone(),
        duration_seconds: snapshot.duration_seconds,
        started_ms: snapshot.started_ms,
        finished_ms: snapshot.finished_ms,
        message: snapshot.message.clone(),
        warning: snapshot.warning.clone(),
        metrics: contract_metrics(&snapshot.metrics),
    }
}

fn contract_metrics(metrics: &RolloutMetrics) -> ContractRolloutMetrics {
    ContractRolloutMetrics {
        inference_count: metrics.inference_count,
        command_count: metrics.command_count,
        missed_deadlines: metrics.missed_deadlines,
        last_latency_ms: metrics.last_latency_ms,
        last_model_ms: metrics.last_model_ms,
        mean_latency_ms: metrics.mean_latency_ms,
        max_latency_ms: metrics.max_latency_ms,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
    Replaced(ModeStatus),
}

async fn wait_for_policy_admission(
    hardware: &mut watch::Receiver<HardwareState>,
    stop: &mut watch::Receiver<bool>,
    expected_robot: &str,
    expected_session: &str,
) -> Result<PolicyAdmission, String> {
    let deadline = TokioInstant::now() + INPUT_STALE_AFTER;
    loop {
        if *stop.borrow() {
            return Ok(PolicyAdmission::Stopped);
        }
        let state = hardware.borrow().clone();
        if matches!(
            &state.mode.selection,
            ModeSelection::Policy { robot, session }
                if robot == expected_robot && session == expected_session
        ) {
            return Ok(PolicyAdmission::Admitted);
        }

        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return Ok(PolicyAdmission::Stopped);
                }
            }
            changed = hardware.changed() => {
                if changed.is_err() {
                    return Err(
                        "stopped policy: hardware telemetry closed before policy control was admitted"
                            .to_owned(),
                    );
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Ok(PolicyAdmission::Replaced(hardware.borrow().mode.clone()));
            }
        }
    }
}

fn control_owner(selection: &ModeSelection) -> ControlOwner {
    match selection {
        ModeSelection::Policy { robot, session } => ControlOwner::Policy {
            robot: robot.clone(),
            session: session.clone(),
        },
        selection => ControlOwner::Other(describe_control_state(selection)),
    }
}

fn describe_control_state(selection: &ModeSelection) -> String {
    match selection {
        ModeSelection::Idle => "Idle".to_owned(),
        ModeSelection::Manual { robot } => format!("Manual on {robot}"),
        ModeSelection::Teleoperate { leader, follower } => {
            format!("Teleoperation from {leader} to {follower}")
        }
        ModeSelection::Replay { robot, episode } => {
            format!("Replay of {episode} on {robot}")
        }
        ModeSelection::Policy { robot, session } => {
            format!("Policy session {session} on {robot}")
        }
    }
}

async fn load_selected_checkpoint(
    repository: &dyn CheckpointRepository,
    request: &StartRolloutRequest,
) -> Result<CheckpointArtifact, String> {
    repository
        .load_checkpoint(CheckpointKey {
            run_id: request.run_id.clone(),
            step: request.checkpoint_step,
        })
        .await
}

fn parse_manifest(artifact: &CheckpointArtifact) -> Result<ArtifactManifest, String> {
    let (_, metadata) = SafeTensors::read_metadata(&artifact.payload)
        .map_err(|error| format!("checkpoint metadata is invalid: {error}"))?;
    let manifest = metadata
        .metadata()
        .as_ref()
        .and_then(|metadata| metadata.get("spiral_robotics"))
        .ok_or_else(|| "checkpoint has no Spiral Robotics manifest".to_owned())?;
    serde_json::from_str(manifest)
        .map_err(|error| format!("checkpoint manifest is invalid: {error}"))
}

fn validate_manifest(
    manifest: &ArtifactManifest,
    request: &StartRolloutRequest,
    config: &[u8],
) -> Result<(), String> {
    validate_artifact_identity(
        &ArtifactIdentity {
            format: manifest.format.clone(),
            purpose: manifest.artifact.clone(),
            run_id: manifest.run_id.clone(),
            step: manifest.step,
            tensor_namespace: manifest.tensor_namespace.clone(),
        },
        &request.run_id,
        request.checkpoint_step,
    )
    .map_err(|error| error.to_string())?;
    let stored_config: serde_json::Value = serde_json::from_slice(config)
        .map_err(|error| format!("stored native model config is invalid: {error}"))?;
    if stored_config != manifest.model_config {
        return Err("stored native model config does not match the checkpoint manifest".to_owned());
    }
    Ok(())
}

async fn wait_for_camera_frames(
    cameras: &mut [CameraInput],
    stop: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        if require_fresh_cameras(cameras).is_ok() {
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

fn policy_robot<'a>(
    hardware: &'a HardwareState,
    robot: &str,
) -> Result<&'a RobotTelemetry, String> {
    hardware
        .telemetry
        .robots
        .iter()
        .find(|candidate| candidate.id == robot)
        .ok_or_else(|| "stopped policy: follower telemetry disappeared".to_owned())
}

fn require_healthy_policy_robot(robot: &RobotTelemetry) -> Result<(), String> {
    if let Some(error) = &robot.error {
        return Err(format!("stopped policy: follower reported {error}"));
    }
    if let Some(joint) = robot.joints.iter().find(|joint| !joint.faults.is_empty()) {
        return Err(format!(
            "stopped policy: joint {:?} reported a motor fault",
            joint.name
        ));
    }
    Ok(())
}

fn policy_observation_state(
    joint_names: &[String],
    robot: &RobotTelemetry,
) -> Result<Vec<f32>, String> {
    prepare_observation_state(
        joint_names,
        &robot
            .joints
            .iter()
            .map(|joint| JointObservation {
                joint: joint.name.clone(),
                normalized: joint.normalized,
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| format!("stopped policy: {error}"))
}

/// Reconstruct the raw joint units used by LeRobot's SO-101 client solely for
/// its near-duplicate observation filter. Center offsets do not affect the
/// distance: body joints use degrees, while the gripper's RANGE_0_100 value is
/// the affine equivalent of this runtime's centered-normalized position.
fn lerobot_similarity_state(
    joint_names: &[String],
    robot: &RobotTelemetry,
) -> Result<Vec<f32>, String> {
    joint_names
        .iter()
        .map(|name| {
            let joint = robot
                .joints
                .iter()
                .find(|joint| joint.name == *name)
                .ok_or_else(|| {
                    format!("stopped policy: joint {name:?} disappeared from live telemetry")
                })?;
            let value = if name == "gripper" {
                joint.normalized.map(|value| (value + 1.0) * 50.0)
            } else {
                joint.degrees
            };
            value
                .filter(|value| value.is_finite())
                .map(|value| value as f32)
                .ok_or_else(|| {
                    format!("stopped policy: joint {name:?} has no finite LeRobot-space telemetry")
                })
        })
        .collect()
}

fn capture_inference_observation(
    state: Vec<f32>,
    similarity_state: Vec<f32>,
    cameras: &[CameraInput],
    specifications: &[spiral_robotics_inference::ImageInputSpecification],
    requested_timestep: u64,
    requested_at: Instant,
) -> Result<InferenceObservation, String> {
    if cameras.len() != specifications.len() {
        return Err(format!(
            "policy camera contract changed from {} inputs to {} specifications",
            cameras.len(),
            specifications.len()
        ));
    }
    let now = Instant::now();
    let mut oldest_input_at = requested_at;
    let images = cameras
        .iter()
        .zip(specifications)
        .map(|(camera, specification)| {
            if camera.name != specification.name {
                return Err(format!(
                    "policy camera order changed from {:?} to {:?}",
                    camera.name, specification.name
                ));
            }
            let frame = camera
                .receiver
                .borrow()
                .clone()
                .ok_or_else(|| format!("camera {:?} has no frame", camera.name))?;
            if now.saturating_duration_since(frame.captured_at) > INPUT_STALE_AFTER {
                return Err(format!("camera {:?} became stale", camera.name));
            }
            oldest_input_at = oldest_input_at.min(frame.captured_at);
            Ok(CapturedImage {
                name: camera.name.clone(),
                width: specification.width,
                height: specification.height,
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

fn spawn_inference(policy: Box<dyn Policy>, observation: InferenceObservation) -> InferenceTask {
    tokio::task::spawn_blocking(move || {
        let mut policy = policy;
        let result = infer_observation(policy.as_mut(), observation);
        InferenceTaskOutput { policy, result }
    })
}

fn infer_observation(
    policy: &mut dyn Policy,
    observation: InferenceObservation,
) -> Result<CompletedInference, String> {
    let InferenceObservation {
        state,
        similarity_state: _,
        images,
        requested_timestep,
        oldest_input_at,
    } = observation;
    let decoded = decode_images(images)?;
    let views = decoded
        .iter()
        .map(|image| ImageView::packed_rgb8(&image.name, image.width, image.height, &image.pixels))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("preparing camera input: {error}"))?;
    let chunk = policy
        .infer(InferenceInput {
            state: &state,
            images: &views,
        })
        .map_err(|error| format!("policy inference failed: {error}"))?;
    Ok(CompletedInference {
        chunk,
        requested_timestep,
        oldest_input_at,
    })
}

fn decode_images(images: Vec<CapturedImage>) -> Result<Vec<DecodedImage>, String> {
    images
        .into_iter()
        .map(|captured| {
            let image = image::load_from_memory(&captured.frame.bytes)
                .map_err(|error| format!("decoding camera {:?}: {error}", captured.name))?
                .to_rgb8();
            let image = image::imageops::resize(
                &image,
                captured.width as u32,
                captured.height as u32,
                FilterType::Triangle,
            );
            Ok(DecodedImage {
                name: captured.name,
                width: captured.width,
                height: captured.height,
                pixels: image.into_raw(),
            })
        })
        .collect()
}

fn resolve_inference(
    output: Result<InferenceTaskOutput, tokio::task::JoinError>,
) -> Result<(Box<dyn Policy>, CompletedInference), String> {
    let output = output.map_err(|error| format!("policy inference worker stopped: {error}"))?;
    output.result.map(|inference| (output.policy, inference))
}

fn completed_inference_age(inference: &CompletedInference) -> Duration {
    Instant::now().saturating_duration_since(inference.oldest_input_at)
}

fn require_fresh_cameras(cameras: &[CameraInput]) -> Result<(), String> {
    require_fresh_observations(
        &cameras
            .iter()
            .map(|camera| ObservationFreshness {
                source: camera.name.clone(),
                age: camera
                    .receiver
                    .borrow()
                    .as_ref()
                    .map(|frame| frame.captured_at.elapsed()),
            })
            .collect::<Vec<_>>(),
        INPUT_STALE_AFTER,
    )
    .map_err(|error| error.to_string())
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactDataContract, ArtifactManifest, CameraInput, CompletedInference, INPUT_STALE_AFTER,
        InferenceObservation, LatestObservationSlot, PolicyAdmission, TimedActionQueue,
        action_refill_threshold, completed_inference_age, load_selected_checkpoint,
        observations_similar, publish_active_rollout, require_fresh_cameras, resolve_inference,
        spawn_inference, stop_and_join_active, take_active_rollout, validate_manifest,
        wait_for_policy_admission,
    };
    use crate::backend::application::checkpoints::{
        CheckpointArtifact, CheckpointFuture, CheckpointKey, CheckpointPublication,
        CheckpointRepository, CheckpointSource,
    };
    use crate::backend::application::policy_runtime::{
        ACT_ARTIFACT_FORMAT, ACT_TENSOR_NAMESPACE, CameraBindingContract, PolicyDataContract,
        PolicyModelContract, TargetRobotContract, validate_rollout_contract,
    };
    use crate::backend::robotics::hardware::HardwareState;
    use bytes::Bytes;
    use spiral_robotics_contracts::api::{ModeSelection, ModeStatus, StartRolloutRequest};
    use spiral_robotics_inference::{
        ActionChunk, InferenceInput, InferenceStats, ModelIdentity, Policy, PolicyKind,
        PolicySpecification,
    };
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    struct FakeCheckpointRepository {
        loads: Mutex<Vec<CheckpointKey>>,
        artifact: CheckpointArtifact,
    }

    struct SlowPolicy {
        identity: ModelIdentity,
        specification: PolicySpecification,
        latency: Duration,
    }

    impl SlowPolicy {
        fn new(latency: Duration) -> Self {
            Self {
                identity: ModelIdentity::local(),
                specification: PolicySpecification {
                    kind: PolicyKind::Act,
                    state_dimensions: 1,
                    image_inputs: Vec::new(),
                    action_dimensions: 1,
                    action_chunk_size: 100,
                    recommended_action_steps: 20,
                },
                latency,
            }
        }
    }

    impl Policy for SlowPolicy {
        fn identity(&self) -> &ModelIdentity {
            &self.identity
        }

        fn specification(&self) -> &PolicySpecification {
            &self.specification
        }

        fn warm_up(&mut self) -> spiral_robotics_inference::Result<InferenceStats> {
            Ok(InferenceStats::default())
        }

        fn infer(
            &mut self,
            _input: InferenceInput<'_>,
        ) -> spiral_robotics_inference::Result<ActionChunk> {
            std::thread::sleep(self.latency);
            ActionChunk::try_new(
                vec![0.0; self.specification.action_chunk_size],
                self.specification.action_chunk_size,
                self.specification.action_dimensions,
                InferenceStats {
                    model: self.latency,
                    total: self.latency,
                    ..InferenceStats::default()
                },
            )
        }
    }

    #[test]
    fn async_action_queue_discards_stale_prefix_and_blends_overlap() {
        let initial = ActionChunk::try_new(
            (0..100).map(|step| step as f32).collect(),
            100,
            1,
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
            100,
            1,
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
        let chunk =
            ActionChunk::try_new(vec![0.0; 100], 100, 1, InferenceStats::default()).unwrap();
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
        assert_eq!(action_refill_threshold(100), 50);
        assert_eq!(action_refill_threshold(99), 49);
        assert_eq!(action_refill_threshold(10), 5);
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
        assert!(observations_similar(&[0.5, 0.5], &[0.0, 0.0]));
        assert!(!observations_similar(&[1.0, 0.0], &[0.0, 0.0]));
        assert!(!observations_similar(&[0.0], &[0.0, 0.0]));
    }

    #[test]
    fn latency_metrics_include_sensor_age_before_model_execution() {
        let now = Instant::now();
        let inference = CompletedInference {
            chunk: ActionChunk::try_new(vec![0.0; 100], 100, 1, InferenceStats::default()).unwrap(),
            requested_timestep: 0,
            oldest_input_at: now - INPUT_STALE_AFTER - Duration::from_nanos(1),
        };

        assert!(completed_inference_age(&inference) > INPUT_STALE_AFTER);
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
        let mut inference = spawn_inference(Box::new(SlowPolicy::new(latency)), observation);
        let mut ticker = tokio::time::interval(Duration::from_millis(10));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let initial =
            ActionChunk::try_new(vec![0.0; 100], 100, 1, InferenceStats::default()).unwrap();
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
        resolve_inference(output).expect("slow inference completes on its worker");

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
                publish_active_rollout(&mut guard, "session-1".to_owned(), stop, task);
            }
        });

        publisher_is_released.recv().unwrap();
        let (shutdown_attempted, attempt_observed) = mpsc::sync_channel(0);
        let (shutdown_finished, shutdown_observed) = mpsc::sync_channel(0);
        let shutdown = std::thread::spawn({
            let active = active.clone();
            move || {
                shutdown_attempted.send(()).unwrap();
                let active = take_active_rollout(&active).expect("published rollout");
                runtime.block_on(stop_and_join_active(active));
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

    impl CheckpointRepository for FakeCheckpointRepository {
        fn register_run_source(
            &self,
            _run_id: &str,
            _source: CheckpointSource,
        ) -> Result<(), String> {
            Err("rollout must not register training sources".to_owned())
        }

        fn publish_checkpoint(
            &self,
            _publication: CheckpointPublication,
        ) -> CheckpointFuture<'_, ()> {
            Box::pin(async { Err("rollout must not publish checkpoints".to_owned()) })
        }

        fn load_checkpoint(&self, key: CheckpointKey) -> CheckpointFuture<'_, CheckpointArtifact> {
            self.loads.lock().unwrap().push(key);
            let artifact = self.artifact.clone();
            Box::pin(async move { Ok(artifact) })
        }

        fn delete_run_checkpoints(
            &self,
            _run_id: String,
            _source: CheckpointSource,
        ) -> CheckpointFuture<'_, ()> {
            Box::pin(async { Err("rollout must not delete checkpoints".to_owned()) })
        }
    }

    #[tokio::test]
    async fn rollout_loads_the_selected_run_and_step_without_settings_access() {
        let repository = FakeCheckpointRepository {
            loads: Mutex::new(Vec::new()),
            artifact: CheckpointArtifact {
                config_json: br#"{"type":"act"}"#.to_vec(),
                payload: b"opaque model bytes".to_vec(),
            },
        };
        let request = StartRolloutRequest {
            run_id: "run-from-project-a".to_owned(),
            checkpoint_step: 20_000,
            robot: "follower".to_owned(),
            duration_seconds: 30,
        };

        let loaded = load_selected_checkpoint(&repository, &request)
            .await
            .expect("the fake repository has no Settings dependency");

        assert_eq!(loaded, repository.artifact);
        assert_eq!(
            *repository.loads.lock().unwrap(),
            [CheckpointKey {
                run_id: "run-from-project-a".to_owned(),
                step: 20_000,
            }]
        );
    }

    #[test]
    fn camera_roles_resolve_persisted_training_stream_ids() {
        let contract = validate_rollout_contract(
            PolicyDataContract {
                value_space: "centered_normalized".to_owned(),
                robot_type: "so101_follower".to_owned(),
                joint_names: vec!["shoulder".to_owned()],
                camera_streams: vec!["follower.camera.wrist".to_owned()],
                sample_hz: 60.0,
                image_layout: "chw".to_owned(),
                image_normalization: "imagenet".to_owned(),
                frame_alignment: "fixed_fps".to_owned(),
                schema_version: spiral_robotics_contracts::spiral::SCHEMA_VERSION,
                supported_schema_version: spiral_robotics_contracts::spiral::SCHEMA_VERSION,
            },
            &PolicyModelContract {
                state_dimensions: 1,
                action_dimensions: 1,
                action_chunk_size: 10,
                recommended_action_steps: 1,
                image_streams: vec!["follower.camera.wrist".to_owned()],
            },
            &TargetRobotContract {
                id: "follower".to_owned(),
                robot_type: Some("so101_follower".to_owned()),
                joint_names: vec!["shoulder".to_owned()],
                camera_bindings: vec![CameraBindingContract {
                    role: "wrist".to_owned(),
                    camera_id: "camera-1".to_owned(),
                }],
                connected: true,
                calibrated: true,
                problem: None,
            },
        )
        .unwrap();
        assert_eq!(contract.camera_bindings[0].camera_id, "camera-1");
    }

    #[tokio::test]
    async fn policy_start_waits_for_the_control_thread_to_publish_admission() {
        let (publisher, mut hardware) = tokio::sync::watch::channel(HardwareState::default());
        let (_stop_sender, mut stop) = tokio::sync::watch::channel(false);
        let publish = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            publisher.send_modify(|state| {
                state.mode = ModeStatus {
                    selection: ModeSelection::Policy {
                        robot: "follower".to_owned(),
                        session: "rollout-1".to_owned(),
                    },
                    active: true,
                    error: None,
                };
            });
        });

        assert!(matches!(
            wait_for_policy_admission(&mut hardware, &mut stop, "follower", "rollout-1").await,
            Ok(PolicyAdmission::Admitted)
        ));
        publish.await.expect("policy publication task");
    }

    #[test]
    fn rollout_accepts_the_native_training_artifact_contract() {
        let model_config = serde_json::json!({"type": "act"});
        let request = StartRolloutRequest {
            run_id: "run-1".to_owned(),
            checkpoint_step: 10,
            robot: "follower".to_owned(),
            duration_seconds: 30,
        };
        let manifest = ArtifactManifest {
            format: ACT_ARTIFACT_FORMAT.to_owned(),
            artifact: "inference".to_owned(),
            run_id: request.run_id.clone(),
            step: request.checkpoint_step,
            data_contract: ArtifactDataContract {
                value_space: "centered_normalized".to_owned(),
                robot_type: "so101_follower".to_owned(),
                joint_names: vec!["shoulder".to_owned()],
                camera_streams: vec!["wrist".to_owned()],
                sample_hz: 60.0,
                image_layout: "chw".to_owned(),
                image_normalization: "imagenet".to_owned(),
                frame_alignment: "fixed_fps".to_owned(),
                spiral_schema_version: spiral_robotics_contracts::spiral::SCHEMA_VERSION,
            },
            model_config: model_config.clone(),
            tensor_namespace: ACT_TENSOR_NAMESPACE.to_owned(),
        };
        let config = serde_json::to_vec(&model_config).unwrap();
        assert!(validate_manifest(&manifest, &request, &config).is_ok());
    }

    #[test]
    fn a_stale_camera_is_rejected_before_an_action_is_sent() {
        let frame = Arc::new(crate::backend::robotics::cameras::Frame {
            bytes: Bytes::from_static(b"jpeg"),
            sequence: 1,
            captured_at: Instant::now() - INPUT_STALE_AFTER - Duration::from_millis(1),
        });
        let (_sender, receiver) = tokio::sync::watch::channel(Some(frame));
        let cameras = [CameraInput {
            name: "follower.camera.wrist".to_owned(),
            receiver,
        }];
        assert!(
            require_fresh_cameras(&cameras)
                .unwrap_err()
                .contains("stale")
        );
    }
}
