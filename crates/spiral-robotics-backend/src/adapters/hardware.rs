//! Backend policy ports implemented by the hardware process boundary.

use std::collections::BTreeMap;

use crate::OperationError;
use spiral_robotics_api::recording as api_recording;
use spiral_robotics_api::robots as api_robots;
use spiral_robotics_hardware as hardware;
use spiral_robotics_hardware::{
    Calibration, CalibrationInput, MotorCalibration, MotorDirection, NormalizedPosition,
};

use crate::robots::calibration::CalibrationControlPort;
use crate::robots::control::RobotControlPort;
use crate::robots::mode::{ModeControlPort, OperatorMode};

impl RobotControlPort for hardware::HardwareHandle {
    async fn hold(&self, robot: &str) -> Result<(), String> {
        hardware::HardwareHandle::hold(self, robot).await
    }

    async fn release(&self, robot: &str) -> Result<(), String> {
        hardware::HardwareHandle::release(self, robot).await
    }

    async fn set_joint_goals(
        &self,
        robot: &str,
        normalized: BTreeMap<String, NormalizedPosition>,
    ) -> Result<(), String> {
        let goals = hardware::JointGoals::new(normalized).map_err(|error| error.to_string())?;
        hardware::HardwareHandle::set_goals(self, robot, goals).await
    }
}

impl ModeControlPort for hardware::HardwareHandle {
    async fn set_operator_mode(&self, mode: OperatorMode) -> Result<(), OperationError> {
        let selection = match mode {
            OperatorMode::Idle => hardware::ModeSelection::Idle,
            OperatorMode::Manual { robot } => hardware::ModeSelection::Manual { robot },
            OperatorMode::Teleoperate { leader, follower } => {
                hardware::ModeSelection::Teleoperate { leader, follower }
            }
        };
        hardware::HardwareHandle::set_mode(self, selection)
            .await
            .map_err(OperationError::conflict)
    }
}

impl CalibrationControlPort for hardware::HardwareHandle {
    async fn start_calibration(&self, robot: &str) -> Result<(), OperationError> {
        hardware::HardwareHandle::start_calibration(self, robot)
            .await
            .map_err(OperationError::conflict)
    }

    async fn calibration_input(&self, robot: &str) -> Result<CalibrationInput, OperationError> {
        hardware::HardwareHandle::calibration_input(self, robot)
            .await
            .map_err(OperationError::conflict)
    }

    async fn cancel_calibration(&self, robot: &str) -> Result<(), OperationError> {
        hardware::HardwareHandle::cancel_calibration(self, robot)
            .await
            .map_err(OperationError::conflict)
    }

    async fn apply_calibration(
        &self,
        robot: &str,
        calibration: Calibration,
    ) -> Result<(), OperationError> {
        hardware::HardwareHandle::apply_calibration(self, robot, calibration)
            .await
            .map_err(OperationError::conflict)
    }
}

pub(crate) trait HardwareApiProjection<Api> {
    fn to_api(&self) -> Api;
}

pub(crate) trait RecordingArmProjection {
    fn to_hardware(&self) -> Vec<hardware::EpisodeArm>;
}

impl RecordingArmProjection for [api_recording::EpisodeArm] {
    fn to_hardware(&self) -> Vec<hardware::EpisodeArm> {
        self.iter()
            .map(|arm| hardware::EpisodeArm::new(arm.id(), arm.joints().to_vec()))
            .collect()
    }
}

impl HardwareApiProjection<api_robots::ConnectionStatus> for hardware::ConnectionStatus {
    fn to_api(&self) -> api_robots::ConnectionStatus {
        api_robots::ConnectionStatus::new(self.connected(), self.error().map(str::to_owned))
    }
}

impl HardwareApiProjection<api_robots::ModeStatus> for hardware::HardwareState {
    fn to_api(&self) -> api_robots::ModeStatus {
        api_robots::ModeStatus::new(
            self.mode().to_api(),
            self.mode_active(),
            self.mode_error().map(str::to_owned),
        )
    }
}

impl HardwareApiProjection<api_robots::MotorLimits> for hardware::MotorLimits {
    fn to_api(&self) -> api_robots::MotorLimits {
        api_robots::MotorLimits::from_hardware_limits(
            self.max_temperature_celsius(),
            api_robots::MotorVoltageRange::new(self.min_voltage_volts(), self.max_voltage_volts()),
            self.max_load(),
        )
    }
}

impl HardwareApiProjection<api_robots::MotorDirection> for MotorDirection {
    fn to_api(&self) -> api_robots::MotorDirection {
        match self {
            MotorDirection::Normal => api_robots::MotorDirection::Normal,
            MotorDirection::Reversed => api_robots::MotorDirection::Reversed,
        }
    }
}

impl HardwareApiProjection<api_robots::MotorCalibration> for MotorCalibration {
    fn to_api(&self) -> api_robots::MotorCalibration {
        let reading = self.reading();
        api_robots::MotorCalibration::new(
            self.id(),
            self.direction().to_api(),
            api_robots::MotorReading::new(
                reading.center(),
                reading.range_min(),
                reading.range_max(),
            ),
        )
        .expect("hardware calibration is already validated")
    }
}

impl HardwareApiProjection<api_robots::TelemetryFrame> for hardware::HardwareState {
    fn to_api(&self) -> api_robots::TelemetryFrame {
        api_robots::TelemetryFrame::new(self.sequence(), self.loop_hz(), self.target_hz())
            .with_robots(self.robots().iter().map(|robot| robot.to_api()).collect())
            .with_operations(
                self.recording().map(|recording| recording.to_api()),
                self.replay().map(|replay| replay.to_api()),
            )
    }
}

