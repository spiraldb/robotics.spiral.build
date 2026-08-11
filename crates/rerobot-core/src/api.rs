//! The wire contract between the studio server and the web UI.
//!
//! Both sides depend on this module, so a change to a field name is a compile
//! error rather than a runtime surprise. Every type derives both halves of
//! serde because the server sends what the browser receives and vice versa.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::calibration::{Direction, MotorCalibration};
use crate::spiral::column as spiral_column;

/// Server-owned data destinations. Secret values are intentionally absent:
/// the browser can replace or clear them but can never read them back.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DataSettings {
    pub spiral: SpiralSettings,
    pub hugging_face: HuggingFaceSettings,
    pub video: VideoSettings,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpiralSettings {
    pub enabled: bool,
    /// ADBC database URI, such as `spiraldb://` or `spiral:memory:`.
    pub uri: String,
    pub project_id: String,
    pub dataset: String,
}

impl Default for SpiralSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            uri: "spiraldb://".to_owned(),
            project_id: String::new(),
            dataset: "rerobot".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceSettings {
    pub enabled: bool,
    /// Dataset repository in `owner/name` form.
    pub repository: String,
    pub revision: String,
    pub has_token: bool,
}

impl Default for HuggingFaceSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            repository: String::new(),
            revision: "main".to_owned(),
            has_token: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VideoSettings {
    pub enabled: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Update from the settings screen. An omitted token preserves the stored
/// secret; `clear_*_token` removes it explicitly.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateDataSettingsRequest {
    pub spiral: SpiralSettings,
    pub hugging_face: HuggingFaceSettings,
    pub video: VideoSettings,
    #[serde(default)]
    pub hugging_face_token: Option<String>,
    #[serde(default)]
    pub clear_hugging_face_token: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionTestResponse {
    pub ok: bool,
    pub message: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicationStatus {
    pub episode_id: String,
    /// `queued`, `publishing`, `published`, `failed`, or `local`.
    pub state: String,
    pub spiral: Option<String>,
    pub hugging_face: Option<String>,
    pub updated_ms: i64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SpiralQueryRequest {
    pub sql: String,
    #[serde(default = "default_query_rows")]
    pub max_rows: usize,
}

fn default_query_rows() -> usize {
    100
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueryColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SpiralQueryResponse {
    pub columns: Vec<QueryColumn>,
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// The policy family executed by the isolated PyTorch worker.
///
/// The first vertical slice is deliberately honest about its data contract:
/// it trains a low-dimensional action-chunking transformer from state/action
/// columns returned by Spiral. Image-token policies join this enum only after
/// Spiral exposes a typed camera tensor contract.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingPolicy {
    #[default]
    Act,
}

impl TrainingPolicy {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Act => "ACT · state",
        }
    }
}

/// Authoritative lifecycle of one backend-owned training process.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingRunPhase {
    #[default]
    Starting,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
}

impl TrainingRunPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Completed => "Completed",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
        }
    }

    pub const fn active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

/// Everything needed to reproduce one run. Spiral is the only sample source.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StartTrainingRequest {
    pub run_name: String,
    #[serde(default)]
    pub policy: TrainingPolicy,
    pub sql: String,
    #[serde(default = "default_training_state_column")]
    pub state_column: String,
    #[serde(default = "default_training_action_column")]
    pub action_column: String,
    #[serde(default = "default_training_device")]
    pub device: String,
    #[serde(default = "default_training_steps")]
    pub steps: u64,
    #[serde(default = "default_training_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_training_learning_rate")]
    pub learning_rate: f64,
    #[serde(default = "default_training_checkpoint_every")]
    pub checkpoint_every: u64,
    #[serde(default = "default_training_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_training_seed")]
    pub seed: u64,
}

impl Default for StartTrainingRequest {
    fn default() -> Self {
        Self {
            run_name: "act-run".to_owned(),
            policy: TrainingPolicy::Act,
            sql: String::new(),
            state_column: default_training_state_column(),
            action_column: default_training_action_column(),
            device: default_training_device(),
            steps: default_training_steps(),
            batch_size: default_training_batch_size(),
            learning_rate: default_training_learning_rate(),
            checkpoint_every: default_training_checkpoint_every(),
            chunk_size: default_training_chunk_size(),
            seed: default_training_seed(),
        }
    }
}

fn default_training_state_column() -> String {
    spiral_column::STATE.to_owned()
}

fn default_training_action_column() -> String {
    spiral_column::ACTION.to_owned()
}

fn default_training_device() -> String {
    "auto".to_owned()
}

const fn default_training_steps() -> u64 {
    18_000
}

const fn default_training_batch_size() -> usize {
    256
}

const fn default_training_learning_rate() -> f64 {
    3.0e-4
}

const fn default_training_checkpoint_every() -> u64 {
    1_000
}

const fn default_training_chunk_size() -> usize {
    16
}

const fn default_training_seed() -> u64 {
    42
}

/// One sample on the shared training-step axis used by every Studio plot.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingMetricPoint {
    pub step: u64,
    pub train_loss: f64,
    pub validation_loss: Option<f64>,
    pub learning_rate: f64,
    pub gpu_utilization_pct: Option<f64>,
    pub gpu_memory_bytes: Option<u64>,
    pub cpu_utilization_pct: f64,
    pub transfer_bytes_per_second: f64,
    pub examples_per_second: f64,
    pub queue_depth: usize,
    pub elapsed_seconds: f64,
    #[serde(default)]
    pub event: Option<String>,
}

