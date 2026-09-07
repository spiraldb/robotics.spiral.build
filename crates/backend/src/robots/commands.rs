//! Device application operations.

use std::collections::BTreeMap;

use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease};
use crate::project_session::context::ProjectContext;
use crate::robots::calibration::{
    CalibrationActions, CalibrationControlActions, CalibrationHistoryActions,
    CalibrationHistoryActivity, CalibrationHistoryPort, CalibrationUpdate, RobotCalibrationStore,
};
use crate::robots::control::{RobotControlActions, RobotControlCommand};
use crate::robots::discovery::SerialPortDiscovery;
use crate::robots::mode::{ModeActions, OperatorMode};
use crate::robots::service::{
    AutoConnectOutcome, AutoConnectStatus, CameraAssignment, RobotActions, RobotCatalogActions,
    RobotConnectionActions, RobotConnectionPort, RobotConnectionSpec, RobotStore, SavedRobot,
};
use crate::robots::workspace::{RobotRecord, RobotWorkspace, WorkspaceError};
use crate::{BackendCommandError, OperationError};
use bytes::Bytes;
use serde_json::{Map, Value};
use spiral_robotics_common::robots::{
    CameraBinding, CameraStatus, ConnectionStatus, ModeSelection, NewRobot, PortSettings,
    RobotSummary, RobotUpdate, SerialPortSummary,
};
use spiral_robotics_hardware::ModeSelection as HardwareModeSelection;
use spiral_robotics_hardware::{Calibration, MotorSpec, RobotSpec};
use spiral_robotics_storage::episodes::ReappliedEpisodePositions;

/// Adapts cached JSON robot documents and the live control thread to plain
/// capability commands. JSON preservation, wire summaries, and concrete
/// hardware specifications remain at this outer edge.
pub(crate) struct RobotAdapter<'a> {
    state: &'a ProjectContext,
    import_document: Option<Bytes>,
    saved_records: Option<Vec<RobotRecord>>,
}

impl<'a> RobotAdapter<'a> {
    pub(super) fn new(state: &'a ProjectContext) -> Self {
        Self {
            state,
            import_document: None,
            saved_records: None,
        }
    }

    pub(crate) async fn prefetched(state: &'a ProjectContext) -> Self {
        Self {
            state,
            import_document: None,
            saved_records: Some(state.workspace().list().await),
        }
    }

    fn import(state: &'a ProjectContext, document: Bytes) -> Self {
        Self {
            state,
            import_document: Some(document),
            saved_records: None,
        }
    }

    pub(crate) async fn summary(
        &self,
        robot: &str,
        connection: ConnectionStatus,
    ) -> Result<RobotSummary, WorkspaceError> {
        let record = match &self.saved_records {
            Some(records) => records
                .iter()
                .find(|record| record.id() == robot)
                .cloned()
                .ok_or_else(|| WorkspaceError::NotFound(robot.to_owned()))?,
            None => self.state.workspace().get(robot).await?,
        };
        Ok(record.summary(connection))
    }
}

