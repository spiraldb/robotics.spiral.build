//! Device application operations.

use std::collections::BTreeMap;

use crate::projects::context::ProjectContext;
use crate::robots::calibration::{
    CalibrationActions, CalibrationControlPort, CalibrationUpdate, RobotCalibrationStore,
};
use crate::robots::control::{RobotControlActions, RobotControlCommand};
use crate::robots::discovery::SerialPortDiscovery;
use crate::robots::mode::{ModeActions, OperatorMode};
use crate::robots::service::{
    AutoConnectOutcome, AutoConnectStatus, RobotActions, RobotConnectionPort, RobotStore,
};
use crate::robots::workspace::{RobotRecord, RobotWorkspace, WorkspaceError};
use crate::{BackendCommandError, OperationError};
use spiral_robotics_common::robots::{
    CameraStatus, ModeSelection, RobotSummary, SerialPortSummary,
};
use spiral_robotics_hardware::{MotorSpec, RobotSpec};

/// Loads saved configurations for hardware lifecycle actions. Startup prefetches
/// the catalog once so unavailable arms can be reported independently.
pub(crate) struct RobotAdapter<'a> {
    state: &'a ProjectContext,
    saved_records: Option<Vec<RobotRecord>>,
}

impl<'a> RobotAdapter<'a> {
    fn new(state: &'a ProjectContext) -> Self {
        Self {
            state,
            saved_records: None,
        }
    }

    async fn prefetched(state: &'a ProjectContext) -> Result<Self, OperationError> {
        Ok(Self {
            state,
            saved_records: Some(
                state
                    .workspace()
                    .try_list()
                    .await
                    .map_err(OperationError::from)?,
            ),
        })
    }

    async fn record(&self, robot: &str) -> Result<RobotRecord, WorkspaceError> {
        match &self.saved_records {
            Some(records) => records
                .iter()
                .find(|record| record.id() == robot)
                .cloned()
                .ok_or_else(|| WorkspaceError::NotFound(robot.to_owned())),
            None => self.state.workspace().get(robot).await,
        }
    }
}

impl RobotStore for RobotAdapter<'_> {
    async fn ensure_exists(&self, robot: &str) -> Result<(), OperationError> {
        self.state
            .workspace()
            .get(robot)
            .await
            .map(|_| ())
            .map_err(OperationError::from)
    }

    async fn delete(&self, robot: &str) -> Result<(), OperationError> {
        self.state
            .workspace()
            .delete(robot)
            .await
            .map_err(OperationError::from)
    }

    async fn saved_robot_ids(&self) -> Vec<String> {
        match &self.saved_records {
            Some(records) => records
                .iter()
                .map(|record| record.id().to_owned())
                .collect(),
            None => self
                .state
                .workspace()
                .list()
                .await
                .into_iter()
                .map(|record| record.id().to_owned())
                .collect(),
        }
    }

    async fn connection_spec(&self, robot: &str) -> Result<RobotSpec, OperationError> {
        self.record(robot)
            .await
            .map_err(OperationError::from)?
            .connection_spec()
    }
}

impl RobotConnectionPort for RobotAdapter<'_> {
    async fn connect(&self, specification: RobotSpec) -> Result<(), OperationError> {
        self.state
            .hardware()
            .connect(specification)
            .await
            .map_err(OperationError::conflict)
    }

    async fn forget(&self, robot: &str) -> Result<(), OperationError> {
        self.state
            .hardware()
            .forget(robot)
            .await
            .map_err(OperationError::conflict)
    }
}

impl RobotCalibrationStore for RobotWorkspace {
    async fn motor_specs(
        &self,
        robot: &str,
    ) -> Result<BTreeMap<String, MotorSpec>, OperationError> {
        self.get(robot)
            .await
            .and_then(|record| record.motors())
            .map_err(OperationError::from)
    }

    async fn save_calibration(
        &self,
        robot: &str,
        update: CalibrationUpdate<'_>,
    ) -> Result<(), OperationError> {
        RobotWorkspace::save_calibration(self, robot, update)
            .await
            .map(|_| ())
            .map_err(OperationError::from)
    }
}

impl DeviceCommands {
    /// Try every usable saved robot once, when Studio starts on this project or
    /// a settings save switches to it.
    ///
    /// A failed bus stays disconnected with its error attached; one absent arm
    /// never prevents the web interface or another arm from starting.
    pub(crate) async fn auto_connect_saved_robots(&self) {
        let adapter = match RobotAdapter::prefetched(&self.state).await {
            Ok(adapter) => adapter,
            Err(error) => {
                tracing::warn!(%error, "could not load saved robots for auto-connect");
                return;
            }
        };
        for outcome in RobotActions::new(&adapter, &adapter).auto_connect().await {
            outcome.log();
        }
    }
}

/// Project-bound operations; the context owns backend task lifetime.
#[derive(Clone)]
pub(crate) struct DeviceCommands {
    state: ProjectContext,
}