/// A model/optimizer snapshot durably ingested into Spiral.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingCheckpoint {
    pub step: u64,
    pub created_ms: i64,
    pub validation_loss: Option<f64>,
    pub spiral_table: String,
    pub byte_size: u64,
}

/// Immutable Spiral identity captured when a run starts.
///
/// The browser can show where samples and checkpoints belong without learning
/// credentials or depending on mutable Settings state.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingSource {
    pub project: String,
    pub dataset: String,
    pub checkpoint_table: String,
}

/// Result of checking every dependency needed before a worker can start.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingReadiness {
    pub ready: bool,
    pub python_ready: bool,
    pub spiral_ready: bool,
    pub summary: String,
}

impl Default for TrainingReadiness {
    fn default() -> Self {
        Self {
            ready: false,
            python_ready: false,
            spiral_ready: false,
            summary: "Checking the local training runtime…".to_owned(),
        }
    }
}

/// Browser-safe run state. Checkpoint bytes and process handles never cross the
/// wire; only their durable Spiral identity does.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingRun {
    pub id: String,
    pub config: StartTrainingRequest,
    #[serde(default)]
    pub source: TrainingSource,
    pub phase: TrainingRunPhase,
    pub current_step: u64,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub message: Option<String>,
    pub metrics: Vec<TrainingMetricPoint>,
    pub checkpoints: Vec<TrainingCheckpoint>,
    pub logs: Vec<String>,
}

/// Complete training state published through the Studio event stream.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingSnapshot {
    #[serde(default)]
    pub readiness: TrainingReadiness,
    pub worker: String,
    pub runs: Vec<TrainingRun>,
}

pub const MAX_TRAINING_METRIC_POINTS: usize = 1_200;
pub const MAX_TRAINING_LOG_LINES: usize = 120;
pub const MAX_TRAINING_CHECKPOINTS: usize = 1_000;
pub const MAX_TRAINING_HISTORY_RUNS: usize = 40;

/// Incremental training event sent after the initial snapshot.
///
/// Metrics are intentionally one point at a time: republishing every retained
/// run for every optimizer step made event bandwidth grow with history.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrainingUpdate {
    Readiness {
        readiness: TrainingReadiness,
    },
    RunAdded {
        run: Box<TrainingRun>,
    },
    RunState {
        run_id: String,
        phase: TrainingRunPhase,
        current_step: u64,
        finished_ms: Option<i64>,
        message: Option<String>,
    },
    Metric {
        run_id: String,
        point: Box<TrainingMetricPoint>,
    },
    Checkpoint {
        run_id: String,
        checkpoint: TrainingCheckpoint,
    },
    Log {
        run_id: String,
        line: String,
    },
}

impl TrainingSnapshot {
    pub fn active_run(&self) -> Option<&TrainingRun> {
        self.runs.iter().find(|run| run.phase.active())
    }

    pub fn apply(&mut self, update: TrainingUpdate) {
        match update {
            TrainingUpdate::Readiness { readiness } => self.readiness = readiness,
            TrainingUpdate::RunAdded { run } => {
                self.runs.retain(|existing| existing.id != run.id);
                self.runs.insert(0, *run);
                self.runs.truncate(MAX_TRAINING_HISTORY_RUNS);
            }
            TrainingUpdate::RunState {
                run_id,
                phase,
                current_step,
                finished_ms,
                message,
            } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == run_id) {
                    run.phase = phase;
                    run.current_step = current_step;
                    run.finished_ms = finished_ms;
                    run.message = message;
                }
            }
            TrainingUpdate::Metric { run_id, point } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == run_id) {
                    run.current_step = point.step;
                    run.metrics.push(*point);
                    let excess = run.metrics.len().saturating_sub(MAX_TRAINING_METRIC_POINTS);
                    if excess > 0 {
                        run.metrics.drain(..excess);
                    }
                }
            }
            TrainingUpdate::Checkpoint { run_id, checkpoint } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == run_id) {
                    run.checkpoints.push(checkpoint);
                    let excess = run
                        .checkpoints
                        .len()
                        .saturating_sub(MAX_TRAINING_CHECKPOINTS);
                    if excess > 0 {
                        run.checkpoints.drain(..excess);
                    }
                }
            }
            TrainingUpdate::Log { run_id, line } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == run_id) {
                    run.logs.push(line);
                    let excess = run.logs.len().saturating_sub(MAX_TRAINING_LOG_LINES);
                    if excess > 0 {
                        run.logs.drain(..excess);
                    }
                }
            }
        }
    }
}

/// Replay a trajectory fetched from Spiral instead of the local recovery file.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartSpiralReplayRequest {
    pub episode: String,
    pub robot: String,
    pub stream: String,
}

/// Everything the UI needs to draw the setup screens.
///
/// This changes only when a robot, camera, or mode changes, so it is sent on
/// connect and then whenever it actually differs.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct StudioSnapshot {
    /// Directory the server reads and writes robot files in.
    pub workspace: String,
    /// True when the server is driving simulated motors instead of hardware.
    pub simulated: bool,
    /// Every robot file found in the workspace, by ID.
    pub robots: Vec<RobotSummary>,
    /// Cameras the server has discovered.
    pub cameras: Vec<CameraStatus>,
    /// The currently selected operating mode.
    pub mode: ModeStatus,
    /// Every episode found in the episode directory, oldest first.
    ///
    /// Takes live here rather than in telemetry because the list only changes
    /// when a recording finishes or a take is judged, while the counters of a
    /// take in progress change sixty times a second.
    pub episodes: Vec<EpisodeSummary>,
    /// Directory the server reads and writes episode files in.
    pub episode_directory: String,
    /// Problems worth showing the operator: unreadable robot files, cameras
    /// that failed to probe, and similar non-fatal trouble.
    pub issues: Vec<String>,
}

