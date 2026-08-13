//! Backend-owned native policy lifecycle and deadline-aware motor loop.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::FutureExt as _;
use image::imageops::FilterType;
use safetensors::SafeTensors;
use serde::Deserialize;
use spiral_robotics_contracts::api::{
    ModeSelection, ModeStatus, RolloutMetrics as ContractRolloutMetrics,
    RolloutPhase as ContractRolloutPhase, RolloutSnapshot as ContractRolloutSnapshot,
    StartRolloutRequest,
};
use spiral_robotics_inference::{ImageView, InferenceInput, ModelArtifact, Policy};
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

const INPUT_STALE_AFTER: Duration = Duration::from_millis(350);
const INITIAL_CAMERA_TIMEOUT: Duration = Duration::from_secs(5);
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

struct DecodedImage {
    name: String,
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

pub(crate) struct RolloutManager {
    checkpoints: Arc<dyn CheckpointRepository>,
    hardware: Arc<HardwareHandle>,
    cameras: Arc<CameraManager>,
    workspace: Arc<RobotWorkspace>,
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
    ) -> Arc<Self>
    where
        R: CheckpointRepository + 'static,
    {
        Self::new_with_clock(
            checkpoints,
            hardware,
            cameras,
            workspace,
            Arc::new(SystemClock),
        )
    }

