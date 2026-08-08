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
    /// Problems worth showing the operator: unreadable robot files, cameras
    /// that failed to probe, and similar non-fatal trouble.
    pub issues: Vec<String>,
    /// Monotonic counter, bumped whenever any of the above changes.
    pub revision: u64,
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
}

impl ModeSelection {
    /// Human-readable name for status text.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Manual { .. } => "Manual",
            Self::Teleoperate { .. } => "Teleoperation",
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
    pub robots: Vec<RobotTelemetry>,
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