/// One robot instance: exactly one arm, stored in exactly one JSON file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RobotSummary {
    pub id: String,
    /// Name of the robot's JSON file inside the workspace.
    pub file: String,
    pub robot_type: Option<String>,
    pub port: Option<PortSettings>,
    pub motors: Vec<MotorSummary>,
    pub cameras: Vec<CameraBinding>,
    /// True when every motor has a stored calibration.
    pub calibrated: bool,
    /// Why this robot cannot be used, when its file is structurally wrong.
    ///
    /// A file that parses as JSON can still describe an unusable robot — two
    /// motors sharing a bus ID, for instance. Carrying the reason here keeps
    /// the card from silently rendering as an empty robot.
    pub problem: Option<String>,
    pub connection: ConnectionStatus,
}

/// Pick the best connected pair for a recording-led teleoperation session.
///
/// Robot type is a preference rather than a requirement: a configured leader
/// and follower win, while two otherwise ready compatible arms still work.
/// Compatibility is checked here so the Record button and the server make the
/// same decision instead of offering a pair the control thread must reject.
pub fn default_teleoperation_pair(robots: &[RobotSummary]) -> Option<(String, String)> {
    let ready = robots
        .iter()
        .filter(|robot| robot.connection.connected && robot.calibrated && robot.problem.is_none())
        .collect::<Vec<_>>();

    ready
        .iter()
        .enumerate()
        .flat_map(|(leader_index, leader)| {
            ready
                .iter()
                .enumerate()
                .filter(move |(follower_index, follower)| {
                    leader_index != *follower_index
                        && leader.motors.iter().any(|leader_motor| {
                            follower
                                .motors
                                .iter()
                                .any(|follower_motor| follower_motor.name == leader_motor.name)
                        })
                })
                .map(move |(follower_index, follower)| {
                    let role_penalty = u8::from(!robot_has_role(leader, "leader"))
                        + u8::from(!robot_has_role(follower, "follower"));
                    (
                        (role_penalty, leader_index, follower_index),
                        (leader.id.clone(), follower.id.clone()),
                    )
                })
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, pair)| pair)
}

fn robot_has_role(robot: &RobotSummary, role: &str) -> bool {
    robot
        .robot_type
        .as_deref()
        .is_some_and(|kind| kind.to_ascii_lowercase().contains(role))
}

/// A motor as configured, with its calibration when one exists.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MotorSummary {
    pub name: String,
    pub id: u32,
    pub direction: Direction,
    pub model: Option<String>,
    pub calibration: Option<MotorCalibration>,
    /// What this motor will fault at, read from its EEPROM when it connected.
    ///
    /// Lives on the snapshot rather than the telemetry frame because it does
    /// not change while an arm is running; resending it twenty times a second
    /// would be resending a constant.
    #[serde(default)]
    pub limits: MotorLimits,
}

/// The limits a motor enforces on itself.
///
/// These are read from the hardware rather than written down here. A motor
/// already knows the temperature and voltage it will fault at, and a threshold
/// copied out of a datasheet is a threshold that can disagree with the motor in
/// front of the operator.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MotorLimits {
    pub max_temperature_celsius: Option<f64>,
    pub min_voltage_volts: Option<f64>,
    pub max_voltage_volts: Option<f64>,
    /// Configured torque ceiling as a fraction of the motor's rated torque.
    pub max_load: Option<f64>,
}

/// Serial settings for a robot's motor bus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortSettings {
    pub path: String,
    pub baud_rate: u32,
}

/// A camera assigned to a robot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CameraBinding {
    /// Role the camera plays for this robot, such as `wrist` or `top`.
    pub name: String,
    /// Stable device identifier reported by camera discovery.
    pub id: String,
    /// Where the camera is bolted, when the robot's file says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<CameraMount>,
}

/// Where a camera sits and how much of the bench it can see.
///
/// Optional, and stays optional. A camera with no mount is a camera whose
/// position nobody has told the studio, and the honest thing to do with an
/// unknown position is not to draw one — a frustum at the origin would be a
/// claim about the hardware rather than a picture of it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CameraMount {
    /// Link in the robot description the camera rides, or `None` for one fixed
    /// to the bench. A wrist camera moves with the wrist; a top camera does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    /// Offset from that frame, in metres.
    #[serde(default)]
    pub xyz: [f64; 3],
    /// Roll, pitch, and yaw from that frame, in radians.
    #[serde(default)]
    pub rpy: [f64; 3],
    /// Horizontal field of view, in degrees.
    #[serde(default = "default_field_of_view")]
    pub fov_degrees: f64,
}

/// Field of view assumed for a mount that does not state one.
///
/// A middling webcam lens. It is a drawing default, not a measurement, and the
/// frustum it produces answers "is this pointed at the workspace" rather than
/// "what exactly is in shot".
fn default_field_of_view() -> f64 {
    62.0
}

/// Whether a robot's motor bus is currently open.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionStatus {
    pub connected: bool,
    /// Populated when the last connect or transfer attempt failed.
    pub error: Option<String>,
}