impl HardwareApiProjection<api_robots::ModeSelection> for hardware::ModeSelection {
    fn to_api(&self) -> api_robots::ModeSelection {
        match self {
            Self::Idle => api_robots::ModeSelection::Idle,
            Self::Manual { robot } => api_robots::ModeSelection::Manual {
                robot: robot.clone(),
            },
            Self::Teleoperate { leader, follower } => api_robots::ModeSelection::Teleoperate {
                leader: leader.clone(),
                follower: follower.clone(),
            },
            Self::Replay { robot, episode } => api_robots::ModeSelection::Replay {
                robot: robot.clone(),
                episode: episode.clone(),
            },
            Self::Policy { robot, session } => api_robots::ModeSelection::Policy {
                robot: robot.clone(),
                session: session.clone(),
            },
        }
    }
}

impl HardwareApiProjection<api_robots::RobotTelemetry> for hardware::RobotTelemetry {
    fn to_api(&self) -> api_robots::RobotTelemetry {
        api_robots::RobotTelemetry::new(
            self.id(),
            self.torque(),
            self.joints().iter().map(|joint| joint.to_api()).collect(),
        )
        .with_tracking_lagging(self.tracking_lagging())
        .with_error(self.error().map(str::to_owned))
        .with_calibration(self.calibration().map(|progress| progress.to_api()))
    }
}

impl HardwareApiProjection<api_robots::JointTelemetry> for hardware::JointTelemetry {
    fn to_api(&self) -> api_robots::JointTelemetry {
        let range = self
            .range()
            .map(|range| api_robots::JointRange::new(range.min(), range.center(), range.max()));
        let faults = self.faults().iter().map(|fault| fault.to_api()).collect();
        api_robots::JointTelemetry::new(
            api_robots::JointIdentity::new(self.name(), self.id()),
            api_robots::JointKinematics::new(self.raw(), self.degrees(), self.normalized())
                .with_raw_targets(self.goal_raw(), self.target_raw())
                .with_calibrated_target(self.target_degrees(), range),
            api_robots::JointDiagnostics::new(
                api_robots::MotorElectricalReadings::new(
                    self.temperature_celsius(),
                    self.voltage_volts(),
                    self.current_amps(),
                ),
                self.load(),
                self.speed_degrees_per_second(),
            )
            .with_temperature_rise(self.temperature_rise_per_minute())
            .with_faults(faults),
        )
        .with_condition(match self.condition() {
            hardware::MotorCondition::Nominal => api_robots::MotorCondition::Nominal,
            hardware::MotorCondition::Caution => api_robots::MotorCondition::Caution,
            hardware::MotorCondition::Fault => api_robots::MotorCondition::Fault,
        })
    }
}

impl HardwareApiProjection<api_robots::MotorFault> for hardware::MotorFault {
    fn to_api(&self) -> api_robots::MotorFault {
        match self {
            Self::Overheat => api_robots::MotorFault::Overheat,
            Self::Overload => api_robots::MotorFault::Overload,
            Self::Voltage => api_robots::MotorFault::Voltage,
            Self::Unrecognized => api_robots::MotorFault::Unrecognized,
        }
    }
}

impl HardwareApiProjection<api_robots::CalibrationProgress> for hardware::CalibrationProgress {
    fn to_api(&self) -> api_robots::CalibrationProgress {
        let joints = self
            .joints()
            .iter()
            .map(|joint| {
                api_robots::CalibrationJointProgress::new(
                    joint.name(),
                    joint.min(),
                    joint.position(),
                )
                .with_max(joint.max())
                .with_encoder_counts_per_turn(joint.encoder_counts_per_turn())
                .with_ready(joint.ready())
            })
            .collect();
        api_robots::CalibrationProgress::new(joints, self.ready())
    }
}

impl HardwareApiProjection<api_recording::RecordingProgress> for hardware::RecordingProgress {
    fn to_api(&self) -> api_recording::RecordingProgress {
        api_recording::RecordingProgress::new(
            api_recording::RecordingIdentity::new(
                self.episode(),
                self.dataset_id(),
                self.arms().to_vec(),
            ),
            api_recording::RecordingClock::new(
                self.frames(),
                self.elapsed_seconds(),
                self.wall_elapsed_seconds(),
            )
            .with_paused_seconds(self.paused_seconds()),
            api_recording::RecordingState::new(
                self.paused(),
                self.max_follow_error_degrees(),
                self.error().map(str::to_owned),
            ),
        )
    }
}

impl HardwareApiProjection<api_recording::ReplayProgress> for hardware::ReplayProgress {
    fn to_api(&self) -> api_recording::ReplayProgress {
        api_recording::ReplayProgress::new(
            api_recording::ReplayIdentity::new(self.episode(), self.robot()),
            api_recording::ReplayTimeline::new(self.frame(), self.frames(), self.elapsed_seconds())
                .with_duration_seconds(self.duration_seconds()),
            api_recording::ReplayTransport::new(
                match self.purpose() {
                    hardware::ReplayPurpose::Timeline => api_recording::ReplayPurpose::Timeline,
                    hardware::ReplayPurpose::PhysicalSeek => {
                        api_recording::ReplayPurpose::PhysicalSeek
                    }
                },
                match self.phase() {
                    hardware::ReplayPhase::Seeking => api_recording::ReplayPhase::Seeking,
                    hardware::ReplayPhase::Playing => api_recording::ReplayPhase::Playing,
                },
                self.paused(),
            ),
        )
    }
}