impl RobotStore for RobotAdapter<'_> {
    type CameraConfiguration = Map<String, Value>;

    async fn create(&self, robot: NewRobot) -> Result<SavedRobot, OperationError> {
        let (id, robot_type, port) = robot.into_parts();
        let record = self
            .state
            .workspace()
            .create(
                crate::robots::workspace::RobotWorkspaceCreate::new(id, robot_type).with_port(port),
            )
            .await
            .map_err(OperationError::from)?;
        Ok(SavedRobot::new(record.id()))
    }

    async fn import_configuration(&self) -> Result<SavedRobot, OperationError> {
        let configuration = self
            .import_document
            .as_ref()
            .ok_or_else(|| OperationError::Internal("missing robot import document".to_owned()))?;
        let record = self
            .state
            .workspace()
            .import_configuration(configuration)
            .await
            .map_err(OperationError::from)?;
        Ok(SavedRobot::new(record.id()))
    }

    async fn camera_configuration(
        &self,
        robot: &str,
        assignments: &[CameraAssignment],
    ) -> Result<Self::CameraConfiguration, OperationError> {
        let record = self
            .state
            .workspace()
            .get(robot)
            .await
            .map_err(OperationError::from)?;
        Ok(record
            .camera_object()
            .with_camera_assignments(assignments, &self.state.cameras().statuses()))
    }

    async fn update(
        &self,
        command: RobotUpdate<Map<String, Value>>,
    ) -> Result<SavedRobot, OperationError> {
        let (id, robot_type, port, cameras) = command.into_parts();
        let record = self
            .state
            .workspace()
            .update(
                crate::robots::workspace::RobotWorkspaceUpdate::new(id, robot_type)
                    .with_port(port)
                    .with_cameras(cameras),
            )
            .await
            .map_err(OperationError::from)?;
        Ok(SavedRobot::new(record.id()))
    }

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

    async fn connection_spec(&self, robot: &str) -> Result<RobotConnectionSpec, OperationError> {
        let record = self
            .state
            .workspace()
            .get(robot)
            .await
            .map_err(OperationError::from)?;
        let port = record.port().ok_or_else(|| {
            OperationError::InvalidInput(format!(
                "robot {:?} has no serial port assigned; set one before connecting",
                record.id()
            ))
        })?;
        let motors = record.motors().map_err(OperationError::from)?;
        let calibration = record.calibration().map_err(OperationError::from)?;
        Ok(RobotConnectionSpec::new(
            record.id(),
            PortSettings::new(port.path(), port.baud_rate()),
            motors,
        )
        .with_calibration(calibration))
    }
}

impl RobotConnectionPort for RobotAdapter<'_> {
    fn is_connected(&self, robot: &str) -> bool {
        self.state
            .hardware()
            .state()
            .connections()
            .get(robot)
            .is_some_and(|connection| connection.connected())
    }

    async fn connect(&self, specification: RobotConnectionSpec) -> Result<(), OperationError> {
        let (id, port, motors, calibration) = specification.into_parts();
        let port = spiral_robotics_hardware::PortSettings::try_new(port.path(), port.baud_rate())
            .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        let specification = RobotSpec::new(id, port, motors)
            .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        let specification = match calibration {
            Some(calibration) => specification
                .with_calibration(calibration)
                .map_err(|error| OperationError::invalid_input(error.to_string()))?,
            None => specification,
        };
        self.state
            .hardware()
            .connect(specification)
            .await
            .map_err(OperationError::conflict)
    }

    async fn disconnect(&self, robot: &str) -> Result<(), OperationError> {
        self.state
            .hardware()
            .disconnect(robot)
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

struct CalibrationHistoryAdapter<'a> {
    state: &'a ProjectContext,
}

impl CalibrationHistoryPort for CalibrationHistoryAdapter<'_> {
    fn reserve_catalog_mutation(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
        self.state.catalog_lease().reserve_calibration_history(
            self.state.workflow().is_active(),
            self.state.workflow_agent().is_active(),
        )
    }

    fn release_catalog_mutation(&self, token: CatalogLeaseToken) {
        let released = self.state.catalog_lease().release(token);
        debug_assert!(
            released,
            "calibration history refresh releases its exact catalog lease once"
        );
    }

    fn activity(&self) -> CalibrationHistoryActivity {
        let hardware = self.state.hardware().state();
        CalibrationHistoryActivity::new(
            hardware
                .recording()
                .as_ref()
                .map(|recording| recording.episode().to_owned()),
            match hardware.mode() {
                HardwareModeSelection::Replay { episode, .. } => Some(episode.clone()),
                _ => None,
            },
            self.state.training().snapshot().runs().iter().any(|run| {
                matches!(
                    run.phase(),
                    spiral_robotics_common::training::TrainingRunPhase::Starting
                        | spiral_robotics_common::training::TrainingRunPhase::Running
                        | spiral_robotics_common::training::TrainingRunPhase::Stopping
                )
            }),
        )
    }

    async fn current_calibration(
        &self,
        robot: &str,
    ) -> Result<Option<Calibration>, OperationError> {
        let record = self
            .state
            .workspace()
            .get(robot)
            .await
            .map_err(OperationError::from)?;
        let motors = record.motors().map_err(OperationError::from)?;
        let calibration = record.calibration().map_err(OperationError::from)?;
        if let Some(calibration) = &calibration {
            calibration
                .validate_for_motor_specs(&motors)
            .map_err(|error| {
                OperationError::InvalidInput(format!(
                    "robot {robot:?} has calibration that does not match its configured motors: {error}"
                ))
            })?;
        }
        Ok(calibration)
    }

    async fn reapply_positions(
        &self,
        robot: String,
        calibration: Calibration,
    ) -> Result<ReappliedEpisodePositions, OperationError> {
        self.state
            .episodes()
            .reapply_calibration(&robot, &calibration)
            .await
            .map_err(OperationError::from)
    }
}

