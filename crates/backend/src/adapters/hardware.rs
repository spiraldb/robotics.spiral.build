//! Backend policy ports implemented by the hardware process boundary.

use std::collections::BTreeMap;

use crate::OperationError;
use spiral_robotics_hardware as hardware;
use spiral_robotics_hardware::{Calibration, CalibrationInput, NormalizedPosition};

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
