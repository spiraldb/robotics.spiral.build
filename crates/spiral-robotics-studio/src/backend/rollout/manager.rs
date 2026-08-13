//! Backend-owned native policy lifecycle and deadline-aware motor loop.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use image::imageops::FilterType;
use safetensors::SafeTensors;
use serde::Deserialize;
use spiral_robotics_core::api::{
    MAX_ROLLOUT_DURATION_SECONDS, ModeSelection, ModeStatus, RolloutPhase, RolloutSnapshot,
    StartRolloutRequest,
};
use spiral_robotics_inference::{ImageView, InferenceInput, ModelArtifact, Policy};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant as TokioInstant, MissedTickBehavior};

use crate::backend::robotics::cameras::{CameraManager, Frame};
use crate::backend::robotics::hardware::{HardwareHandle, HardwareState};
use crate::backend::robots::workspace::RobotWorkspace;
use crate::backend::storage::manager::{StorageManager, TrainingCheckpointArtifact};
use crate::backend::training::{ACT_ARTIFACT_FORMAT, ACT_TENSOR_NAMESPACE};

const INPUT_STALE_AFTER: Duration = Duration::from_millis(350);
const INITIAL_CAMERA_TIMEOUT: Duration = Duration::from_secs(5);
static SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
    robot_type: String,
    joint_names: Vec<String>,
    camera_streams: Vec<String>,
    sample_hz: f64,
}

struct ActiveRollout {
    session: String,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
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
    storage: Arc<StorageManager>,
    hardware: Arc<HardwareHandle>,
    cameras: Arc<CameraManager>,
    workspace: Arc<RobotWorkspace>,
    state: watch::Sender<RolloutSnapshot>,
    active: AsyncMutex<Option<ActiveRollout>>,
}

impl RolloutManager {
    pub(crate) fn new(
        storage: Arc<StorageManager>,
        hardware: Arc<HardwareHandle>,
        cameras: Arc<CameraManager>,
        workspace: Arc<RobotWorkspace>,
    ) -> Arc<Self> {
        let (state, receiver) = watch::channel(RolloutSnapshot::default());
        drop(receiver);
        Arc::new(Self {
            storage,
            hardware,
            cameras,
            workspace,
            state,
            active: AsyncMutex::new(None),
        })
    }

    pub(crate) fn snapshot(&self) -> RolloutSnapshot {
        self.state.borrow().clone()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<RolloutSnapshot> {
        self.state.subscribe()
    }

    pub(crate) async fn start(
        self: &Arc<Self>,
        request: StartRolloutRequest,
    ) -> Result<RolloutSnapshot, String> {
        validate_request(&request)?;
        let mut active = self.active.lock().await;
        if let Some(existing) = active.as_ref() {
            return Err(format!(
                "policy session {:?} is already active",
                existing.session
            ));
        }
        let session = format!(
            "rollout-{}-{}",
            now_ms(),
            SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let (stop, stop_receiver) = watch::channel(false);
        *active = Some(ActiveRollout {
            session: session.clone(),
            stop,
            task: None,
        });
        self.state.send_replace(RolloutSnapshot {
            phase: RolloutPhase::Loading,
            session_id: Some(session.clone()),
            run_id: Some(request.run_id.clone()),
            checkpoint_step: Some(request.checkpoint_step),
            robot: Some(request.robot.clone()),
            duration_seconds: Some(request.duration_seconds),
            started_ms: Some(now_ms()),
            message: Some("Loading checkpoint…".to_owned()),
            ..RolloutSnapshot::default()
        });

        let manager = self.clone();
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            manager
                .run_and_finish(task_session, request, stop_receiver)
                .await;
        });
        if let Some(current) = active.as_mut().filter(|item| item.session == session) {
            current.task = Some(task);
        } else {
            task.abort();
        }
        Ok(self.snapshot())
    }

    pub(crate) async fn stop(&self) -> Result<(), String> {
        let active = self.active.lock().await;
        let Some(active) = active.as_ref() else {
            return Err("no policy rollout is active".to_owned());
        };
        self.update_session(&active.session, |snapshot| {
            snapshot.phase = RolloutPhase::Stopping;
            snapshot.message = Some("Stopping after the current control cycle…".to_owned());
        });
        active
            .stop
            .send(true)
            .map_err(|_| "the rollout task already stopped".to_owned())
    }

    pub(crate) async fn shutdown(&self) {
        let active = self.active.lock().await.take();
        let Some(mut active) = active else {
            return;
        };
        let _ = active.stop.send(true);
        if let Some(task) = active.task.take() {
            let _ = task.await;
        }
        let _ = self.hardware.stop_policy_session(&active.session).await;
    }

