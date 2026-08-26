//! Robot configuration project_store backed directly by SpiralDB.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::sync::MutexRecovery;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::{
    ArrayRef, Int64Array, LargeStringArray, RecordBatch, StringArray, StringViewArray,
};
use arrow_schema::{DataType, Field, Schema};
use serde_json::{Map, Value};
use spiral_robotics_api::robots::{
    CameraBinding, ConnectionStatus, MotorLimits, MotorSummary, PortSettings, RobotSummary,
};
use spiral_robotics_core::OperationError;
use spiral_robotics_core::calibration::{Calibration, MotorSpec, MotorSpecifications};
use spiral_robotics_storage::{
    ProjectStore, ROBOT_CONFIGURATION_COLUMN, ROBOT_IDENTIFIER_COLUMN, ROBOT_UPDATED_MILLIS_COLUMN,
    StorageSchema,
};

use super::calibration::CalibrationUpdate;
use super::calibration_codec;
use super::robot_config::{ConfigError, RobotConfig, RobotConfigUpdate};
use spiral_robotics_storage::SpiralSql;

pub(crate) const DEFAULT_ROBOT_TYPE: &str = "so101_follower";
pub(crate) const DEFAULT_BAUD_RATE: u32 = 1_000_000;
const MAX_ROBOT_ID_LENGTH: usize = 64;
const MAX_LABEL_LENGTH: usize = 64;

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

struct RobotLabel(String);

impl RobotLabel {
    fn parse(field: &str, value: &str) -> Result<Self, WorkspaceError> {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.len() > MAX_LABEL_LENGTH
            || trimmed.chars().any(char::is_control)
        {
            return Err(WorkspaceError::Invalid(format!(
                "{field} must be 1 to {MAX_LABEL_LENGTH} characters and contain no control characters"
            )));
        }
        Ok(Self(trimmed.to_owned()))
    }
}

impl From<RobotLabel> for String {
    fn from(label: RobotLabel) -> Self {
        label.0
    }
}

pub(crate) struct RobotWorkspaceCreate {
    id: String,
    robot_type: Option<String>,
    port: Option<PortSettings>,
}

impl RobotWorkspaceCreate {
    pub(crate) fn new(id: String, robot_type: Option<String>) -> Self {
        Self {
            id,
            robot_type,
            port: None,
        }
    }

    pub(crate) fn with_port(mut self, port: Option<PortSettings>) -> Self {
        self.port = port;
        self
    }
}

pub(crate) struct RobotWorkspaceUpdate {
    id: String,
    robot_type: Option<String>,
    port: Option<PortSettings>,
    cameras: Option<Map<String, Value>>,
}

impl RobotWorkspaceUpdate {
    pub(crate) fn new(id: String, robot_type: Option<String>) -> Self {
        Self {
            id,
            robot_type,
            port: None,
            cameras: None,
        }
    }

    pub(crate) fn with_port(mut self, port: Option<PortSettings>) -> Self {
        self.port = port;
        self
    }

    pub(crate) fn with_cameras(mut self, cameras: Option<Map<String, Value>>) -> Self {
        self.cameras = cameras;
        self
    }
}