/// A camera the server knows about.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CameraStatus {
    /// Stable device identifier, also used in stream URLs.
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// True when a capture thread is running and serving frames.
    pub streaming: bool,
    /// Frames delivered since the stream opened.
    pub frames: u64,
    pub error: Option<String>,
}

/// A serial port offered as a candidate motor bus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SerialPortSummary {
    pub path: String,
    /// `usb`, `bluetooth`, `pci`, or `unknown`.
    pub kind: String,
    pub description: Option<String>,
    /// ID of the robot currently holding this port open, if any.
    pub used_by: Option<String>,
}

/// The operating mode the studio is in.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ModeSelection {
    /// Motors are read but never commanded.
    #[default]
    Idle,
    /// The UI commands one robot's joints directly.
    Manual { robot: String },
    /// A leader arm's measured pose is mirrored onto a follower arm.
    Teleoperate { leader: String, follower: String },
    /// A recorded episode's joint targets are played back onto one arm.
    ///
    /// The trajectory itself is not carried here: this type is the *published*
    /// mode, and a mode the browser can post is a mode the browser could post
    /// a hundred megabytes into. Replay is started through its own endpoint,
    /// which loads the episode server-side and hands the samples to the
    /// control thread directly.
    Replay { robot: String, episode: String },
}

impl ModeSelection {
    /// Human-readable name for status text.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Manual { .. } => "Manual",
            Self::Teleoperate { .. } => "Teleoperation",
            Self::Replay { .. } => "Replay",
        }
    }
}

/// The selected mode plus whatever went wrong entering or running it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModeStatus {
    pub selection: ModeSelection,
    /// True once the mode is actually driving motors.
    pub active: bool,
    pub error: Option<String>,
}

/// A live sample of every connected robot.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TelemetryFrame {
    pub sequence: u64,
    /// Measured rate of the hardware control loop.
    pub loop_hz: f64,
    /// Requested rate of the hardware control loop.
    ///
    /// Keeping the target beside the measurement lets operators distinguish a
    /// deliberately slow configuration from a loop that cannot keep up with
    /// its motor buses.
    pub target_hz: f64,
    pub robots: Vec<RobotTelemetry>,
    /// Present only while a take is being captured.
    pub recording: Option<RecordingProgress>,
    /// Present only while an episode is being played back.
    pub replay: Option<ReplayProgress>,
}

/// One robot's live state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RobotTelemetry {
    pub id: String,
    pub torque: bool,
    pub joints: Vec<JointTelemetry>,
    pub error: Option<String>,
    /// Present only while a calibration sweep is running for this robot.
    pub calibration: Option<CalibrationProgress>,
}

/// One joint's live state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JointTelemetry {
    pub name: String,
    pub id: u32,
    /// Raw encoder reading, always available.
    pub raw: i64,
    /// Degrees from the calibrated center; `None` until the joint is calibrated.
    pub degrees: Option<f64>,
    /// Position within the calibrated travel, `-1.0` to `1.0`.
    pub normalized: Option<f64>,
    /// Last raw position commanded to this joint, when it is being driven.
    pub goal_raw: Option<i64>,
    /// Final calibrated target before the controller's safety ramp.
    pub target_raw: Option<i64>,
    /// Final target in degrees from the calibrated center.
    ///
    /// This is carried explicitly because encoder direction is part of the
    /// calibration and cannot be reconstructed from [`JointRange`] alone.
    pub target_degrees: Option<f64>,
    pub range: Option<JointRange>,

    /// What the motor reports about its own condition.
    ///
    /// Every one of these is optional and stays optional: a bus whose firmware
    /// will not answer the wider status read still drives the arm perfectly
    /// well, and a missing measurement reported as zero would put a cold motor
    /// and an unreadable one in the same cell. Unlike the fields above, none of
    /// these depend on calibration — they are the only readings an operator can
    /// get out of an arm that has never been calibrated.
    #[serde(default)]
    pub temperature_celsius: Option<f64>,
    #[serde(default)]
    pub voltage_volts: Option<f64>,
    #[serde(default)]
    pub current_amps: Option<f64>,
    /// Signed fraction of the motor's rated torque.
    #[serde(default)]
    pub load: Option<f64>,
    #[serde(default)]
    pub speed_degrees_per_second: Option<f64>,
    /// How fast this motor is heating, in degrees per minute.
    ///
    /// A temperature alone cannot be acted on: fifty-eight degrees is fine, or
    /// four minutes from a shutdown, depending entirely on which way it is
    /// going. Measured over a window rather than between frames, because these
    /// registers report whole degrees and a difference taken across two frames
    /// is quantization noise wearing a unit. `None` until the window fills.
    #[serde(default)]
    pub temperature_rise_per_minute: Option<f64>,
    /// Faults the motor is reporting about itself, not thresholds applied here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub faults: Vec<MotorFault>,
}

/// A fault a motor reports about itself.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorFault {
    Overheat,
    Overload,
    Voltage,
    /// A fault bit the motor set that this build does not have a name for.
    ///
    /// Carried rather than dropped: an operator who can see that something is
    /// wrong can act on it, and a fault silently swallowed because it was
    /// unfamiliar is the one that ends a session.
    Unrecognized,
}

impl MotorFault {
    /// How the studio says this on the page.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Overheat => "overheating",
            Self::Overload => "overloaded",
            Self::Voltage => "supply voltage",
            Self::Unrecognized => "motor fault",
        }
    }
}

