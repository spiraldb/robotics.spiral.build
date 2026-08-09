//! The wire contract between the studio server and the web UI.
//!
//! Both sides depend on this module, so a change to a field name is a compile
//! error rather than a runtime surprise. Every type derives both halves of
//! serde because the server sends what the browser receives and vice versa.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::calibration::{Direction, MotorCalibration};

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

/// A motor as configured, with its calibration when one exists.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MotorSummary {
    pub name: String,
    pub id: u32,
    pub direction: Direction,
    pub model: Option<String>,
    pub calibration: Option<MotorCalibration>,
}

/// Serial settings for a robot's motor bus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortSettings {
    pub path: String,
    pub baud_rate: u32,
}

/// A camera assigned to a robot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CameraBinding {
    /// Role the camera plays for this robot, such as `wrist` or `top`.
    pub name: String,
    /// Stable device identifier reported by camera discovery.
    pub id: String,
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
    /// Arms being sampled.
    pub arms: Vec<String>,
    /// Largest `Follow Δ` seen so far in this take, in degrees.
    pub max_follow_error_degrees: Option<f64>,
    /// Set when the capture stopped itself — a full disk, or an arm that went
    /// away mid-take.
    pub error: Option<String>,
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

/// Change the operator's judgement of a stored take.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct JudgeEpisodeRequest {
    pub discarded: bool,
}

/// Play a stored episode back onto one arm.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartReplayRequest {
    pub episode: String,
    /// The arm to drive. It must be connected and calibrated.
    pub robot: String,
    /// Which recorded arm's trajectory to play, when the take holds more than
    /// one. Defaults to the arm being driven, then to the take's only arm.
    #[serde(default)]
    pub source_arm: Option<String>,
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
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
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
}
