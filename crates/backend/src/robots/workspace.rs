//! Robot configuration and validation backed by the local Studio database.

use std::collections::BTreeMap;
use std::fmt;

use crate::OperationError;

use serde_json::{Map, Value};
use spiral_robotics_common::robots::{
    CameraBinding, ConnectionStatus, MotorLimits, MotorSummary, PortSettings, RobotSummary,
};
use spiral_robotics_hardware::{Calibration, MotorSpec};
use spiral_robotics_storage::{RobotDocumentWrite, StudioProject};

use super::calibration::CalibrationUpdate;
use super::calibration_codec;
use super::robot_config::{ConfigError, RobotConfig, RobotConfigUpdate};

pub(crate) const DEFAULT_BAUD_RATE: u32 = 1_000_000;
const MAX_ROBOT_ID_LENGTH: usize = 64;

struct RobotId(String);

impl RobotId {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for RobotId {
    type Error = WorkspaceError;

    fn try_from(id: &str) -> Result<Self, Self::Error> {
        if !id.is_empty()
            && id.len() <= MAX_ROBOT_ID_LENGTH
            && id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            Ok(Self(id.to_owned()))
        } else {
            Err(WorkspaceError::InvalidId(id.to_owned()))
        }
    }
}

#[derive(Debug)]
pub(crate) enum WorkspaceError {
    InvalidId(String),
    NotFound(String),
    Conflict(String),
    Invalid(String),
    Config(ConfigError),
}

