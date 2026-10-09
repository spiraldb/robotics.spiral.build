//! Physical I/O and fixed-rate control for Spiral Robotics.
//!
//! The public boundary is deliberately smaller than the implementation. The
//! application supplies validated robot definitions and semantic control
//! requests; [`HardwareHandle`] serializes them onto the one thread that owns
//! every motor bus and publishes immutable [`HardwareState`] snapshots.
//! Calibration and the Feetech protocol live here because their invariants
//! depend on physical devices. The machine's serial driver opens the lines buses
//! hang off ([`SerialDriver`]), so this crate builds for a computer and a
//! microcontroller alike. HTTP contracts, operator workflow, episode schemas,
//! persistence, and policy inference remain in the composing application.
//!
//! Source modules are private. Their layout is an implementation detail rather
//! than a second API alongside this reviewed, flat boundary.

use std::error::Error;
use std::fmt;

mod capture_clock;
pub use capture_clock::CaptureClock;
mod bus;
mod calibration;
mod control;
mod control_policy;
mod feetech;
mod serial_driver;
mod simulated;
mod state;

pub use calibration::{CalibrationError, NormalizedPosition};
pub use control::{
    ArmConditionSummary, CONTROL_HZ, ControlSettings, DEFAULT_MAX_STEP_COUNTS,
    DEFAULT_TELEOP_MAX_STEP_COUNTS, EpisodeCheckpoint, HardwareHandle, HardwareState, JointGoals,
    PolicyGoals, RecordingCompletion, RecordingWriter, RobotSpec, Sample,
};
#[cfg(any(test, feature = "test-support"))]
pub use serial_driver::NoSerialDriver;
pub use serial_driver::{SERIAL_READ_TIMEOUT, SerialDriver, SerialLine};
pub use spiral_robotics_common::robots::{ModeSelection, MotorLimits, RobotTelemetry};
pub use state::CompletedRecording;

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
