//! Physical I/O and fixed-rate control for Spiral Robotics.
//!
//! The public boundary is deliberately smaller than the implementation. The
//! application supplies validated robot definitions and semantic control
//! requests; [`HardwareHandle`] serializes them onto the one thread that owns
//! every motor bus and publishes immutable [`HardwareState`] snapshots.
//! Calibration, device discovery, and camera capture live here because their
//! invariants depend on physical devices. HTTP contracts, operator workflow,
//! episode schemas, persistence, and policy inference remain in the composing
//! application.
//!
//! Source modules are private. Their layout is an implementation detail rather
//! than a second API alongside this reviewed, flat boundary.

use std::error::Error;
use std::fmt;

mod bus;
mod calibration;
mod cameras;
mod control;
mod control_policy;
mod feetech;
mod serial_ports;
mod state;

pub use calibration::{
    CALIBRATION_COORDINATE_SPACE, CALIBRATION_FORMAT_VERSION, CALIBRATION_UNITS, Calibration,
    CalibrationError, CalibrationFormat, CalibrationInput, MotorCalibration, MotorDirection,
    MotorReading, MotorSpec, NormalizedPosition,
};
pub use cameras::{
    CameraRuntime, DEFAULT_JPEG_QUALITY, DetectedCamera, JpegFrame, JpegStreamConfig, ProbeOutcome,
};
pub use control::{
    ArmConditionSummary, CONTROL_HZ, ControlSettings, DEFAULT_MAX_STEP_COUNTS,
    DEFAULT_TELEOP_MAX_STEP_COUNTS, EpisodeCheckpoint, HardwareHandle, HardwareState, JointGoals,
    PolicyGoals, RecordingCompletion, RecordingWriter, ReplayRequest, ReplayTrajectory, RobotSpec,
    Sample,
};
pub use serial_ports::SerialPortCatalog;
pub use state::{
    CalibrationJointProgress, CalibrationProgress, CompletedRecording, ConnectionStatus,
    EpisodeArm, JointRange, JointTelemetry, ModeSelection, MotorCondition, MotorFault, MotorLimits,
    PortSettings, RecordingGap, RecordingProgress, ReplayPhase, ReplayProgress, ReplayPurpose,
    RobotTelemetry,
};

/// A malformed value was rejected before it could enter the hardware actor.
///
/// Runtime refusals such as a disconnected robot are reported by command
/// futures. This error is reserved for construction-time preconditions, so a
/// successfully constructed boundary value remains valid for its lifetime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareInputError {
    message: String,
}

impl HardwareInputError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn validate_name(kind: &str, value: &str) -> Result<(), Self> {
        if value.is_empty()
            || value
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            Err(Self::new(format!(
                "{kind} must be nonempty and contain no whitespace or control characters"
            )))
        } else {
            Ok(())
        }
    }
}

impl fmt::Display for HardwareInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for HardwareInputError {}