impl DeviceCommands {
    pub(super) async fn list_ports(&self) -> Result<Vec<SerialPortSummary>, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                SerialPortDiscovery::list(&state).await.map_err(Into::into)
            })
            .await
    }

    pub(super) async fn refresh_cameras(&self) -> Result<Vec<CameraStatus>, BackendCommandError> {
        self.state
            .execute(
                move |state| async move { state.cameras().refresh().await.map_err(Into::into) },
            )
            .await
    }

    pub(crate) fn telemetry(
        &self,
    ) -> Result<spiral_robotics_common::robots::TelemetryFrame, BackendCommandError> {
        Ok(self.state.hardware().state().telemetry().clone())
    }
    pub(super) fn hardware_status(
        &self,
    ) -> Result<spiral_robotics_common::robots::HardwareStatus, BackendCommandError> {
        let hardware = self.state.hardware().state();
        let mut issues = self.state.cameras().issues();
        if let Some(issue) = hardware.recording_issue() {
            issues.push(issue.to_owned());
        }
        Ok(spiral_robotics_common::robots::HardwareStatus::new(
            hardware.mode_status().clone(),
            self.state.cameras().statuses(),
            issues,
        ))
    }
    pub(super) async fn list_robots(&self) -> Result<Vec<RobotSummary>, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                let records = state
                    .workspace()
                    .try_list()
                    .await
                    .map_err(OperationError::from)?;
                let hardware = state.hardware().state();
                Ok(records
                    .iter()
                    .map(|record| record.live_summary(&hardware))
                    .collect())
            })
            .await
    }

    pub(super) async fn save_document(
        &self,
        request: spiral_robotics_common::robots::document::RobotDocumentSave,
    ) -> Result<RobotSummary, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                request
                    .document()
                    .validate()
                    .map_err(OperationError::InvalidInput)?;
                let id = request
                    .document()
                    .id()
                    .map_err(OperationError::InvalidInput)?;
                let connected = state
                    .hardware()
                    .state()
                    .connections()
                    .get(id)
                    .is_some_and(|status| status.connected());
                if connected && request.changes_control_configuration() {
                    return Err(OperationError::Conflict(
                        "Disconnect the robot before changing its port, motors, or calibration"
                            .into(),
                    )
                    .into());
                }
                let record = state
                    .workspace()
                    .save_document(&request)
                    .await
                    .map_err(OperationError::from)?;
                Ok(record.live_summary(&state.hardware().state()))
            })
            .await
    }

    pub(super) async fn get_robot(&self, id: String) -> Result<RobotSummary, BackendCommandError> {
        self.state
            .execute(move |state| async move { Self::from(state).present_robot(&id).await })
            .await
    }

    pub(super) async fn delete_robot(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                let adapter = RobotAdapter::new(&state);
                RobotActions::new(&adapter, &adapter)
                    .delete(&id)
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn connect_robot(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                let adapter = RobotAdapter::new(&state);
                RobotActions::new(&adapter, &adapter)
                    .connect(&id)
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn disconnect_robot(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state
                    .hardware()
                    .disconnect(&id)
                    .await
                    .map_err(|error| OperationError::conflict(error).into())
            })
            .await
    }

    pub(super) async fn hold_robot(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                RobotControlActions::new(state.hardware().as_ref())
                    .execute(RobotControlCommand::Hold { robot: id })
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn release_robot(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                RobotControlActions::new(state.hardware().as_ref())
                    .execute(RobotControlCommand::Release { robot: id })
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn set_robot_goal(
        &self,
        id: String,
        normalized: BTreeMap<String, f64>,
    ) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                RobotControlActions::new(state.hardware().as_ref())
                    .execute(RobotControlCommand::MoveJoints {
                        robot: id,
                        normalized,
                    })
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn start_calibration(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                CalibrationControlPort::start_calibration(state.hardware().as_ref(), &id)
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn finish_calibration(
        &self,
        id: String,
    ) -> Result<RobotSummary, BackendCommandError> {
        self.state.execute(move |state| async move {
                let output = CalibrationActions::new(
                    state.workspace().as_ref(),
                    state.hardware().as_ref(),
                )
                .finish(&id)
                .await?;
                if let Some(warning) = output.live_warning() {
                    tracing::warn!(robot = %id, "saved calibration but live refresh was incomplete: {warning}");
                }
                Self::from(state).present_robot(&id).await
            })
            .await
    }

    pub(super) async fn cancel_calibration(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                CalibrationControlPort::cancel_calibration(state.hardware().as_ref(), &id)
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn set_mode(
        &self,
        selection: ModeSelection,
    ) -> Result<(), BackendCommandError> {
        self.state.execute(move |state| async move {
                // Replay and policy need their own admission and resource-loading paths.
                let mode = match selection {
                    ModeSelection::Idle => OperatorMode::Idle,
                    ModeSelection::Manual { robot } => OperatorMode::Manual { robot },
                    ModeSelection::Teleoperate { leader, follower } => {
                        OperatorMode::Teleoperate { leader, follower }
                    }
                    ModeSelection::Replay { .. } => {
                        return Err(BackendCommandError::from(OperationError::invalid_input(
                            "start a replay with POST /api/replay, which loads the take first"
                                .to_owned(),
                        )));
                    }
                    ModeSelection::Policy { .. } => {
                        return Err(BackendCommandError::from(OperationError::invalid_input(
                            "start policy control with POST /api/rollout-runs, which validates and warms the checkpoint first"
                                .to_owned(),
                        )));
                    }
                };
                ModeActions::new(state.hardware().as_ref())
                    .set(mode)
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    async fn present_robot(&self, id: &str) -> Result<RobotSummary, BackendCommandError> {
        let record = self
            .state
            .workspace()
            .get(id)
            .await
            .map_err(OperationError::from)?;
        Ok(record.live_summary(&self.state.hardware().state()))
    }
}

impl AutoConnectOutcome {
    fn log(self) {
        let (robot, status) = self.into_parts();
        match status {
            AutoConnectStatus::Connected => {
                tracing::info!(robot = %robot, "auto-connected saved robot")
            }
            AutoConnectStatus::Skipped(error) => {
                tracing::info!(robot = %robot, "saved robot is not ready for auto-connect: {error}")
            }
            AutoConnectStatus::Failed(error) => {
                tracing::warn!(robot = %robot, "auto-connect failed: {error}")
            }
        }
    }
}

impl From<crate::projects::context::ProjectContext> for DeviceCommands {
    fn from(state: crate::projects::context::ProjectContext) -> Self {
        Self { state }
    }
}
