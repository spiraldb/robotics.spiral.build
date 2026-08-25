//! Physical I/O and fixed-rate control for Spiral Robotics.
//!
//! This crate owns protocol-neutral motor-bus contracts, concrete motor
//! adapters, camera capture, serial discovery, and the single control thread
//! that coordinates connected arms. Framework-free robot and calibration
//! policy remains in `spiral-robotics-core`; delivery lives in the backend and
//! persistence lives in `spiral-robotics-storage`.

mod bus;
pub mod cameras;
mod control;
mod control_policy;
mod feetech;
pub mod serial_ports;

pub use control::{
    ArmConditionSummary, CONTROL_HZ, CONTROL_PERIOD, ControlSettings, DEFAULT_MAX_STEP_COUNTS,
    DEFAULT_TELEOP_MAX_STEP_COUNTS, EpisodeCheckpoint, HardwareHandle, HardwareState, PolicyGoals,
    RecordingCompletion, RecordingWriter, ReplayRequest, ReplayTrajectory, RobotSpec, Sample,
};
