//! Backend policy ports implemented by the hardware process boundary.

use std::collections::BTreeMap;

use spiral_robotics_api::robots::ModeSelection;
use spiral_robotics_core::OperationError;
use spiral_robotics_core::calibration::{Calibration, CalibrationInput, NormalizedPosition};
use spiral_robotics_hardware::HardwareHandle;

use crate::robots::calibration::CalibrationControlPort;
use crate::robots::control::RobotControlPort;
use crate::robots::mode::{ModeControlPort, OperatorMode};

impl RobotControlPort for HardwareHandle {
    async fn hold(&self, robot: &str) -> Result<(), String> {
        HardwareHandle::hold(self, robot).await
    }

    async fn release(&self, robot: &str) -> Result<(), String> {
        HardwareHandle::release(self, robot).await
    }

    async fn set_joint_goals(
        &self,
        robot: &str,
        normalized: BTreeMap<String, NormalizedPosition>,
    ) -> Result<(), String> {
        HardwareHandle::set_goals(
            self,
            robot,
            normalized
                .into_iter()
                .map(|(joint, value)| (joint, value.get()))
                .collect(),
        )
        .await
    }
}

impl ModeControlPort for HardwareHandle {
    async fn set_operator_mode(&self, mode: OperatorMode) -> Result<(), OperationError> {
        let selection = match mode {
            OperatorMode::Idle => ModeSelection::Idle,
            OperatorMode::Manual { robot } => ModeSelection::Manual { robot },
            OperatorMode::Teleoperate { leader, follower } => {
                ModeSelection::Teleoperate { leader, follower }
            }
        };
        HardwareHandle::set_mode(self, selection)
            .await
            .map_err(OperationError::conflict)
    }
}

impl CalibrationControlPort for HardwareHandle {
    async fn start_calibration(&self, robot: &str) -> Result<(), OperationError> {
        HardwareHandle::start_calibration(self, robot)
            .await
            .map_err(OperationError::conflict)
    }

    async fn calibration_input(&self, robot: &str) -> Result<CalibrationInput, OperationError> {
        HardwareHandle::calibration_input(self, robot)
            .await
            .map_err(OperationError::conflict)
    }

    async fn cancel_calibration(&self, robot: &str) -> Result<(), OperationError> {
        HardwareHandle::cancel_calibration(self, robot)
            .await
            .map_err(OperationError::conflict)
    }

    async fn apply_calibration(
        &self,
        robot: &str,
        calibration: Calibration,
    ) -> Result<(), OperationError> {
        HardwareHandle::apply_calibration(self, robot, calibration)
            .await
            .map_err(OperationError::conflict)
    }
}