/// How a motor is doing against its own limits.
///
/// Ordered worst-last so an arm's condition is the maximum of its motors'.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorCondition {
    #[default]
    Nominal,
    Caution,
    Fault,
}

impl MotorCondition {
    pub fn is_nominal(self) -> bool {
        self == Self::Nominal
    }
}

/// How close to a motor's own limit is close enough to report, as a fraction.
///
/// Lives on the wire contract for the same reason
/// [`CAUTION_FOLLOW_ERROR_DEGREES`] does: the interface tints a reading with
/// it, and anything else reading this API has to reach the same verdict about
/// the same motor. The number is a margin, never a threshold — the threshold
/// itself comes from the motor's EEPROM.
pub const CAUTION_LIMIT_FRACTION: f64 = 0.85;

/// How near the edge of its own voltage window a supply may drift quietly.
pub const CAUTION_VOLTAGE_MARGIN_VOLTS: f64 = 0.3;

impl JointTelemetry {
    /// This motor's condition, judged against the limits it reported itself.
    ///
    /// A motor raising its own fault bit is a fault whatever the limits say —
    /// that is the hardware's verdict and it outranks any margin applied here.
    /// Without limits there is nothing to be near, so only the motor's own
    /// faults can speak.
    pub fn condition(&self, limits: &MotorLimits) -> MotorCondition {
        if !self.faults.is_empty() {
            return MotorCondition::Fault;
        }

        let hot = match (self.temperature_celsius, limits.max_temperature_celsius) {
            (Some(celsius), Some(limit)) => celsius >= limit * CAUTION_LIMIT_FRACTION,
            _ => false,
        };
        let strained = match (self.load, limits.max_load) {
            (Some(load), Some(limit)) => load.abs() >= limit * CAUTION_LIMIT_FRACTION,
            _ => false,
        };
        let supply = match self.voltage_volts {
            Some(volts) => {
                let low = limits
                    .min_voltage_volts
                    .is_some_and(|min| volts <= min + CAUTION_VOLTAGE_MARGIN_VOLTS);
                let high = limits
                    .max_voltage_volts
                    .is_some_and(|max| volts >= max - CAUTION_VOLTAGE_MARGIN_VOLTS);
                low || high
            }
            None => false,
        };

        if hot || strained || supply {
            MotorCondition::Caution
        } else {
            MotorCondition::Nominal
        }
    }
}

/// The calibrated travel of one joint.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JointRange {
    pub min: i64,
    pub center: i64,
    pub max: i64,
}

/// Live progress of a calibration sweep.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CalibrationProgress {
    pub joints: Vec<CalibrationJointProgress>,
    /// True once every joint has moved far enough to record a range.
    pub ready: bool,
}

/// One joint's progress through its calibration sweep.
///
/// Carries its own name rather than relying on position, so the UI never has
/// to assume two independently built lists are in the same order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CalibrationJointProgress {
    pub name: String,
    pub min: i64,
    pub position: i64,
    pub max: i64,
}

impl CalibrationJointProgress {
    /// Travel observed so far, in encoder counts.
    pub fn span(&self) -> i64 {
        self.max.saturating_sub(self.min)
    }
}

/// Degrees of `Follow Δ` at which a joint stops tracking acceptably.
///
/// Lives on the wire contract because both ends need the same number: the
/// control thread uses it to decide when a take has settled and its measurement
/// can start, and the interface uses it to colour a reading. Two copies of this
/// constant would let the studio call a take clean that the recorder called
/// caution.
pub const CAUTION_FOLLOW_ERROR_DEGREES: f64 = 5.0;

/// One recorded episode, as summarised from its file.
///
/// An episode is one take: the operator arms the recorder, performs the task
/// once, and stops. Unlike a robot, an episode is immutable except for the one
/// judgement the operator makes about it afterwards.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeSummary {
    /// Stable identifier, also the file's stem.
    pub id: String,
    /// Name of the episode's file inside the episode directory.
    pub file: String,
    /// Arms sampled in this take, in the order their joints appear in a frame.
    pub arms: Vec<EpisodeArm>,
    pub frames: u64,
    pub duration_seconds: f64,
    /// Wall-clock intervals deliberately omitted from the episode timeline.
    ///
    /// Samples and recorded video use the active timeline, so pausing does not
    /// create a motionless training-data plateau. Each entry preserves where
    /// the omission happened and how much real time passed there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recording_gaps: Vec<RecordingGap>,
    /// Rate the take was captured at, so a replay can honour its timing.
    pub sample_hz: f64,
    /// When the take started, as milliseconds since the Unix epoch.
    ///
    /// Carried as a number rather than a formatted string so the client can
    /// render it in the operator's own locale and timezone.
    pub started_ms: i64,
    /// The mode that produced the take, in the studio's own words.
    pub source: String,
    /// Largest gap between a measured and a commanded joint angle during the
    /// take, in degrees — the same quantity the inspector shows live as
    /// `Follow Δ`, kept for the whole take instead of for one instant.
    pub max_follow_error_degrees: Option<f64>,
    /// True once the operator has judged this take unusable.
    ///
    /// A discarded take is kept on disk and excluded from an export, because
    /// the judgement is the operator's and deleting on their behalf is not.
    pub discarded: bool,
    /// Why this take cannot be replayed or exported.
    ///
    /// A take whose recording never finished — the server was killed mid-take,
    /// say — is still listed, with the reason attached, rather than silently
    /// omitted or silently offered as if it were whole.
    pub problem: Option<String>,
}