impl DeviceCommands {
    /// Try every usable saved robot once when the studio starts.
    ///
    /// A failed bus stays disconnected with its error attached; one absent arm
    /// never prevents the web interface or another arm from starting.
    pub(crate) async fn auto_connect_saved_robots(&self) {
        let Ok(_activity) = self.state.admit() else {
            return;
        };
        let adapter = RobotAdapter::new(&self.state);
        for outcome in RobotActions::new(&adapter, &adapter).auto_connect().await {
            outcome.log();
        }
    }
}

impl CatalogMutationLease {
    fn reserve_calibration_history(
        &self,
        workflow_active: bool,
        assistant_active: bool,
    ) -> Result<Option<CatalogLeaseToken>, OperationError> {
        let Some(token) = self.try_acquire(CatalogLeaseOwner::CalibrationReapply) else {
            return Ok(None);
        };
        let conflict = if workflow_active {
            Some(
                "Wait for the active Workflow run to finish before reapplying calibration to episode history.",
            )
        } else if assistant_active {
            Some(
                "Wait for the active Workflow Assistant turn to finish before reapplying calibration to episode history.",
            )
        } else {
            None
        };
        if let Some(message) = conflict {
            let released = self.release(token);
            debug_assert!(
                released,
                "rejected calibration-history admission releases its exact catalog lease"
            );
            return Err(OperationError::Conflict(message.to_owned()));
        }
        Ok(Some(token))
    }
}

/// Project-bound operations; the context owns admission and backend task lifetime.
#[derive(Clone)]
pub(crate) struct DeviceCommands {
    state: ProjectContext,
}