#[derive(Debug)]
pub(crate) enum WorkspaceError {
    InvalidId(String),
    NotFound(String),
    AlreadyExists(String),
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
            WorkspaceError::AlreadyExists(_) => Self::Conflict(message),
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
            Self::AlreadyExists(id) => write!(formatter, "a robot named {id:?} already exists"),
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
    pub(crate) fn robot_type(&self) -> Option<String> {
        self.config
            .root()
            .get("robot_type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }

    pub(crate) fn port(&self) -> Option<PortSettings> {
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

    pub(crate) fn camera_object(&self) -> Map<String, Value> {
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

    pub(crate) fn motors(&self) -> Result<BTreeMap<String, MotorSpec>, WorkspaceError> {
        let motors = self
            .config
            .motor_specs()
            .map_err(|source| {
                WorkspaceError::Invalid(format!("robot {:?} has invalid motors: {source}", self.id))
            })?
            .unwrap_or_default();
        MotorSpecifications::try_from(&motors).map_err(|source| {
            WorkspaceError::Invalid(format!("robot {:?} has invalid motors: {source}", self.id))
        })?;
        Ok(motors)
    }

    pub(crate) fn calibration(&self) -> Result<Option<Calibration>, WorkspaceError> {
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
                MotorSummary::new(name.clone(), spec.id(), spec.direction())
                    .with_model(spec.model().map(str::to_owned))
                    .with_calibration(
                        calibration
                            .as_ref()
                            .and_then(|calibration| calibration.motors().get(&name).copied()),
                    )
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
            spiral_robotics_api::robots::RobotAvailability::new(calibrated, problem, connection),
        )
        .with_port(self.port())
        .with_motors(motors)
        .with_cameras(self.cameras())
    }
}

pub(crate) struct RobotWorkspace {
    project_store: Arc<ProjectStore>,
    last_issue: Mutex<Option<String>>,
}

impl RobotWorkspace {
    pub(crate) fn new(project_store: Arc<ProjectStore>) -> Self {
        Self {
            project_store,
            last_issue: Mutex::new(None),
        }
    }

    pub(crate) fn list(&self) -> Vec<RobotRecord> {
        match self.try_list() {
            Ok(records) => {
                *self.last_issue.recover_lock() = None;
                records
            }
            Err(error) => {
                *self.last_issue.recover_lock() = Some(error.to_string());
                Vec::new()
            }
        }
    }

    pub(crate) fn issues(&self) -> Vec<String> {
        self.last_issue.recover_lock().iter().cloned().collect()
    }

    pub(crate) fn get(&self, id: &str) -> Result<RobotRecord, WorkspaceError> {
        let id = RobotId::try_from(id)?;
        self.try_list()?
            .into_iter()
            .find(|record| record.id == id.as_str())
            .ok_or(WorkspaceError::NotFound(id.0))
    }

    pub(crate) fn create(
        &self,
        request: RobotWorkspaceCreate,
    ) -> Result<RobotRecord, WorkspaceError> {
        let RobotWorkspaceCreate {
            id,
            robot_type,
            port,
        } = request;
        let id = RobotId::try_from(id.as_str())?;
        let id = id.as_str();
        match self.get(id) {
            Ok(_) => return Err(WorkspaceError::AlreadyExists(id.to_owned())),
            Err(WorkspaceError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
        let robot_type = String::from(RobotLabel::parse(
            "robot_type",
            robot_type.as_deref().unwrap_or(DEFAULT_ROBOT_TYPE),
        )?);
        let mut root = Map::new();
        root.insert("robot_id".to_owned(), Value::String(id.to_owned()));
        root.insert("robot_type".to_owned(), Value::String(robot_type));
        if let Some(port) = port.filter(|port| !port.path().trim().is_empty()) {
            root.insert("port".to_owned(), port.configuration_value());
        }
        root.insert(
            "motors".to_owned(),
            RobotConfig::default()
                .encode_motor_specs(&spiral_robotics_core::calibration::MotorSpec::so101_arm())
                .expect("stock motor specs encode"),
        );
        self.persist(id, RobotConfig::from_value(Value::Object(root))?)
    }

    pub(crate) fn import_configuration(&self, bytes: &[u8]) -> Result<RobotRecord, WorkspaceError> {
        let config = RobotConfig::from_slice(bytes).map_err(|source| {
            WorkspaceError::Invalid(format!("the configuration is not valid JSON: {source}"))
        })?;
        let id = config
            .root()
            .get("robot_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                WorkspaceError::Invalid(
                    "the configuration needs a string `robot_id` field".to_owned(),
                )
            })?
            .to_owned();
        let id = RobotId::try_from(id.as_str())?.0;
        match self.get(&id) {
            Ok(_) => return Err(WorkspaceError::AlreadyExists(id)),
            Err(WorkspaceError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
        self.persist(&id, config)
    }

    pub(crate) fn update(
        &self,
        request: RobotWorkspaceUpdate,
    ) -> Result<RobotRecord, WorkspaceError> {
        let RobotWorkspaceUpdate {
            id,
            robot_type,
            port,
            cameras,
        } = request;
        let id = id.as_str();
        let record = self.get(id)?;
        let mut root = record.config.root().clone();
        if let Some(robot_type) = robot_type.as_deref() {
            root.insert(
                "robot_type".to_owned(),
                Value::String(RobotLabel::parse("robot_type", robot_type)?.into()),
            );
        }
        if let Some(port) = port {
            let mut fields = root
                .get("port")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            fields.insert("path".to_owned(), Value::String(port.path().to_owned()));
            fields.insert("baud_rate".to_owned(), Value::from(port.baud_rate()));
            root.insert("port".to_owned(), Value::Object(fields));
        }
        if let Some(cameras) = cameras {
            root.insert("cameras".to_owned(), Value::Object(cameras));
        }
        self.persist(id, RobotConfig::from_value(Value::Object(root))?)
    }

    pub(crate) fn save_calibration(
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
        let record = self.get(id)?;
        let merged = record.config.merged(RobotConfigUpdate::new(
            record.config.encode_motor_specs(motors)?,
            calibration_codec::CalibrationCodec::encode(calibration).map_err(|source| {
                WorkspaceError::Invalid(format!("calibration could not be serialized: {source}"))
            })?,
        ))?;
        self.persist(id, merged)
    }

    pub(crate) fn delete(&self, id: &str) -> Result<(), WorkspaceError> {
        self.get(id)?;
        let relation = StorageSchema::robot_configuration_relation(self.project_store.namespace());
        let id = SpiralSql::string_literal(id);
        self.project_store
            .run_invalidating_blocking(move |connection| {
                if connection.table_exists(&relation)? {
                    connection.execute_update_sql(&format!(
                        "DELETE FROM {relation} WHERE {ROBOT_IDENTIFIER_COLUMN} = {id}"
                    ))?;
                }
                Ok(())
            })
            .map_err(WorkspaceError::Invalid)
    }

    pub(crate) fn try_list(&self) -> Result<Vec<RobotRecord>, WorkspaceError> {
        let relation = StorageSchema::robot_configuration_relation(self.project_store.namespace());
        self.project_store
            .run_blocking(move |connection| {
                if !connection.table_exists(&relation)? {
                    return Ok(Vec::new());
                }
                let reader = connection
                    .execute_query(format!(
                        "SELECT {}, {} FROM {relation} ORDER BY {}",
                        ROBOT_IDENTIFIER_COLUMN,
                        ROBOT_CONFIGURATION_COLUMN,
                        ROBOT_IDENTIFIER_COLUMN
                    ))
                    .map_err(|error| error.to_string())?;
                let mut records = Vec::new();
                for batch in reader {
                    let batch = batch.map_err(|error| error.to_string())?;
                    let ids = batch
                        .column_by_name(ROBOT_IDENTIFIER_COLUMN)
                        .ok_or_else(|| "robots result has no robot_id".to_owned())?;
                    let configs = batch
                        .column_by_name(ROBOT_CONFIGURATION_COLUMN)
                        .ok_or_else(|| "robots result has no config_json".to_owned())?;
                    for row in 0..batch.num_rows() {
                        let id = ids.as_ref().text_at(row)?;
                        let config =
                            RobotConfig::from_slice(configs.as_ref().text_at(row)?.as_bytes())
                                .map_err(|error| error.to_string())?;
                        records.push(RobotRecord { id, config });
                    }
                }
                Ok(records)
            })
            .map_err(WorkspaceError::Invalid)
    }

    fn persist(&self, id: &str, config: RobotConfig) -> Result<RobotRecord, WorkspaceError> {
        let namespace = self.project_store.namespace().to_owned();
        let relation = StorageSchema::robot_configuration_relation(&namespace);
        let config_json = config.to_pretty_json()?;
        let id_owned = id.to_owned();
        let id_sql = SpiralSql::string_literal(id);
        self.project_store
            .run_invalidating_blocking(move |connection| {
                connection.ensure_dataset_namespace(&namespace)?;
                if connection.table_exists(&relation)? {
                    connection.execute_delete_sql(&format!(
                        "DELETE FROM {relation} WHERE {ROBOT_IDENTIFIER_COLUMN} = {id_sql}"
                    ))?;
                }
                let schema = Arc::new(Schema::new(vec![
                    Field::new(ROBOT_IDENTIFIER_COLUMN, DataType::Utf8, false),
                    Field::new(ROBOT_CONFIGURATION_COLUMN, DataType::Utf8, false),
                    Field::new(ROBOT_UPDATED_MILLIS_COLUMN, DataType::Int64, false),
                ]));
                let batch = RecordBatch::try_new(
                    schema,
                    vec![
                        Arc::new(StringArray::from(vec![id_owned])) as ArrayRef,
                        Arc::new(StringArray::from(vec![config_json])) as ArrayRef,
                        Arc::new(Int64Array::from(vec![SystemTime::now().unix_millis()]))
                            as ArrayRef,
                    ],
                )
                .map_err(|error| error.to_string())?;
                connection.ingest_batch(&relation, batch)
            })
            .map_err(WorkspaceError::Invalid)?;
        Ok(RobotRecord {
            id: id.to_owned(),
            config,
        })
    }
}

trait PortConfigurationValue {
    fn configuration_value(&self) -> Value;
}

impl PortConfigurationValue for PortSettings {
    fn configuration_value(&self) -> Value {
        let mut fields = Map::new();
        fields.insert(
            "path".to_owned(),
            Value::String(self.path().trim().to_owned()),
        );
        fields.insert(
            "baud_rate".to_owned(),
            Value::from(if self.baud_rate() == 0 {
                DEFAULT_BAUD_RATE
            } else {
                self.baud_rate()
            }),
        );
        Value::Object(fields)
    }
}

trait ArrowTextValue {
    fn text_at(&self, row: usize) -> Result<String, String>;
}

impl ArrowTextValue for dyn arrow_array::Array {
    fn text_at(&self, row: usize) -> Result<String, String> {
        if self.is_null(row) {
            return Err("unexpected null text value".to_owned());
        }
        if let Some(array) = self.as_any().downcast_ref::<StringArray>() {
            return Ok(array.value(row).to_owned());
        }
        if let Some(array) = self.as_any().downcast_ref::<StringViewArray>() {
            return Ok(array.value(row).to_owned());
        }
        if let Some(array) = self.as_any().downcast_ref::<LargeStringArray>() {
            return Ok(array.value(row).to_owned());
        }
        Err(format!("expected UTF-8 text, got {}", self.data_type()))
    }
}

trait UnixMillis {
    fn unix_millis(self) -> i64;
}

impl UnixMillis for SystemTime {
    fn unix_millis(self) -> i64 {
        self.duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0)
    }
}