impl EpisodeSummary {
    /// True when the take is complete enough to replay or export.
    pub fn usable(&self) -> bool {
        self.problem.is_none() && self.frames > 0
    }

    /// The joints of the arm named, when this take sampled it.
    pub fn arm(&self, id: &str) -> Option<&EpisodeArm> {
        self.arms.iter().find(|arm| arm.id == id)
    }
}

/// One arm's presence in a take.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EpisodeArm {
    pub id: String,
    /// Joint names, in the order their values appear in every frame.
    pub joints: Vec<String>,
}

/// A take being captured right now.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecordingProgress {
    /// Identifier the finished take will have.
    pub episode: String,
    pub frames: u64,
    pub elapsed_seconds: f64,
    /// Real time since Record was pressed, including deliberate pauses.
    #[serde(default)]
    pub wall_elapsed_seconds: f64,
    /// Real time omitted from the active episode timeline so far.
    #[serde(default)]
    pub paused_seconds: f64,
    /// Whether samples and camera frames are currently being omitted.
    #[serde(default)]
    pub paused: bool,
    /// Arms being sampled.
    pub arms: Vec<String>,
    /// Largest `Follow Δ` seen so far in this take, in degrees.
    pub max_follow_error_degrees: Option<f64>,
    /// Set when the capture stopped itself — a full disk, or an arm that went
    /// away mid-take.
    pub error: Option<String>,
}

/// A deliberate pause in an otherwise continuous recording session.
///
/// `after_seconds` is expressed on the gap-free active timeline. This keeps
/// replay and dataset sampling contiguous while retaining enough provenance to
/// display or audit every omitted wall-clock interval.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecordingGap {
    pub after_seconds: f64,
    pub duration_seconds: f64,
}

/// Why a replay trajectory is loaded.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPurpose {
    /// User-facing episode transport controlled by Play/Pause.
    #[default]
    Timeline,
    /// One committed Timeline scrub moving the robot to a single pose.
    PhysicalSeek,
}

/// Whether a replay is positioning the arm or advancing through the episode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPhase {
    Seeking,
    /// Default preserves the behavior of progress sent by an older backend.
    #[default]
    Playing,
}

/// An episode being played back right now.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReplayProgress {
    pub episode: String,
    /// The arm being driven.
    pub robot: String,
    /// Frames played so far.
    pub frame: u64,
    pub frames: u64,
    pub elapsed_seconds: f64,
    pub duration_seconds: f64,
    /// Whether this is Timeline transport or a one-pose physical seek.
    #[serde(default)]
    pub purpose: ReplayPurpose,
    /// Positioning never advances the episode clock or recorded media.
    #[serde(default)]
    pub phase: ReplayPhase,
    /// Whether playback is holding its current frame without discarding the
    /// loaded trajectory. Resuming continues from this exact position.
    #[serde(default)]
    pub paused: bool,
}

/// Begin capturing a take.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartRecordingRequest {
    /// Arms to sample. Empty means every connected arm, which is the useful
    /// default: a take is a record of the whole scene, not of one arm.
    #[serde(default)]
    pub arms: Vec<String>,
    /// A short operator note stored with the take.
    #[serde(default)]
    pub note: Option<String>,
}

/// Finish the take in progress.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StopRecordingRequest {
    /// False marks the finished take discarded in the same gesture that ends
    /// it, for the common case of knowing the take was bad while it ran.
    pub keep: bool,
}

/// Pause or resume the take in progress without changing its episode ID.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetRecordingPausedRequest {
    pub paused: bool,
}

/// Pause or resume a replay without unloading its trajectory.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetReplayPausedRequest {
    pub paused: bool,
}

/// Change the operator's judgement of a stored take.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct JudgeEpisodeRequest {
    pub discarded: bool,
}

/// Play a stored episode back onto one arm.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct StartReplayRequest {
    pub episode: String,
    /// The arm to drive. It must be connected and calibrated.
    pub robot: String,
    /// Which recorded arm's trajectory to play, when the take holds more than
    /// one. Defaults to the arm being driven, then to the take's only arm.
    #[serde(default)]
    pub source_arm: Option<String>,
    /// Episode-clock position where replay should begin. Older clients omit
    /// this and retain the original frame-zero behavior.
    #[serde(default)]
    pub start_seconds: f64,
}

/// Write the kept takes out as a dataset.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportRequest {
    /// Directory to write. Relative paths resolve against the episode
    /// directory's parent; an existing directory is refused rather than merged
    /// into, so an export is always exactly one session's decision.
    pub directory: String,
}

/// What an export produced.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportResponse {
    /// Absolute path actually written.
    pub directory: String,
    pub episodes: u64,
    pub frames: u64,
    /// Takes left out because the operator discarded them or they are unusable.
    pub skipped: u64,
}

/// What importing a LeRobot dataset added to the local episode workspace.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImportResponse {
    pub episodes: u64,
    pub frames: u64,
    /// Imported episodes are assigned fresh local IDs; this is the first one.
    pub first_episode: Option<String>,
}

/// Create a new robot instance and its JSON file.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateRobotRequest {
    pub id: String,
    /// Defaults to `so101_follower` when omitted.
    #[serde(default)]
    pub robot_type: Option<String>,
    #[serde(default)]
    pub port: Option<String>,
    #[serde(default)]
    pub baud_rate: Option<u32>,
}