    async fn run_and_finish(
        self: Arc<Self>,
        session: String,
        request: StartRolloutRequest,
        stop: watch::Receiver<bool>,
    ) {
        let result = self.run_session(&session, &request, stop).await;
        let _ = self.hardware.stop_policy_session(&session).await;
        match result {
            Ok(message) => self.update_session(&session, |snapshot| {
                snapshot.phase = RolloutPhase::Idle;
                snapshot.finished_ms = Some(now_ms());
                snapshot.message = Some(message);
            }),
            Err(error) => self.update_session(&session, |snapshot| {
                snapshot.phase = RolloutPhase::Failed;
                snapshot.finished_ms = Some(now_ms());
                snapshot.message = Some(error);
            }),
        }
        let mut active = self.active.lock().await;
        if active.as_ref().is_some_and(|item| item.session == session) {
            active.take();
        }
    }

    async fn run_session(
        &self,
        session: &str,
        request: &StartRolloutRequest,
        mut stop: watch::Receiver<bool>,
    ) -> Result<String, String> {
        let artifact = self
            .storage
            .training_checkpoint(&request.run_id, request.checkpoint_step)
            .await?;
        if *stop.borrow() {
            return Ok("Stopped before model loading.".to_owned());
        }
        let manifest = parse_manifest(&artifact)?;
        validate_manifest(&manifest, request, &artifact.config_json)?;
        let (joint_names, camera_streams, robot_type) = (
            manifest.data_contract.joint_names.clone(),
            manifest.data_contract.camera_streams.clone(),
            manifest.data_contract.robot_type.clone(),
        );
        let sample_hz = manifest.data_contract.sample_hz;
        let model_artifact = ModelArtifact::from_bytes(artifact.config_json, artifact.payload)
            .map_err(|error| format!("checkpoint identity is invalid: {error}"))?;
        let (mut policy, _backend): (Box<dyn Policy>, &'static str) =
            tokio::task::spawn_blocking(move || load_native_policy(model_artifact))
                .await
                .map_err(|error| format!("policy loader stopped unexpectedly: {error}"))?
                .map_err(|error| format!("checkpoint could not be loaded: {error}"))?;
        let specification = policy.specification().clone();
        if specification.state_dimensions != joint_names.len()
            || specification.action_dimensions != joint_names.len()
        {
            return Err(format!(
                "checkpoint joint contract has {} names but the model expects {} state and {} action values",
                joint_names.len(),
                specification.state_dimensions,
                specification.action_dimensions
            ));
        }
        let action_steps = specification.recommended_action_steps;
        if action_steps == 0 || action_steps > specification.action_chunk_size {
            return Err(format!(
                "checkpoint recommends {action_steps} action steps for chunk size {}",
                specification.action_chunk_size
            ));
        }
        let model_images = specification
            .image_inputs
            .iter()
            .map(|image| image.name.as_str())
            .collect::<Vec<_>>();
        if model_images
            != camera_streams
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        {
            return Err("checkpoint camera order does not match its configuration".to_owned());
        }

        let hardware_state = self.hardware.state();
        let record = self
            .workspace
            .get(&request.robot)
            .map_err(|error| error.to_string())?;
        if record.robot_type().as_deref() != Some(robot_type.as_str()) {
            return Err(format!(
                "checkpoint was trained for {robot_type:?}, but robot {:?} is {:?}",
                request.robot,
                record.robot_type()
            ));
        }
        let connection = hardware_state
            .connections
            .get(&request.robot)
            .cloned()
            .unwrap_or_default();
        let summary = record.summary(connection);
        if summary.problem.is_some() || !summary.connection.connected || !summary.calibrated {
            return Err(format!(
                "robot {:?} must be valid, connected, and calibrated before rollout",
                request.robot
            ));
        }
        let configured_joints = summary
            .motors
            .iter()
            .map(|motor| motor.name.as_str())
            .collect::<BTreeSet<_>>();
        if configured_joints != joint_names.iter().map(String::as_str).collect() {
            return Err("checkpoint joint names do not exactly match the target robot".to_owned());
        }
        let mut cameras = Vec::with_capacity(specification.image_inputs.len());
        for image in &specification.image_inputs {
            let binding = summary
                .cameras
                .iter()
                .find(|binding| camera_matches(&image.name, &binding.name))
                .ok_or_else(|| {
                    format!(
                        "robot {:?} has no camera binding for checkpoint stream {:?}",
                        request.robot, image.name
                    )
                })?;
            cameras.push(CameraInput {
                name: image.name.clone(),
                receiver: self.cameras.subscribe(&binding.id)?,
            });
        }

        self.update_session(session, |snapshot| {
            snapshot.phase = RolloutPhase::WarmingUp;
            snapshot.message = Some("Preparing policy and waiting for cameras…".to_owned());
        });
        let warmup = tokio::task::block_in_place(|| policy.warm_up())
            .map_err(|error| format!("policy setup failed: {error}"))?;
        self.update_session(session, |snapshot| {
            snapshot.metrics.last_latency_ms = Some(duration_ms(warmup.total));
            snapshot.metrics.last_model_ms = Some(duration_ms(warmup.model));
        });
        wait_for_camera_frames(&mut cameras, &mut stop).await?;
        if *stop.borrow() {
            return Ok("Stopped before the arm was enabled.".to_owned());
        }

        self.hardware
            .set_mode(ModeSelection::Policy {
                robot: request.robot.clone(),
                session: session.to_owned(),
            })
            .await?;
        let mut hardware_updates = self.hardware.subscribe();
        if let Some(message) =
            wait_for_policy_admission(&mut hardware_updates, &request.robot, session).await?
        {
            return Ok(message);
        }
        self.update_session(session, |snapshot| {
            snapshot.phase = RolloutPhase::Running;
            snapshot.message = Some(format!(
                "Controlling the follower at {sample_hz:.1} Hz · {action_steps} actions per observation · {} maximum.",
                format_duration(request.duration_seconds)
            ));
        });

        let period = Duration::from_secs_f64(1.0 / sample_hz);
        let deadline =
            TokioInstant::now() + Duration::from_secs(u64::from(request.duration_seconds));
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut chunk = None;
        let mut action_index = 0_usize;
        let mut last_hardware_sequence = 0_u64;
        let mut hardware_changed_at = Instant::now();
        let mut latency_sum = 0.0;
        let mut latency_max = 0.0_f64;

        loop {
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return Ok("Policy stopped safely; the follower is holding its current position.".to_owned());
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Ok(format!(
                        "Policy completed its configured {} duration; the follower is holding its current position.",
                        format_duration(request.duration_seconds)
                    ));
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
            if let Some(message) = policy_exit(&hardware.mode, &request.robot, session)? {
                return Ok(message);
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
            let state = ordered_state(robot, &joint_names)?;

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
                if let Some(message) = policy_exit(&refreshed.mode, &request.robot, session)? {
                    return Ok(message);
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
                latency_sum += latency;
                latency_max = latency_max.max(latency);
                self.update_session(session, |snapshot| {
                    snapshot.metrics.inference_count += 1;
                    snapshot.metrics.last_latency_ms = Some(latency);
                    snapshot.metrics.last_model_ms = Some(duration_ms(inference.stats.model));
                    snapshot.metrics.mean_latency_ms =
                        Some(latency_sum / snapshot.metrics.inference_count as f64);
                    snapshot.metrics.max_latency_ms = Some(latency_max);
                });
                chunk = Some(inference);
                action_index = 0;
            }
            let action = chunk
                .as_ref()
                .and_then(|chunk| chunk.action(action_index))
                .ok_or_else(|| "policy returned an incomplete action sequence".to_owned())?;
            let goals = joint_names
                .iter()
                .zip(action)
                .map(|(name, value)| (name.clone(), f64::from(*value)))
                .collect::<BTreeMap<_, _>>();
            self.hardware
                .set_policy_goals(&request.robot, session, goals)
                .await?;
            action_index += 1;
            self.update_session(session, |snapshot| {
                snapshot.metrics.command_count += 1;
                if cycle_started.elapsed() > period {
                    snapshot.metrics.missed_deadlines += 1;
                }
            });
        }
    }

    fn update_session(&self, session: &str, update: impl FnOnce(&mut RolloutSnapshot)) {
        self.state.send_modify(|snapshot| {
            if snapshot.session_id.as_deref() == Some(session) {
                update(snapshot);
            }
        });
    }
}

fn validate_request(request: &StartRolloutRequest) -> Result<(), String> {
    if request.run_id.trim().is_empty() || request.robot.trim().is_empty() {
        return Err("rollout requires a training run and target robot".to_owned());
    }
    if request.checkpoint_step == 0 {
        return Err("checkpoint step must be greater than zero".to_owned());
    }
    if !(1..=MAX_ROLLOUT_DURATION_SECONDS).contains(&request.duration_seconds) {
        return Err(format!(
            "rollout duration_seconds must be between 1 and {MAX_ROLLOUT_DURATION_SECONDS}"
        ));
    }
    Ok(())
}

fn format_duration(seconds: u32) -> String {
    match seconds {
        1 => "1 second".to_owned(),
        value if value % 60 == 0 => {
            let minutes = value / 60;
            format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
        }
        value => format!("{value} seconds"),
    }
}

/// Wait until the control thread publishes the Policy mode it already accepted.
///
/// Command replies are sent while the fixed-rate control thread is draining its
/// queue, before that cycle's state is published. Reading `HardwareHandle::state`
/// immediately after `set_mode` can therefore still return the previous Idle
/// snapshot. Admission is complete only when the exact robot/session pair is
/// visible in an authoritative published state.
async fn wait_for_policy_admission(
    hardware: &mut watch::Receiver<HardwareState>,
    expected_robot: &str,
    expected_session: &str,
) -> Result<Option<String>, String> {
    let deadline = TokioInstant::now() + INPUT_STALE_AFTER;
    loop {
        let state = hardware.borrow().clone();
        if matches!(
            &state.mode.selection,
            ModeSelection::Policy { robot, session }
                if robot == expected_robot && session == expected_session
        ) {
            return Ok(None);
        }

        match tokio::time::timeout_at(deadline, hardware.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return Err(
                    "stopped policy: hardware telemetry closed before policy control was admitted"
                        .to_owned(),
                );
            }
            Err(_) => {
                let current = hardware.borrow().clone();
                return policy_exit(&current.mode, expected_robot, expected_session);
            }
        }
    }
}

/// Confirm that this rollout still owns the exclusive hardware-control state.
///
/// Selecting another healthy mode is an operator-requested state transition,
/// not a failed rollout. A hardware-owned transition carries its reason in the
/// mode error and remains a failure. The session check matters as much as the
/// mode name: cleanup for an old rollout must never command through or stop a
/// newer policy session.
fn policy_exit(
    mode: &ModeStatus,
    expected_robot: &str,
    expected_session: &str,
) -> Result<Option<String>, String> {
    if matches!(
        &mode.selection,
        ModeSelection::Policy { robot, session }
            if robot == expected_robot && session == expected_session
    ) {
        return Ok(None);
    }

    let replacement = describe_control_state(&mode.selection);
    if let Some(error) = &mode.error {
        return Err(format!(
            "stopped policy: hardware control transitioned to {replacement}: {error}"
        ));
    }
    Ok(Some(format!(
        "Policy stopped; hardware control transitioned to {replacement}."
    )))
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

fn parse_manifest(artifact: &TrainingCheckpointArtifact) -> Result<ArtifactManifest, String> {
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
    if manifest.format != ACT_ARTIFACT_FORMAT
        || manifest.artifact != "inference"
        || manifest.tensor_namespace != ACT_TENSOR_NAMESPACE
    {
        return Err("checkpoint is not a native Spiral ACT inference artifact".to_owned());
    }
    if manifest.run_id != request.run_id || manifest.step != request.checkpoint_step {
        return Err("checkpoint manifest identity does not match the requested run".to_owned());
    }
    let stored_config: serde_json::Value = serde_json::from_slice(config)
        .map_err(|error| format!("stored native model config is invalid: {error}"))?;
    if stored_config != manifest.model_config {
        return Err("stored native model config does not match the checkpoint manifest".to_owned());
    }
    if manifest.data_contract.joint_names.is_empty()
        || manifest.data_contract.camera_streams.is_empty()
    {
        return Err("checkpoint has an empty robot or camera contract".to_owned());
    }
    if !manifest.data_contract.sample_hz.is_finite()
        || !(1.0..=100.0).contains(&manifest.data_contract.sample_hz)
    {
        return Err("checkpoint sample_hz must be between 1 and 100".to_owned());
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
        if cameras.iter().all(|camera| {
            camera
                .receiver
                .borrow()
                .as_ref()
                .is_some_and(|frame| frame.captured_at.elapsed() <= INPUT_STALE_AFTER)
        }) {
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
    for camera in cameras {
        let frame = camera
            .receiver
            .borrow()
            .clone()
            .ok_or_else(|| format!("camera {:?} has no frame", camera.name))?;
        if frame.captured_at.elapsed() > INPUT_STALE_AFTER {
            return Err(format!("camera {:?} became stale", camera.name));
        }
    }
    Ok(())
}

fn ordered_state(
    robot: &spiral_robotics_core::api::RobotTelemetry,
    joint_names: &[String],
) -> Result<Vec<f32>, String> {
    joint_names
        .iter()
        .map(|name| {
            robot
                .joints
                .iter()
                .find(|joint| joint.name == *name)
                .and_then(|joint| joint.normalized)
                .filter(|value| value.is_finite())
                .map(|value| value as f32)
                .ok_or_else(|| format!("joint {name:?} has no finite calibrated telemetry"))
        })
        .collect()
}

fn camera_matches(stream: &str, role: &str) -> bool {
    stream == role || stream.ends_with(&format!(".{role}"))
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactDataContract, ArtifactManifest, CameraInput, INPUT_STALE_AFTER,
        MAX_ROLLOUT_DURATION_SECONDS, camera_matches, format_duration, policy_exit,
        require_fresh_cameras, validate_manifest, validate_request, wait_for_policy_admission,
    };
    use crate::backend::robotics::hardware::HardwareState;
    use crate::backend::training::{ACT_ARTIFACT_FORMAT, ACT_TENSOR_NAMESPACE};
    use bytes::Bytes;
    use spiral_robotics_core::api::{ModeSelection, ModeStatus, StartRolloutRequest};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn camera_roles_resolve_persisted_training_stream_ids() {
        assert!(camera_matches("follower.camera.wrist", "wrist"));
        assert!(camera_matches("top", "top"));
        assert!(!camera_matches("follower.camera.top", "wrist"));
    }

    #[test]
    fn rollout_request_only_selects_checkpoint_and_robot() {
        let request = StartRolloutRequest {
            run_id: "run-1".to_owned(),
            checkpoint_step: 10,
            robot: "follower".to_owned(),
            duration_seconds: 30,
        };
        assert!(validate_request(&request).is_ok());
    }

    #[test]
    fn rollout_duration_is_bounded_and_operator_readable() {
        let mut request = StartRolloutRequest {
            run_id: "run-1".to_owned(),
            checkpoint_step: 10,
            robot: "follower".to_owned(),
            duration_seconds: 30,
        };
        assert!(validate_request(&request).is_ok());
        request.duration_seconds = 0;
        assert!(validate_request(&request).is_err());
        request.duration_seconds = MAX_ROLLOUT_DURATION_SECONDS + 1;
        assert!(validate_request(&request).is_err());
        assert_eq!(format_duration(1), "1 second");
        assert_eq!(format_duration(60), "1 minute");
        assert_eq!(format_duration(120), "2 minutes");
    }

    #[test]
    fn the_admitted_policy_session_keeps_running() {
        let mode = ModeStatus {
            selection: ModeSelection::Policy {
                robot: "follower".to_owned(),
                session: "rollout-1".to_owned(),
            },
            active: true,
            error: None,
        };

        assert_eq!(policy_exit(&mode, "follower", "rollout-1"), Ok(None));
    }

    #[tokio::test]
    async fn policy_start_waits_for_the_control_thread_to_publish_admission() {
        let (publisher, mut hardware) = tokio::sync::watch::channel(HardwareState::default());
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

        assert_eq!(
            wait_for_policy_admission(&mut hardware, "follower", "rollout-1").await,
            Ok(None),
            "the stale Idle snapshot before publication must not stop the new policy"
        );
        publish.await.expect("policy publication task");
    }

    #[test]
    fn a_commanded_mode_transition_completes_the_policy_cleanly() {
        let mode = ModeStatus {
            selection: ModeSelection::Manual {
                robot: "follower".to_owned(),
            },
            active: true,
            error: None,
        };

        let message = policy_exit(&mode, "follower", "rollout-1")
            .expect("a commanded transition is not a rollout failure")
            .expect("the policy no longer owns hardware control");
        assert!(message.contains("Manual on follower"));
    }

    #[test]
    fn stop_and_hold_completes_the_policy_in_idle() {
        let mode = ModeStatus {
            selection: ModeSelection::Idle,
            active: false,
            error: None,
        };

        let message = policy_exit(&mode, "follower", "rollout-1")
            .expect("stop and hold is not a rollout failure")
            .expect("the policy no longer owns hardware control");
        assert!(message.contains("transitioned to Idle"));
    }

    #[test]
    fn a_hardware_forced_mode_transition_fails_the_policy_with_its_reason() {
        let mode = ModeStatus {
            selection: ModeSelection::Idle,
            active: false,
            error: Some("stopped Policy: its motor bus stopped responding".to_owned()),
        };

        let error = policy_exit(&mode, "follower", "rollout-1")
            .expect_err("a hardware failure must remain a failed rollout");
        assert!(error.contains("transitioned to Idle"));
        assert!(error.contains("motor bus stopped responding"));
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
                robot_type: "so101_follower".to_owned(),
                joint_names: vec!["shoulder".to_owned()],
                camera_streams: vec!["wrist".to_owned()],
                sample_hz: 60.0,
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
