//! Physical I/O and fixed-rate control for Spiral Robotics.
//!
//! This crate owns protocol-neutral motor-bus contracts, concrete motor
//! adapters, camera capture, serial discovery, motor calibration policy, and
//! the single control thread that coordinates connected arms. Delivery lives
//! in the backend and persistence lives in `spiral-robotics-storage`.

mod bus;
pub mod calibration;
pub mod cameras;
mod control;
mod control_policy;
mod feetech;
pub mod serial_ports;
mod state;

pub use control::{
    ArmConditionSummary, CONTROL_HZ, ControlSettings, DEFAULT_MAX_STEP_COUNTS,
    DEFAULT_TELEOP_MAX_STEP_COUNTS, EpisodeCheckpoint, HardwareHandle, HardwareState, PolicyGoals,
    RecordingCompletion, RecordingWriter, ReplayRequest, ReplayTrajectory, RobotSpec, Sample,
};
pub use state::{
    CalibrationJointProgress, CalibrationProgress, CompletedRecording, ConnectionStatus,
    EpisodeArm, JointRange, JointTelemetry, ModeSelection, ModeStatus, MotorCondition, MotorFault,
    MotorLimits, PortSettings, RecordingGap, RecordingProgress, ReplayPhase, ReplayProgress,
    ReplayPurpose, RobotTelemetry, TelemetryFrame,
};