/// Change a robot's port or camera assignments.
///
/// Omitted fields are left exactly as they are on disk, including fields this
/// build does not understand.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct UpdateRobotRequest {
    #[serde(default)]
    pub robot_type: Option<String>,
    #[serde(default)]
    pub port: Option<PortSettings>,
    #[serde(default)]
    pub cameras: Option<Vec<CameraBinding>>,
}

/// Enable or disable holding torque on a robot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TorqueRequest {
    pub enabled: bool,
}

/// Command joint positions in normalized `[-1.0, 1.0]` space.
///
/// Normalized commands are the only ones the API accepts: they are meaningless
/// without a calibration, which is exactly the safety property wanted here.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GoalRequest {
    pub normalized: BTreeMap<String, f64>,
}

/// Identify a motor bus by comparing the ports present before and after a cable
/// is unplugged.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IdentifyPortRequest {
    /// Port paths observed before the cable was removed.
    pub before: Vec<String>,
}

/// The outcome of a port-identification attempt.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IdentifyPortResponse {
    /// The single port that disappeared, when exactly one did.
    pub port: Option<String>,
    /// Every port that disappeared, for the ambiguous case.
    pub removed: Vec<String>,
    pub message: String,
}

/// A failed request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiError {
    pub error: String,
}

impl ApiError {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn robot(id: &str, robot_type: Option<&str>) -> RobotSummary {
        RobotSummary {
            id: id.to_owned(),
            file: format!("{id}.json"),
            robot_type: robot_type.map(str::to_owned),
            port: None,
            motors: vec![MotorSummary {
                name: "shoulder".to_owned(),
                id: 1,
                direction: Direction::Normal,
                model: None,
                calibration: None,
                limits: MotorLimits::default(),
            }],
            cameras: Vec::new(),
            calibrated: true,
            problem: None,
            connection: ConnectionStatus {
                connected: true,
                error: None,
            },
        }
    }

    #[test]
    fn teleoperation_pair_prefers_configured_roles_over_list_order() {
        let robots = [
            robot("follower", Some("so101_follower")),
            robot("spare", None),
            robot("leader", Some("SO101_LEADER")),
        ];

        assert_eq!(
            default_teleoperation_pair(&robots),
            Some(("leader".to_owned(), "follower".to_owned()))
        );
    }

    #[test]
    fn teleoperation_pair_falls_back_to_the_first_compatible_ready_arms() {
        let robots = [robot("left", None), robot("right", None)];

        assert_eq!(
            default_teleoperation_pair(&robots),
            Some(("left".to_owned(), "right".to_owned()))
        );
    }

    #[test]
    fn teleoperation_pair_ignores_unready_and_incompatible_arms() {
        let mut disconnected = robot("disconnected-leader", Some("leader"));
        disconnected.connection.connected = false;
        let mut incompatible = robot("incompatible-leader", Some("leader"));
        incompatible.motors[0].name = "elbow".to_owned();
        let follower = robot("follower", Some("follower"));
        let fallback = robot("fallback", None);

        assert_eq!(
            default_teleoperation_pair(&[disconnected, incompatible, follower, fallback.clone(),]),
            Some((fallback.id, "follower".to_owned()))
        );
    }