impl From<WorkspaceError> for OperationError {
    fn from(error: WorkspaceError) -> Self {
        let message = error.to_string();
        match error {
            WorkspaceError::InvalidId(_) | WorkspaceError::Invalid(_) => {
                Self::InvalidInput(message)
            }
            WorkspaceError::NotFound(_) => Self::NotFound(message),
            WorkspaceError::Conflict(_) => Self::Conflict(message),
            WorkspaceError::Config(_) => Self::Internal(message),
        }
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId(id) => write!(
                formatter,
                "robot id {id:?} must be 1 to {MAX_ROBOT_ID_LENGTH} characters of letters, digits, '-', or '_'"
            ),
            Self::NotFound(id) => write!(formatter, "no robot named {id:?}"),
            Self::Conflict(message) => formatter.write_str(message),
            Self::Invalid(detail) => formatter.write_str(detail),
            Self::Config(source) => source.fmt(formatter),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<ConfigError> for WorkspaceError {
    fn from(source: ConfigError) -> Self {
        Self::Config(source)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RobotRecord {
    id: String,
    config: RobotConfig,
}

impl RobotRecord {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
}

impl RobotRecord {
    pub(super) fn live_summary(
        &self,
        hardware: &spiral_robotics_hardware::HardwareState,
    ) -> RobotSummary {
        let connection = hardware
            .connections()
            .get(self.id())
            .cloned()
            .unwrap_or_default();
        let mut summary = self.summary(connection);
        if let Some(limits) = hardware.limits().get(self.id()) {
            let motors = summary
                .motors()
                .iter()
                .cloned()
                .map(|mut motor| {
                    if let Some(limit) = limits.get(motor.name()) {
                        motor.set_limits(*limit);
                    }
                    motor
                })
                .collect();
            summary = summary.with_motors(motors);
        }
        summary
    }
    pub(super) fn connection_spec(
        &self,
    ) -> Result<spiral_robotics_hardware::RobotSpec, OperationError> {
        let port = self.port().ok_or_else(|| {
            OperationError::invalid_input(format!(
                "robot {:?} has no serial port assigned; set one before connecting",
                self.id()
            ))
        })?;
        let port = spiral_robotics_hardware::PortSettings::try_new(port.path(), port.baud_rate())
            .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        let specification = spiral_robotics_hardware::RobotSpec::new(
            self.id().to_owned(),
            port,
            self.motors().map_err(OperationError::from)?,
        )
        .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        match self.calibration().map_err(OperationError::from)? {
            Some(calibration) => specification
                .with_calibration(calibration)
                .map_err(|error| OperationError::invalid_input(error.to_string())),
            None => Ok(specification),
        }
    }

    pub(crate) fn robot_type(&self) -> Option<String> {
        self.config
            .root()
            .get("robot_type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }

    pub(super) fn port(&self) -> Option<PortSettings> {
        let port = self.config.root().get("port")?.as_object()?;
        let path = port.get("path")?.as_str()?;
        if path.trim().is_empty() {
            return None;
        }
        Some(PortSettings::new(
            path,
            port.get("baud_rate")
                .and_then(Value::as_u64)
                .and_then(|rate| u32::try_from(rate).ok())
                .filter(|rate| *rate > 0)
                .unwrap_or(DEFAULT_BAUD_RATE),
        ))
    }

    fn camera_object(&self) -> Map<String, Value> {
        self.config
            .root()
            .get("cameras")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn cameras(&self) -> Vec<CameraBinding> {
        self.camera_object()
            .into_iter()
            .filter_map(|(name, value)| {
                let fields = value.as_object()?;
                let id = fields.get("id")?.as_str()?.to_owned();
                let mount = fields
                    .get("mount")
                    .cloned()
                    .and_then(|mount| serde_json::from_value(mount).ok());
                Some(CameraBinding::new(name, id, mount))
            })
            .collect()
    }

    pub(super) fn motors(&self) -> Result<BTreeMap<String, MotorSpec>, WorkspaceError> {
        let motors = self
            .config
            .motor_specs()
            .map_err(|source| {
                WorkspaceError::Invalid(format!("robot {:?} has invalid motors: {source}", self.id))
            })?
            .unwrap_or_default();
        MotorSpec::validate_set(&motors).map_err(|source| {
            WorkspaceError::Invalid(format!("robot {:?} has invalid motors: {source}", self.id))
        })?;
        Ok(motors)
    }

    fn calibration(&self) -> Result<Option<Calibration>, WorkspaceError> {
        self.config
            .calibration()
            .filter(|value| !value.is_null())
            .map(|value| {
                calibration_codec::CalibrationCodec::decode(value.clone()).map_err(|source| {
                    WorkspaceError::Invalid(format!(
                        "robot {:?} has invalid calibration: {source}",
                        self.id
                    ))
                })
            })
            .transpose()
    }

    pub(crate) fn summary(&self, connection: ConnectionStatus) -> RobotSummary {
        let motors_result = self.motors();
        let calibration_result = self.calibration();
        let mut problem = motors_result
            .as_ref()
            .err()
            .or_else(|| calibration_result.as_ref().err())
            .map(ToString::to_string);
        let motors = motors_result.unwrap_or_default();
        let calibration = calibration_result.ok().flatten();
        if problem.is_none()
            && let Some(calibration) = &calibration
            && let Err(error) = calibration.validate_for_motor_specs(&motors)
        {
            problem = Some(format!(
                "robot {:?} has calibration that does not match its configured motors: {error}",
                self.id
            ));
        }
        let calibrated = calibration.is_some() && problem.is_none();
        let mut motors = motors
            .into_iter()
            .map(|(name, spec)| {
                MotorSummary::new(name.clone(), spec.id())
                    .with_model(spec.model().map(str::to_owned))
                    .with_limits(MotorLimits::default())
            })
            .collect::<Vec<_>>();
        motors.sort_by(|left, right| {
            left.id()
                .cmp(&right.id())
                .then_with(|| left.name().cmp(right.name()))
        });
        RobotSummary::new(
            self.id.clone(),
            self.robot_type(),
            spiral_robotics_common::robots::RobotAvailability::new(calibrated, problem, connection),
        )
        .with_port(self.port())
        .with_motors(motors)
        .with_cameras(self.cameras())
        .with_document(
            spiral_robotics_common::robots::document::RobotDocument::from_object(
                self.config.root().clone(),
            ),
        )
    }
}

pub(crate) struct RobotWorkspace {
    project_store: StudioProject,
}

impl RobotWorkspace {
    pub(crate) fn new(project_store: StudioProject) -> Self {
        Self { project_store }
    }

    pub(crate) async fn list(&self) -> Vec<RobotRecord> {
        self.try_list().await.unwrap_or_else(|error| {
            tracing::warn!(%error, "Could not load robot configurations");
            Vec::new()
        })
    }

    pub(crate) async fn get(&self, id: &str) -> Result<RobotRecord, WorkspaceError> {
        let id = RobotId::try_from(id)?;
        let document = self
            .project_store
            .robot_document(id.as_str())
            .await
            .map_err(WorkspaceError::Invalid)?
            .ok_or_else(|| WorkspaceError::NotFound(id.0.clone()))?;
        Ok(RobotRecord {
            id: id.0,
            config: RobotConfig::from_value(document)?,
        })
    }

    pub(super) async fn save_document(
        &self,
        request: &spiral_robotics_common::robots::document::RobotDocumentSave,
    ) -> Result<RobotRecord, WorkspaceError> {
        request
            .document()
            .validate()
            .map_err(WorkspaceError::Invalid)?;
        let id = request
            .document()
            .id()
            .map_err(WorkspaceError::Invalid)?
            .to_owned();
        let config = RobotConfig::from_value(Value::Object(request.document().object().clone()))?;
        let expected = request
            .expected()
            .map(|document| RobotConfig::from_value(Value::Object(document.object().clone())))
            .transpose()?;
        let record = RobotRecord { id, config };
        // Decode native motor/calibration types before saving a configuration that hardware will consume.
        if let Some(problem) = record.summary(ConnectionStatus::default()).problem() {
            return Err(WorkspaceError::Invalid(problem.to_owned()));
        }
        self.persist(record, expected.as_ref()).await
    }

    pub(super) async fn save_calibration(
        &self,
        id: &str,
        update: CalibrationUpdate<'_>,
    ) -> Result<RobotRecord, WorkspaceError> {
        let motors = update.motors();
        let calibration = update.calibration();
        calibration.validate_for_motor_specs(motors).map_err(|source| {
            WorkspaceError::Invalid(format!(
                "robot {id:?} has calibration that does not match its configured motors: {source}"
            ))
        })?;
        let record = self.get(id).await?;
        let merged = record.config.merged(RobotConfigUpdate::new(
            record.config.encode_motor_specs(motors)?,
            calibration_codec::CalibrationCodec::encode(calibration).map_err(|source| {
                WorkspaceError::Invalid(format!("calibration could not be serialized: {source}"))
            })?,
        ))?;
        self.persist(
            RobotRecord {
                id: id.to_owned(),
                config: merged,
            },
            Some(&record.config),
        )
        .await
    }

    pub(super) async fn delete(&self, id: &str) -> Result<(), WorkspaceError> {
        let id = RobotId::try_from(id)?;
        if self
            .project_store
            .delete_robot_document(id.as_str())
            .await
            .map_err(WorkspaceError::Invalid)?
        {
            Ok(())
        } else {
            Err(WorkspaceError::NotFound(id.0))
        }
    }

    pub(crate) async fn try_list(&self) -> Result<Vec<RobotRecord>, WorkspaceError> {
        self.project_store
            .robot_documents()
            .await
            .map_err(WorkspaceError::Invalid)?
            .into_iter()
            .map(|(id, document)| {
                Ok(RobotRecord {
                    id,
                    config: RobotConfig::from_value(document)?,
                })
            })
            .collect()
    }

    async fn persist(
        &self,
        record: RobotRecord,
        expected: Option<&RobotConfig>,
    ) -> Result<RobotRecord, WorkspaceError> {
        let document = serde_json::from_str(&record.config.to_pretty_json()?)
            .map_err(|error| WorkspaceError::Invalid(error.to_string()))?;
        let expected = expected
            .map(RobotConfig::to_pretty_json)
            .transpose()?
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(|error| WorkspaceError::Invalid(error.to_string()))?;
        let write = RobotDocumentWrite::new(record.id.clone(), document, expected);
        if !self
            .project_store
            .save_robot_document(write)
            .await
            .map_err(WorkspaceError::Invalid)?
        {
            return Err(WorkspaceError::Conflict(
                "Robot configuration changed; reload it before saving".into(),
            ));
        }
        Ok(record)
    }
}

#[cfg(test)]
mod tests;