impl DeviceCommands {
    pub(crate) async fn list_ports(&self) -> Result<Vec<SerialPortSummary>, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                SerialPortDiscovery::list(&commands.state)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn identify_port(
        &self,
        before: Vec<String>,
    ) -> Result<spiral_robotics_common::robots::SerialPortIdentification, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                Ok(
                    spiral_robotics_common::robots::SerialPortIdentification::from(
                        SerialPortDiscovery::identify(before).await?.into_parts(),
                    ),
                )
            })
            .await?
    }

    pub(crate) async fn refresh_cameras(&self) -> Result<Vec<CameraStatus>, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.state.cameras().refresh().await.map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn list_robots(&self) -> Result<Vec<RobotSummary>, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                let adapter = RobotAdapter::prefetched(&commands.state).await;
                let hardware = commands.state.hardware().state();
                let mut summaries = Vec::new();
                for robot in RobotCatalogActions::new(&adapter).list().await {
                    let connection = hardware
                        .connections()
                        .get(robot.id())
                        .cloned()
                        .unwrap_or_default();
                    if let Ok(summary) = adapter.summary(robot.id(), connection).await {
                        summaries.push(summary);
                    }
                }
                Ok(summaries)
            })
            .await?
    }

    pub(crate) async fn create_robot(
        &self,
        request: NewRobot,
    ) -> Result<RobotSummary, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                let adapter = RobotAdapter::new(&commands.state);
                let robot = RobotCatalogActions::new(&adapter).create(request).await?;
                RobotActions::new(&adapter, &adapter)
                    .auto_connect_robot(robot.id().to_owned())
                    .await
                    .log();
                commands.present_saved_robot(robot).await
            })
            .await?
    }

    /// Add a complete unified robot JSON document as a new workspace instance.
    pub(crate) async fn import_robot_configuration(
        &self,
        document: Vec<u8>,
    ) -> Result<RobotSummary, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                if document.is_empty() {
                    return Err(BackendCommandError::from(OperationError::invalid_input(
                        "choose a robot configuration JSON file to import".to_owned(),
                    )));
                }
                let adapter = RobotAdapter::import(&commands.state, Bytes::from(document));
                let robot = RobotCatalogActions::new(&adapter)
                    .import_configuration()
                    .await?;
                RobotActions::new(&adapter, &adapter)
                    .auto_connect_robot(robot.id().to_owned())
                    .await
                    .log();
                commands.present_saved_robot(robot).await
            })
            .await?
    }

    pub(crate) async fn get_robot(&self, id: String) -> Result<RobotSummary, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                let robot = RobotCatalogActions::new(&RobotAdapter::new(&commands.state))
                    .get(id)
                    .await?;
                commands.present_saved_robot(robot).await
            })
            .await?
    }

    pub(crate) async fn update_robot(
        &self,
        request: RobotUpdate<Vec<CameraBinding>>,
    ) -> Result<RobotSummary, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                let (id, robot_type, port, camera_bindings) = request.into_parts();
                let adapter = RobotAdapter::new(&commands.state);
                let should_auto_connect = port.is_some();
                let robot = RobotActions::new(&adapter, &adapter)
                    .update(RobotUpdate::new(id, robot_type, port, camera_bindings))
                    .await?;
                if should_auto_connect {
                    RobotActions::new(&adapter, &adapter)
                        .auto_connect_robot(robot.id().to_owned())
                        .await
                        .log();
                }
                commands.present_saved_robot(robot).await
            })
            .await?
    }

    pub(crate) async fn delete_robot(&self, id: String) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                let adapter = RobotAdapter::new(&commands.state);
                RobotActions::new(&adapter, &adapter)
                    .delete(&id)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn connect_robot(&self, id: String) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                let adapter = RobotAdapter::new(&commands.state);
                RobotActions::new(&adapter, &adapter)
                    .connect(&id)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn disconnect_robot(&self, id: String) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                RobotConnectionActions::new(&RobotAdapter::new(&commands.state))
                    .disconnect(&id)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn hold_robot(&self, id: String) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                RobotControlActions::new(commands.state.hardware().as_ref())
                    .execute(RobotControlCommand::Hold { robot: id })
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn release_robot(&self, id: String) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                RobotControlActions::new(commands.state.hardware().as_ref())
                    .execute(RobotControlCommand::Release { robot: id })
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn set_robot_goal(
        &self,
        id: String,
        normalized: BTreeMap<String, f64>,
    ) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                RobotControlActions::new(commands.state.hardware().as_ref())
                    .execute(RobotControlCommand::MoveJoints {
                        robot: id,
                        normalized,
                    })
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn start_calibration(&self, id: String) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                CalibrationControlActions::new(commands.state.hardware().as_ref())
                    .start(&id)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn finish_calibration(
        &self,
        id: String,
    ) -> Result<RobotSummary, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state.tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                let output = CalibrationActions::new(
                    commands.state.workspace().as_ref(),
                    commands.state.hardware().as_ref(),
                )
                .finish(&id)
                .await?;
                if let Some(warning) = output.live_warning() {
                    tracing::warn!(robot = %id, "saved calibration but live refresh was incomplete: {warning}");
                }
                commands.present_robot(&id).await
            })
            .await?
    }

    pub(crate) async fn cancel_calibration(&self, id: String) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                CalibrationControlActions::new(commands.state.hardware().as_ref())
                    .cancel(&id)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn reapply_calibration(
        &self,
        id: String,
    ) -> Result<spiral_robotics_common::robots::CalibrationApplication, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                let outcome = CalibrationHistoryActions::new(&CalibrationHistoryAdapter {
                    state: &commands.state,
                })
                .reapply(&id)
                .await
                .map_err(BackendCommandError::from)?;
                Ok(
                    spiral_robotics_common::robots::CalibrationApplication::from(
                        outcome.into_parts(),
                    ),
                )
            })
            .await?
    }

    pub(crate) async fn set_mode(
        &self,
        selection: ModeSelection,
    ) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state.tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                // Replay and policy need their own admission and resource-loading paths.
                let mode = match selection {
                    ModeSelection::Idle => OperatorMode::Idle,
                    ModeSelection::Manual { robot } => OperatorMode::Manual { robot },
                    ModeSelection::Teleoperate { leader, follower } => {
                        OperatorMode::Teleoperate { leader, follower }
                    }
                    ModeSelection::Replay { .. } => {
                        return Err(BackendCommandError::from(OperationError::invalid_input(
                            "start a replay with POST /api/episodes/replay, which loads the take first"
                                .to_owned(),
                        )));
                    }
                    ModeSelection::Policy { .. } => {
                        return Err(BackendCommandError::from(OperationError::invalid_input(
                            "start policy control with POST /api/rollout, which validates and warms the checkpoint first"
                                .to_owned(),
                        )));
                    }
                };
                ModeActions::new(commands.state.hardware().as_ref())
                    .set(mode)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    async fn present_saved_robot(
        &self,
        robot: SavedRobot,
    ) -> Result<RobotSummary, BackendCommandError> {
        self.present_robot(robot.id()).await
    }

    async fn present_robot(&self, id: &str) -> Result<RobotSummary, BackendCommandError> {
        let connection = self
            .state
            .hardware()
            .state()
            .connections()
            .get(id)
            .cloned()
            .unwrap_or_default();
        self.state
            .workspace()
            .get(id)
            .await
            .map(|record| record.summary(connection))
            .map_err(|error| BackendCommandError::from(OperationError::from(error)))
    }

    fn require_catalog_ready(&self) -> Result<(), BackendCommandError> {
        match self.state.bootstrap().catalog_unavailable() {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }
}