    pub(crate) fn new_with_clock<R>(
        checkpoints: Arc<R>,
        hardware: Arc<HardwareHandle>,
        cameras: Arc<CameraManager>,
        workspace: Arc<RobotWorkspace>,
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
        let model_artifact = ModelArtifact::from_bytes(artifact.config_json, artifact.payload)
            .map_err(|error| format!("checkpoint identity is invalid: {error}"))?;
        let (mut policy, _backend): (Box<dyn Policy>, &'static str) =
            tokio::task::spawn_blocking(move || load_native_policy(model_artifact))
                .await
                .map_err(|error| format!("policy loader stopped unexpectedly: {error}"))?
                .map_err(|error| format!("checkpoint could not be loaded: {error}"))?;
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
        let action_steps = contract.action_steps;
        let period = contract.sample_period;
        let sample_hz = contract.sample_hz;
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
        let warmup = tokio::task::block_in_place(|| policy.warm_up())
            .map_err(|error| format!("policy setup failed: {error}"))?;
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
        if !lock(&self.coordinator).running(session, sample_hz, action_steps)? {
            return lock(&self.coordinator).stopped_outcome(session);
        }

        let deadline =
            TokioInstant::now() + Duration::from_secs(u64::from(request.duration_seconds));
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut chunk = None;
        let mut action_index = 0_usize;
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
            let robot = hardware
                .telemetry
                .robots
                .iter()
                .find(|robot| robot.id == request.robot)
                .ok_or_else(|| "stopped policy: follower telemetry disappeared".to_owned())?;
            if let Some(error) = &robot.error {
                return Err(format!("stopped policy: follower reported {error}"));
            }
            if let Some(joint) = robot.joints.iter().find(|joint| !joint.faults.is_empty()) {
                return Err(format!(
                    "stopped policy: joint {:?} reported a motor fault",
                    joint.name
                ));
            }
            require_fresh_cameras(&cameras)?;
            let state = prepare_observation_state(
                &joint_names,
                &robot
                    .joints
                    .iter()
                    .map(|joint| JointObservation {
                        joint: joint.name.clone(),
                        normalized: joint.normalized,
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| format!("stopped policy: {error}"))?;

            if chunk.is_none() || action_index >= action_steps {
                let decoded = decode_images(&cameras, &specification.image_inputs)?;
                let views = decoded
                    .iter()
                    .map(|image| {
                        ImageView::packed_rgb8(
                            &image.name,
                            image.width,
                            image.height,
                            &image.pixels,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| format!("preparing camera input: {error}"))?;
                let inference = tokio::task::block_in_place(|| {
                    policy.infer(InferenceInput {
                        state: &state,
                        images: &views,
                    })
                })
                .map_err(|error| format!("policy inference failed: {error}"))?;
                if inference.stats.total > INPUT_STALE_AFTER {
                    return Err(format!(
                        "stopped policy: {:.1} ms inference latency exceeded the {:.0} ms observation safety window",
                        duration_ms(inference.stats.total),
                        duration_ms(INPUT_STALE_AFTER)
                    ));
                }
                let refreshed = self.hardware.state();
                if let Some(outcome) = self.classify_control(session, &refreshed.mode)? {
                    return Ok(outcome);
                }
                let refreshed_robot = refreshed
                    .telemetry
                    .robots
                    .iter()
                    .find(|robot| robot.id == request.robot)
                    .ok_or_else(|| {
                        "stopped policy: follower telemetry disappeared during inference".to_owned()
                    })?;
                if refreshed_robot.error.is_some()
                    || refreshed_robot
                        .joints
                        .iter()
                        .any(|joint| !joint.faults.is_empty())
                {
                    return Err("stopped policy: follower faulted during inference".to_owned());
                }
                require_fresh_cameras(&cameras)?;
                let latency = duration_ms(inference.stats.total);
                lock(&self.coordinator).record_inference(
                    session,
                    latency,
                    duration_ms(inference.stats.model),
                )?;
                chunk = Some(inference);
                action_index = 0;
            }
            let action = chunk
                .as_ref()
                .and_then(|chunk| chunk.action(action_index))
                .ok_or_else(|| "policy returned an incomplete action sequence".to_owned())?;
            let prepared = prepare_action_goals(&joint_names, action)
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
            action_index += 1;
            lock(&self.coordinator).record_command(session, cycle_started.elapsed() > period)?;
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

fn decode_images(
    cameras: &[CameraInput],
    specifications: &[spiral_robotics_inference::ImageInputSpecification],
) -> Result<Vec<DecodedImage>, String> {
    cameras
        .iter()
        .zip(specifications)
        .map(|(camera, specification)| {
            let frame = camera
                .receiver
                .borrow()
                .clone()
                .ok_or_else(|| format!("camera {:?} has no frame", camera.name))?;
            if frame.captured_at.elapsed() > INPUT_STALE_AFTER {
                return Err(format!("camera {:?} became stale", camera.name));
            }
            let image = image::load_from_memory(&frame.bytes)
                .map_err(|error| format!("decoding camera {:?}: {error}", camera.name))?
                .to_rgb8();
            let image = image::imageops::resize(
                &image,
                specification.width as u32,
                specification.height as u32,
                FilterType::Triangle,
            );
            Ok(DecodedImage {
                name: camera.name.clone(),
                width: specification.width,
                height: specification.height,
                pixels: image.into_raw(),
            })
        })
        .collect()
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

#[cfg(target_os = "macos")]
fn load_native_policy(
    artifact: ModelArtifact,
) -> spiral_robotics_inference::Result<(Box<dyn Policy>, &'static str)> {
    if !spiral_robotics_inference::act::metal_available() {
        tracing::warn!("Metal rollout backend unavailable; using Accelerate CPU");
        return spiral_robotics_inference::act::load_cpu_artifact(artifact)
            .map(|policy| (Box::new(policy) as Box<dyn Policy>, "Accelerate CPU"));
    }

    // ModelArtifact clones share immutable bytes, so preserving the CPU
    // fallback does not duplicate the checkpoint allocation.
    let fallback = artifact.clone();
    let metal = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        spiral_robotics_inference::act::load_metal_artifact(artifact)
    }));
    match metal {
        Ok(Ok(policy)) => Ok((Box::new(policy) as Box<dyn Policy>, "Metal")),
        Ok(Err(error)) => {
            tracing::warn!("Metal rollout backend unavailable ({error}); using Accelerate CPU");
            spiral_robotics_inference::act::load_cpu_artifact(fallback)
                .map(|policy| (Box::new(policy) as Box<dyn Policy>, "Accelerate CPU"))
        }
        Err(_) => {
            tracing::warn!("Metal rollout backend panicked during discovery; using Accelerate CPU");
            spiral_robotics_inference::act::load_cpu_artifact(fallback)
                .map(|policy| (Box::new(policy) as Box<dyn Policy>, "Accelerate CPU"))
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn load_native_policy(
    artifact: ModelArtifact,
) -> spiral_robotics_inference::Result<(Box<dyn Policy>, &'static str)> {
    spiral_robotics_inference::act::load_cpu_artifact(artifact)
        .map(|policy| (Box::new(policy) as Box<dyn Policy>, "CPU"))
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactDataContract, ArtifactManifest, CameraInput, INPUT_STALE_AFTER, PolicyAdmission,
        load_selected_checkpoint, publish_active_rollout, require_fresh_cameras,
        stop_and_join_active, take_active_rollout, validate_manifest, wait_for_policy_admission,
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
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    struct FakeCheckpointRepository {
        loads: Mutex<Vec<CheckpointKey>>,
        artifact: CheckpointArtifact,
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