    #[test]
    fn mode_selection_round_trips_through_its_tag() {
        let cases = [
            (ModeSelection::Idle, r#"{"mode":"idle"}"#),
            (
                ModeSelection::Manual {
                    robot: "arm".to_owned(),
                },
                r#"{"mode":"manual","robot":"arm"}"#,
            ),
            (
                ModeSelection::Teleoperate {
                    leader: "left".to_owned(),
                    follower: "right".to_owned(),
                },
                r#"{"mode":"teleoperate","leader":"left","follower":"right"}"#,
            ),
            (
                ModeSelection::Replay {
                    robot: "right".to_owned(),
                    episode: "episode_000007".to_owned(),
                },
                r#"{"mode":"replay","robot":"right","episode":"episode_000007"}"#,
            ),
        ];

        for (mode, json) in cases {
            assert_eq!(serde_json::to_string(&mode).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<ModeSelection>(json).unwrap(),
                mode,
                "{json} should round-trip"
            );
        }
    }

    #[test]
    fn optional_request_fields_may_be_omitted() {
        let created: CreateRobotRequest = serde_json::from_str(r#"{"id":"arm"}"#).unwrap();
        assert_eq!(created.id, "arm");
        assert_eq!(created.robot_type, None);

        let update: UpdateRobotRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(update, UpdateRobotRequest::default());
    }

    #[test]
    fn older_replay_progress_defaults_to_timeline_playback() {
        let replay: ReplayProgress = serde_json::from_str(
            r#"{
                "episode":"episode_000001",
                "robot":"follower",
                "frame":12,
                "frames":60,
                "elapsed_seconds":0.2,
                "duration_seconds":1.0
            }"#,
        )
        .unwrap();

        assert_eq!(replay.purpose, ReplayPurpose::Timeline);
        assert_eq!(replay.phase, ReplayPhase::Playing);
        assert!(!replay.paused);
    }

    #[test]
    fn an_unfinished_take_is_listed_but_not_offered() {
        let mut episode = EpisodeSummary {
            id: "episode_000001".to_owned(),
            file: "episode_000001.jsonl".to_owned(),
            arms: vec![EpisodeArm {
                id: "follower".to_owned(),
                joints: vec!["shoulder_pan".to_owned()],
            }],
            frames: 120,
            duration_seconds: 2.0,
            recording_gaps: Vec::new(),
            sample_hz: 60.0,
            started_ms: 1_770_000_000_000,
            source: "Teleoperation".to_owned(),
            max_follow_error_degrees: Some(3.5),
            discarded: false,
            problem: None,
        };
        assert!(episode.usable());
        assert_eq!(episode.arm("follower").map(|arm| arm.joints.len()), Some(1));
        assert_eq!(episode.arm("leader"), None);

        episode.problem = Some("recording did not finish".to_owned());
        assert!(!episode.usable(), "a truncated take must not be offered");

        // An empty take is unusable even without a recorded reason: there is
        // nothing in it to replay.
        episode.problem = None;
        episode.frames = 0;
        assert!(!episode.usable());
    }

    #[test]
    fn calibration_span_saturates_instead_of_overflowing() {
        let progress = CalibrationJointProgress {
            name: "gripper".to_owned(),
            min: i64::MIN,
            position: 0,
            max: i64::MAX,
        };
        assert_eq!(progress.span(), i64::MAX);
    }

    fn joint() -> JointTelemetry {
        JointTelemetry {
            name: "shoulder_pan".to_owned(),
            id: 1,
            raw: 2048,
            degrees: None,
            normalized: None,
            goal_raw: None,
            target_raw: None,
            target_degrees: None,
            range: None,
            temperature_celsius: Some(30.0),
            voltage_volts: Some(7.4),
            current_amps: Some(0.2),
            load: Some(0.1),
            speed_degrees_per_second: Some(0.0),
            temperature_rise_per_minute: Some(0.0),
            faults: Vec::new(),
        }
    }

    fn limits() -> MotorLimits {
        MotorLimits {
            max_temperature_celsius: Some(70.0),
            min_voltage_volts: Some(4.5),
            max_voltage_volts: Some(9.0),
            max_load: Some(1.0),
        }
    }

    #[test]
    fn a_motor_within_its_own_limits_is_nominal() {
        assert_eq!(joint().condition(&limits()), MotorCondition::Nominal);
    }

    #[test]
    fn nearing_a_limit_is_caution_and_the_motors_own_verdict_is_fault() {
        let hot = JointTelemetry {
            temperature_celsius: Some(70.0 * CAUTION_LIMIT_FRACTION),
            ..joint()
        };
        assert_eq!(hot.condition(&limits()), MotorCondition::Caution);

        let strained = JointTelemetry {
            load: Some(-0.9),
            ..joint()
        };
        assert_eq!(
            strained.condition(&limits()),
            MotorCondition::Caution,
            "load is judged by how hard it is pushing, not which way"
        );

        let sagging = JointTelemetry {
            voltage_volts: Some(4.5 + CAUTION_VOLTAGE_MARGIN_VOLTS - 0.1),
            ..joint()
        };
        assert_eq!(sagging.condition(&limits()), MotorCondition::Caution);

        // The motor's own fault byte outranks every margin applied here.
        let faulted = JointTelemetry {
            faults: vec![MotorFault::Overheat],
            ..joint()
        };
        assert_eq!(faulted.condition(&limits()), MotorCondition::Fault);
    }

    #[test]
    fn without_limits_only_the_motors_own_faults_can_speak() {
        let scorching = JointTelemetry {
            temperature_celsius: Some(200.0),
            ..joint()
        };

        assert_eq!(
            scorching.condition(&MotorLimits::default()),
            MotorCondition::Nominal,
            "there is nothing to be near, and a threshold invented here would be a guess"
        );
        assert_eq!(
            JointTelemetry {
                faults: vec![MotorFault::Unrecognized],
                ..scorching
            }
            .condition(&MotorLimits::default()),
            MotorCondition::Fault
        );
    }

    #[test]
    fn an_arms_condition_is_the_worst_of_its_motors() {
        assert_eq!(
            [
                MotorCondition::Nominal,
                MotorCondition::Fault,
                MotorCondition::Caution
            ]
            .into_iter()
            .max(),
            Some(MotorCondition::Fault)
        );
    }

    #[test]
    fn incremental_training_updates_are_bounded_and_change_only_the_target_run() {
        let mut snapshot = TrainingSnapshot {
            runs: vec![TrainingRun {
                id: "run-a".to_owned(),
                ..TrainingRun::default()
            }],
            ..TrainingSnapshot::default()
        };
        for step in 0..=MAX_TRAINING_METRIC_POINTS {
            snapshot.apply(TrainingUpdate::Metric {
                run_id: "run-a".to_owned(),
                point: Box::new(TrainingMetricPoint {
                    step: step as u64,
                    ..TrainingMetricPoint::default()
                }),
            });
        }
        for line in 0..=MAX_TRAINING_LOG_LINES {
            snapshot.apply(TrainingUpdate::Log {
                run_id: "run-a".to_owned(),
                line: format!("line {line}"),
            });
        }

        let run = &snapshot.runs[0];
        assert_eq!(run.metrics.len(), MAX_TRAINING_METRIC_POINTS);
        assert_eq!(run.metrics[0].step, 1);
        assert_eq!(run.logs.len(), MAX_TRAINING_LOG_LINES);
        assert_eq!(run.logs[0], "line 1");
        assert_eq!(run.current_step, MAX_TRAINING_METRIC_POINTS as u64);
    }
}