/// Merge camera assignments into the robot's stored `cameras` object.
///
/// Fields this build does not understand are kept for cameras that keep their
/// role name, and the resolution of a known device is refreshed so the file
/// records what will actually be captured.
pub(crate) trait CameraConfigurationObject {
    fn with_camera_assignments(
        &self,
        assignments: &[CameraAssignment],
        discovered: &[CameraStatus],
    ) -> Map<String, Value>;
}

impl CameraConfigurationObject for Map<String, Value> {
    fn with_camera_assignments(
        &self,
        assignments: &[CameraAssignment],
        discovered: &[CameraStatus],
    ) -> Map<String, Value> {
        let existing = self;
        let mut cameras = Self::new();

        for assignment in assignments {
            let mut fields = match existing.get(assignment.role()) {
                Some(Value::Object(fields)) => fields.clone(),
                _ => Map::new(),
            };
            fields
                .entry("type".to_owned())
                .or_insert_with(|| Value::String("opencv".to_owned()));
            fields.insert(
                "id".to_owned(),
                Value::String(assignment.device_id().to_owned()),
            );
            if let Some(camera) = discovered
                .iter()
                .find(|camera| camera.id() == assignment.device_id())
            {
                fields.insert("width".to_owned(), Value::from(camera.width()));
                fields.insert("height".to_owned(), Value::from(camera.height()));
                fields.insert("fps".to_owned(), Value::from(camera.fps()));
            }
            cameras.insert(assignment.role().to_owned(), Value::Object(fields));
        }

        cameras
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

impl From<crate::project_session::context::ProjectContext> for DeviceCommands {
    fn from(state: crate::project_session::context::ProjectContext) -> Self {
        Self { state }
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    #[test]
    fn active_workflow_or_assistant_blocks_history_rewrite_and_releases_admission() {
        for (activity, expected, next_owner) in [
            (
                (true, false),
                "active Workflow run",
                CatalogLeaseOwner::WorkflowStart,
            ),
            (
                (false, true),
                "active Workflow Assistant turn",
                CatalogLeaseOwner::WorkflowAgentStart,
            ),
        ] {
            let lease = CatalogMutationLease::default();
            let error = lease
                .reserve_calibration_history(activity.0, activity.1)
                .expect_err("published activity must block a later history rewrite");
            assert!(error.to_string().contains(expected));

            let next = lease
                .try_acquire(next_owner)
                .expect("a rejected history rewrite must release its admission lease");
            assert!(lease.release(next));
        }
    }

    #[test]
    fn admitted_history_rewrite_blocks_later_workflow_and_assistant_admission() {
        let lease = CatalogMutationLease::default();
        let calibration = lease
            .reserve_calibration_history(false, false)
            .expect("idle activity check")
            .expect("calibration-history admission");

        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowStart)
                .is_none(),
            "manual Workflow cannot pass an admitted history rewrite"
        );
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowAgentStart)
                .is_none(),
            "Assistant cannot pass an admitted history rewrite"
        );
        assert!(lease.release(calibration));
    }
}
