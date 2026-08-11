//! The directory of robot files the Studio reads and writes.
//!
//! One arm is one robot instance is one JSON file, exactly as the calibration
//! CLI stores it. The studio keeps the parsed documents in memory so building a
//! state snapshot never touches the disk, and every write goes through
//! [`RobotConfig`]'s atomic, compare-and-swap save.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rerobot::robot_config::{ConfigError, RobotConfig};
use rerobot_core::api::{
    CameraBinding, ConnectionStatus, MotorLimits, MotorSummary, PortSettings, RobotSummary,
};
use rerobot_core::arm;
use rerobot_core::calibration::{Calibration, MotorSpec, validate_motor_specs};
use serde_json::{Map, Value};

/// Default robot type given to a newly created instance.
pub const DEFAULT_ROBOT_TYPE: &str = "so101_follower";

/// Default motor-bus baud rate for SO-101 arms.
pub const DEFAULT_BAUD_RATE: u32 = 1_000_000;

/// Longest robot ID accepted, which is also the longest file stem written.
const MAX_ROBOT_ID_LENGTH: usize = 64;

/// Longest human-visible label stored in a robot file.
const MAX_LABEL_LENGTH: usize = 64;

/// Something went wrong reading or writing the robot workspace.
#[derive(Debug)]
pub enum WorkspaceError {
    /// The requested ID could not be used as a file name.
    InvalidId(String),
    /// No robot with that ID exists.
    NotFound(String),
    /// A robot with that ID already exists.
    AlreadyExists(String),
    /// The stored document is present but does not describe a usable robot.
    Invalid(String),
    /// The file changed underneath the studio.
    Conflict(String),
    /// The filesystem or JSON layer failed.
    Config(ConfigError),
    /// A directory operation failed.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
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
            Self::Conflict(detail) => formatter.write_str(detail),
            Self::Config(source) => source.fmt(formatter),
            Self::Io { path, source } => {
                write!(formatter, "failed to access '{}': {source}", path.display())
            }
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<ConfigError> for WorkspaceError {
    fn from(source: ConfigError) -> Self {
        Self::Config(source)
    }
}

/// One robot instance, as stored on disk.
#[derive(Clone, Debug)]
pub struct RobotRecord {
    pub id: String,
    pub path: PathBuf,
    pub config: RobotConfig,
}

impl RobotRecord {
    /// The robot's declared hardware profile.
    pub fn robot_type(&self) -> Option<String> {
        self.config
            .root()
            .get("robot_type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }

    /// The robot's motor-bus settings, when it has been assigned a port.
    pub fn port(&self) -> Option<PortSettings> {
        let port = self.config.root().get("port")?.as_object()?;
        let path = port.get("path")?.as_str()?;
        if path.trim().is_empty() {
            return None;
        }
        Some(PortSettings {
            path: path.to_owned(),
            baud_rate: port
                .get("baud_rate")
                .and_then(Value::as_u64)
                .and_then(|rate| u32::try_from(rate).ok())
                .filter(|rate| *rate > 0)
                .unwrap_or(DEFAULT_BAUD_RATE),
        })
    }

    /// The raw `cameras` object, so updates can preserve fields this build does
    /// not understand.
    pub fn camera_object(&self) -> Map<String, Value> {
        self.config
            .root()
            .get("cameras")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    /// Cameras assigned to this robot, by role name.
    pub fn cameras(&self) -> Vec<CameraBinding> {
        self.camera_object()
            .into_iter()
            .filter_map(|(name, value)| {
                let fields = value.as_object()?;
                let id = fields.get("id")?.as_str()?.to_owned();
                // A mount that does not parse is a mount the studio does not
                // have, not a reason to drop the camera: the feed still works
                // and the property card still belongs on the panel.
                let mount = fields
                    .get("mount")
                    .cloned()
                    .and_then(|mount| serde_json::from_value(mount).ok());
                Some(CameraBinding { name, id, mount })
            })
            .collect()
    }

    /// The robot's motors, or a description of why they cannot be read.
    pub fn motors(&self) -> Result<BTreeMap<String, MotorSpec>, WorkspaceError> {
        let motors = self
            .config
            .deserialize_motors::<BTreeMap<String, MotorSpec>>()
            .map_err(|source| {
                WorkspaceError::Invalid(format!("robot {:?} has invalid motors: {source}", self.id))
            })?
            .unwrap_or_default();
        validate_motor_specs(&motors).map_err(|source| {
            WorkspaceError::Invalid(format!("robot {:?} has invalid motors: {source}", self.id))
        })?;
        Ok(motors)
    }

    /// The robot's stored calibration, or a description of why it cannot be
    /// read.
    pub fn calibration(&self) -> Result<Option<Calibration>, WorkspaceError> {
        self.config
            .deserialize_calibration::<Calibration>()
            .map_err(|source| {
                WorkspaceError::Invalid(format!(
                    "robot {:?} has invalid calibration: {source}",
                    self.id
                ))
            })
    }

    /// Build the UI-facing summary, folding in live connection state.
    ///
    /// A file that parses but does not describe a usable robot still produces a
    /// summary — with `problem` explaining what is wrong, because a card that
    /// renders as an empty robot tells the operator nothing.
    pub fn summary(&self, connection: ConnectionStatus) -> RobotSummary {
        let problem = self
            .motors()
            .err()
            .or_else(|| self.calibration().err())
            .map(|error| error.to_string());
        let motors = self.motors().unwrap_or_default();
        let calibration = self.calibration().ok().flatten();
        let calibrated = calibration.as_ref().is_some_and(|calibration| {
            !motors.is_empty()
                && motors
                    .keys()
                    .all(|name| calibration.motors.contains_key(name))
        });

        let mut motors = motors
            .into_iter()
            .map(|(name, spec)| MotorSummary {
                id: spec.id,
                direction: spec.direction,
                model: spec
                    .extra
                    .get("model")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                calibration: calibration
                    .as_ref()
                    .and_then(|calibration| calibration.motors.get(&name).copied()),
                // The file does not record what a motor will fault at; the
                // motor does. A connected robot's limits are folded in where
                // the snapshot is assembled.
                limits: MotorLimits::default(),
                name,
            })
            .collect::<Vec<_>>();
        // Bus-ID order, which for an SO-101 is also the order the joints appear
        // along the arm and the order the UI shows them in.
        motors.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.name.cmp(&right.name))
        });

        RobotSummary {
            id: self.id.clone(),
            file: self
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("{}.json", self.id)),
            robot_type: self.robot_type(),
            port: self.port(),
            motors,
            cameras: self.cameras(),
            calibrated,
            problem,
            connection,
        }
    }
}

/// A directory of robot files, cached in memory.
#[derive(Debug)]
pub struct RobotWorkspace {
    directory: PathBuf,
    state: Mutex<WorkspaceState>,
}

#[derive(Debug, Default)]
struct WorkspaceState {
    robots: BTreeMap<String, RobotRecord>,
    /// Files that look like robot files but could not be read.
    issues: Vec<String>,
    /// What the directory looked like when it was last read.
    fingerprint: DirectoryFingerprint,
}

/// Enough of each robot file's metadata to notice an outside edit.
///
/// The workspace is shared with the command-line tools and with the operator's
/// editor, so the cache cannot assume it is the only writer.
type DirectoryFingerprint = Vec<(PathBuf, u64, Option<std::time::SystemTime>)>;

impl RobotWorkspace {
    /// Open `directory`, creating it if necessary, and load every robot file.
    pub fn open(directory: PathBuf) -> Result<Self, WorkspaceError> {
        fs::create_dir_all(&directory).map_err(|source| WorkspaceError::Io {
            path: directory.clone(),
            source,
        })?;
        let workspace = Self {
            directory,
            state: Mutex::new(WorkspaceState::default()),
        };
        workspace.reload()?;
        Ok(workspace)
    }

    /// The directory being managed.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Re-read the directory only when something in it has changed.
    ///
    /// The studio is not the only writer: `rerobot-calibrate` edits the same
    /// files, and an operator may drop one in by hand. Polling the metadata is
    /// cheap for a handful of small files and keeps the cache honest without
    /// making every reader touch the disk.
    pub fn reload_if_changed(&self) -> Result<bool, WorkspaceError> {
        let fingerprint = self.fingerprint()?;
        if fingerprint == self.lock().fingerprint {
            return Ok(false);
        }
        self.reload()?;
        Ok(true)
    }

    /// Re-read every robot file from disk.
    pub fn reload(&self) -> Result<(), WorkspaceError> {
        let fingerprint = self.fingerprint()?;
        let mut loaded = WorkspaceState {
            fingerprint,
            ..WorkspaceState::default()
        };

        for (path, _, _) in loaded.fingerprint.clone() {
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if validate_robot_id(id).is_err() {
                loaded.issues.push(format!(
                    "ignoring '{}': its name is not a valid robot id",
                    path.display()
                ));
                continue;
            }

            match RobotConfig::load(&path) {
                Ok(config) => {
                    loaded.robots.insert(
                        id.to_owned(),
                        RobotRecord {
                            id: id.to_owned(),
                            path: path.clone(),
                            config,
                        },
                    );
                }
                Err(source) => loaded
                    .issues
                    .push(format!("ignoring '{}': {source}", path.display())),
            }
        }

        *self.lock() = loaded;
        Ok(())
    }

    /// List the workspace's `.json` files with the metadata a change shows up
    /// in, in a stable order.
    fn fingerprint(&self) -> Result<DirectoryFingerprint, WorkspaceError> {
        let entries = fs::read_dir(&self.directory).map_err(|source| WorkspaceError::Io {
            path: self.directory.clone(),
            source,
        })?;

        let mut fingerprint = DirectoryFingerprint::new();
        for entry in entries {
            let entry = entry.map_err(|source| WorkspaceError::Io {
                path: self.directory.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            // A file that cannot be inspected still has to appear, so that it
            // becomes a reported issue rather than vanishing from the listing.
            let (length, modified) = entry.metadata().map_or((0, None), |metadata| {
                (metadata.len(), metadata.modified().ok())
            });
            fingerprint.push((path, length, modified));
        }

        fingerprint.sort();
        Ok(fingerprint)
    }

    /// Every robot, in ID order.
    pub fn list(&self) -> Vec<RobotRecord> {
        self.lock().robots.values().cloned().collect()
    }

    /// Problems found the last time the directory was read.
    pub fn issues(&self) -> Vec<String> {
        self.lock().issues.clone()
    }

    /// One robot by ID.
    pub fn get(&self, id: &str) -> Result<RobotRecord, WorkspaceError> {
        self.lock()
            .robots
            .get(id)
            .cloned()
            .ok_or_else(|| WorkspaceError::NotFound(id.to_owned()))
    }

    /// Create a robot file pre-populated with the stock SO-101 motor map.
    pub fn create(
        &self,
        id: &str,
        robot_type: Option<&str>,
        port: Option<&str>,
        baud_rate: Option<u32>,
    ) -> Result<RobotRecord, WorkspaceError> {
        validate_robot_id(id)?;
        let path = self.path_for(id);
        if self.lock().robots.contains_key(id) || path.exists() {
            return Err(WorkspaceError::AlreadyExists(id.to_owned()));
        }

        let robot_type = clean_label("robot_type", robot_type.unwrap_or(DEFAULT_ROBOT_TYPE))?;
        let mut root = Map::new();
        root.insert("robot_id".to_owned(), Value::String(id.to_owned()));
        root.insert("robot_type".to_owned(), Value::String(robot_type));
        if let Some(port) = port.map(str::trim).filter(|port| !port.is_empty()) {
            root.insert("port".to_owned(), port_value(port, baud_rate));
        }
        root.insert(
            "motors".to_owned(),
            serde_json::to_value(arm::so101_motor_specs()).expect("motor specs serialize"),
        );

        let config = RobotConfig::from_value(Value::Object(root))?;
        config.save_atomic(&path)?;

        let record = RobotRecord {
            id: id.to_owned(),
            path,
            config,
        };
        self.lock().robots.insert(id.to_owned(), record.clone());
        Ok(record)
    }

    /// Import one complete robot configuration without overwriting an existing
    /// instance. The document's `robot_id` is the file name and stable API ID;
    /// requiring it keeps a browser upload independent of the user's local
    /// download name.
    pub fn import_configuration(&self, bytes: &[u8]) -> Result<RobotRecord, WorkspaceError> {
        let value: Value = serde_json::from_slice(bytes).map_err(|source| {
            WorkspaceError::Invalid(format!("the configuration is not valid JSON: {source}"))
        })?;
        let config = RobotConfig::from_value(value)?;
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
        validate_robot_id(&id)?;

        let path = self.path_for(&id);
        if self.lock().robots.contains_key(&id) || path.exists() {
            return Err(WorkspaceError::AlreadyExists(id));
        }

        config.save_atomic(&path)?;
        let record = RobotRecord {
            id: id.clone(),
            path,
            config,
        };
        self.lock().robots.insert(id, record.clone());
        Ok(record)
    }

    /// Replace a robot's type, port, and camera assignments.
    ///
    /// Every other field, including ones this build does not understand, is
    /// preserved exactly.
    pub fn update(
        &self,
        id: &str,
        robot_type: Option<&str>,
        port: Option<&PortSettings>,
        cameras: Option<Map<String, Value>>,
    ) -> Result<RobotRecord, WorkspaceError> {
        let record = self.get(id)?;
        let mut root = record.config.root().clone();

        if let Some(robot_type) = robot_type {
            root.insert(
                "robot_type".to_owned(),
                Value::String(clean_label("robot_type", robot_type)?),
            );
        }
        if let Some(port) = port {
            let mut fields = match root.get("port") {
                Some(Value::Object(fields)) => fields.clone(),
                _ => Map::new(),
            };
            fields.insert("path".to_owned(), Value::String(port.path.clone()));
            fields.insert("baud_rate".to_owned(), Value::from(port.baud_rate));
            root.insert("port".to_owned(), Value::Object(fields));
        }
        if let Some(cameras) = cameras {
            root.insert("cameras".to_owned(), Value::Object(cameras));
        }

        self.commit(record, Value::Object(root))
    }

    /// Store a completed calibration alongside the motors it was measured for.
    pub fn save_calibration(
        &self,
        id: &str,
        motors: &BTreeMap<String, MotorSpec>,
        calibration: &Calibration,
    ) -> Result<RobotRecord, WorkspaceError> {
        let record = self.get(id)?;
        let merged = record.config.merged(
            None,
            None,
            None,
            serde_json::to_value(motors).map_err(|source| {
                WorkspaceError::Invalid(format!("motors could not be serialized: {source}"))
            })?,
            serde_json::to_value(calibration).map_err(|source| {
                WorkspaceError::Invalid(format!("calibration could not be serialized: {source}"))
            })?,
        )?;

        self.commit(record, merged.into_value())
    }

    /// Delete a robot file and forget the instance.
    pub fn delete(&self, id: &str) -> Result<(), WorkspaceError> {
        let record = self.get(id)?;
        fs::remove_file(&record.path).map_err(|source| WorkspaceError::Io {
            path: record.path.clone(),
            source,
        })?;
        // The companion lock only has meaning while the robot file exists.
        let _ = fs::remove_file(record.path.with_extension("json.lock"));
        self.lock().robots.remove(id);
        Ok(())
    }

    /// Write `value` over `record`, refusing to clobber an outside edit.
    fn commit(&self, record: RobotRecord, value: Value) -> Result<RobotRecord, WorkspaceError> {
        let updated = RobotConfig::from_value(value)?;
        match updated.save_atomic_if_unchanged(&record.path, &record.config) {
            Ok(()) => {}
            Err(ConfigError::ConcurrentModification { .. }) => {
                // Someone edited the file behind us; re-read so the next
                // attempt starts from what is actually on disk.
                self.reload()?;
                return Err(WorkspaceError::Conflict(format!(
                    "'{}' changed outside the studio and was reloaded; review it and try again",
                    record.path.display()
                )));
            }
            Err(source) => return Err(source.into()),
        }

        let record = RobotRecord {
            config: updated,
            ..record
        };
        self.lock().robots.insert(record.id.clone(), record.clone());
        Ok(record)
    }

    fn path_for(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WorkspaceState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Build the JSON value stored under a robot's `port` key.
fn port_value(path: &str, baud_rate: Option<u32>) -> Value {
    let mut fields = Map::new();
    fields.insert("path".to_owned(), Value::String(path.to_owned()));
    fields.insert(
        "baud_rate".to_owned(),
        Value::from(
            baud_rate
                .filter(|rate| *rate > 0)
                .unwrap_or(DEFAULT_BAUD_RATE),
        ),
    );
    Value::Object(fields)
}

/// Reject robot IDs that would escape the workspace or confuse the filesystem.
pub fn validate_robot_id(id: &str) -> Result<(), WorkspaceError> {
    if is_plain_name(id, MAX_ROBOT_ID_LENGTH) {
        Ok(())
    } else {
        Err(WorkspaceError::InvalidId(id.to_owned()))
    }
}

/// Reject camera role names that are not plain identifiers.
///
/// A role is a key in the robot file and a label in the UI, so the same
/// restraint that applies to a robot ID applies here.
pub fn validate_camera_role(role: &str) -> Result<(), WorkspaceError> {
    if is_plain_name(role, MAX_LABEL_LENGTH) {
        Ok(())
    } else {
        Err(WorkspaceError::Invalid(format!(
            "camera role {role:?} must be 1 to {MAX_LABEL_LENGTH} characters of letters, digits, '-', or '_'"
        )))
    }
}

fn is_plain_name(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

/// Trim and check a free-text label such as a robot type.
fn clean_label(field: &str, value: &str) -> Result<String, WorkspaceError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_LABEL_LENGTH
        || trimmed.chars().any(char::is_control)
    {
        return Err(WorkspaceError::Invalid(format!(
            "{field} must be 1 to {MAX_LABEL_LENGTH} characters and contain no control characters"
        )));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use rerobot_core::calibration::{CalibrationInput, MotorReading, build_calibration};
    use tempfile::TempDir;

    use super::*;

    fn workspace() -> (TempDir, RobotWorkspace) {
        let directory = TempDir::new().expect("temporary directory");
        let workspace =
            RobotWorkspace::open(directory.path().to_path_buf()).expect("open workspace");
        (directory, workspace)
    }

    #[test]
    fn a_file_written_by_another_tool_is_picked_up() {
        let (directory, workspace) = workspace();
        assert!(workspace.list().is_empty());

        // The calibration CLI writes to this same directory, so the studio
        // cannot treat its in-memory cache as the whole truth.
        fs::write(
            directory.path().join("outside.json"),
            r#"{"schema_version":1,"robot_id":"outside","motors":{"joint":{"id":1}}}"#,
        )
        .expect("write robot file");

        assert!(workspace.reload_if_changed().expect("reload"));
        assert_eq!(
            workspace
                .list()
                .iter()
                .map(|r| r.id.clone())
                .collect::<Vec<_>>(),
            ["outside"]
        );
        assert!(
            !workspace.reload_if_changed().expect("second reload"),
            "an unchanged directory must not be re-read"
        );

        fs::remove_file(directory.path().join("outside.json")).expect("remove");
        assert!(workspace.reload_if_changed().expect("reload after delete"));
        assert!(workspace.list().is_empty());
    }

    #[test]
    fn an_edit_to_an_existing_file_is_noticed() {
        let (directory, workspace) = workspace();
        workspace.create("arm", None, None, None).expect("create");
        workspace.reload_if_changed().expect("settle");

        let path = directory.path().join("arm.json");
        let mut document: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        document["robot_type"] = Value::String("so101_leader".to_owned());
        // A different length as well as a different mtime, so the test does not
        // depend on filesystem timestamp resolution.
        fs::write(&path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

        assert!(workspace.reload_if_changed().expect("reload"));
        assert_eq!(
            workspace.get("arm").unwrap().robot_type().as_deref(),
            Some("so101_leader")
        );
    }

    #[test]
    fn a_structurally_invalid_robot_explains_itself_instead_of_looking_empty() {
        let (directory, workspace) = workspace();
        fs::write(
            directory.path().join("clash.json"),
            r#"{"schema_version":1,"motors":{"a":{"id":1},"b":{"id":1}}}"#,
        )
        .expect("write robot file");
        workspace.reload().expect("reload");

        let summary = workspace
            .get("clash")
            .expect("the robot is listed")
            .summary(ConnectionStatus::default());

        assert!(!summary.calibrated);
        let problem = summary.problem.expect("the reason is carried to the UI");
        assert!(problem.contains("invalid motors"), "{problem}");
        assert!(problem.contains("ID 1"), "{problem}");
    }

    #[test]
    fn labels_are_trimmed_and_control_characters_refused() {
        let (_directory, workspace) = workspace();

        workspace
            .create("padded", Some("  so101_leader  "), None, None)
            .expect("create");
        assert_eq!(
            workspace.get("padded").unwrap().robot_type().as_deref(),
            Some("so101_leader"),
            "surrounding whitespace never belongs in stored data"
        );

        assert!(
            workspace
                .create("bad", Some("so101\nleader"), None, None)
                .is_err()
        );
        assert!(workspace.create("blank", Some("   "), None, None).is_err());
    }

    #[test]
    fn camera_roles_must_be_plain_identifiers() {
        for role in ["../../etc", "", "with space", "sneaky/slash"] {
            assert!(validate_camera_role(role).is_err(), "accepted {role:?}");
        }
        for role in ["wrist", "top_down", "cam-2"] {
            assert!(validate_camera_role(role).is_ok(), "rejected {role:?}");
        }
    }

    #[test]
    fn robot_ids_that_could_escape_the_directory_are_refused() {
        for id in [
            "",
            "..",
            "../evil",
            "a/b",
            "a.b",
            "with space",
            "sym~link",
            &"x".repeat(65),
        ] {
            assert!(validate_robot_id(id).is_err(), "accepted {id:?}");
        }
        for id in ["arm", "left_leader", "SO-101-1", &"x".repeat(64)] {
            assert!(validate_robot_id(id).is_ok(), "rejected {id:?}");
        }
    }

    #[test]
    fn a_new_robot_gets_the_stock_so101_motor_map() {
        let (_directory, workspace) = workspace();

        let record = workspace
            .create("left_follower", None, Some("/dev/ttyACM0"), None)
            .expect("create robot");

        assert_eq!(record.robot_type().as_deref(), Some(DEFAULT_ROBOT_TYPE));
        assert_eq!(
            record.port(),
            Some(PortSettings {
                path: "/dev/ttyACM0".to_owned(),
                baud_rate: DEFAULT_BAUD_RATE
            })
        );
        let motors = record.motors().expect("motors");
        assert_eq!(motors.len(), 6);
        assert_eq!(motors["gripper"].id, 6);
        assert!(record.path.is_file());

        let summary = record.summary(ConnectionStatus::default());
        assert!(!summary.calibrated);
        assert_eq!(summary.file, "left_follower.json");
        assert_eq!(summary.motors.len(), 6);
    }

    #[test]
    fn importing_a_configuration_preserves_the_complete_document() {
        let (directory, workspace) = workspace();
        let source = serde_json::json!({
            "schema_version": 1,
            "robot_id": "imported_leader",
            "robot_type": "so101_leader",
            "motors": rerobot_core::arm::so101_motor_specs(),
            "extension": { "keep": true }
        });

        let record = workspace
            .import_configuration(&serde_json::to_vec(&source).unwrap())
            .expect("import configuration");

        assert_eq!(record.id, "imported_leader");
        assert_eq!(record.config.root()["extension"]["keep"], true);
        assert!(directory.path().join("imported_leader.json").is_file());
        assert_eq!(
            RobotConfig::load(directory.path().join("imported_leader.json")).unwrap(),
            record.config
        );
    }

    #[test]
    fn importing_never_overwrites_an_existing_robot() {
        let (_directory, workspace) = workspace();
        let record = workspace.create("arm", None, None, None).expect("create");
        let bytes = record.config.to_pretty_json().unwrap();

        let error = workspace
            .import_configuration(bytes.as_bytes())
            .expect_err("duplicate import");

        assert!(matches!(error, WorkspaceError::AlreadyExists(id) if id == "arm"));
    }

    #[test]
    fn importing_requires_valid_json_with_a_robot_id() {
        let (_directory, workspace) = workspace();

        assert!(workspace.import_configuration(b"not json").is_err());
        assert!(
            workspace
                .import_configuration(br#"{"schema_version":1}"#)
                .is_err()
        );
        assert!(
            workspace
                .import_configuration(br#"{"robot_id":"../escape"}"#)
                .is_err()
        );
    }

    #[test]
    fn creating_a_duplicate_robot_is_refused() {
        let (_directory, workspace) = workspace();
        workspace.create("arm", None, None, None).expect("create");

        assert!(matches!(
            workspace.create("arm", None, None, None),
            Err(WorkspaceError::AlreadyExists(_))
        ));
        assert!(matches!(
            workspace.create("../escape", None, None, None),
            Err(WorkspaceError::InvalidId(_))
        ));
    }

    #[test]
    fn updates_preserve_unrelated_and_unknown_fields() {
        let (directory, workspace) = workspace();
        workspace
            .create("arm", None, Some("/dev/old"), Some(57_600))
            .expect("create");
        // Simulate a field written by a future version of the tool.
        let path = directory.path().join("arm.json");
        let mut value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["experimental"] = serde_json::json!({"keep": true});
        fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        workspace.reload().expect("reload");

        let cameras = Map::from_iter([(
            "wrist".to_owned(),
            serde_json::json!({"type": "opencv", "id": "/dev/video0"}),
        )]);
        let record = workspace
            .update(
                "arm",
                Some("so101_leader"),
                Some(&PortSettings {
                    path: "/dev/new".to_owned(),
                    baud_rate: 1_000_000,
                }),
                Some(cameras),
            )
            .expect("update");

        assert_eq!(record.robot_type().as_deref(), Some("so101_leader"));
        assert_eq!(record.port().unwrap().path, "/dev/new");
        assert_eq!(record.port().unwrap().baud_rate, 1_000_000);
        assert_eq!(
            record.cameras(),
            [CameraBinding {
                name: "wrist".to_owned(),
                id: "/dev/video0".to_owned(),
                mount: None,
            }]
        );
        assert_eq!(record.config.root()["experimental"]["keep"], true);
        assert_eq!(record.motors().expect("motors").len(), 6);
    }

    #[test]
    fn saving_a_calibration_marks_the_robot_calibrated() {
        let (_directory, workspace) = workspace();
        let record = workspace.create("arm", None, None, None).expect("create");
        let motors = record.motors().expect("motors");
        let input = CalibrationInput::Direct {
            motors: motors
                .keys()
                .map(|name| {
                    (
                        name.clone(),
                        MotorReading {
                            center: 2048,
                            range_min: 1000,
                            range_max: 3000,
                        },
                    )
                })
                .collect(),
        };
        let calibration = build_calibration(&motors, &input).expect("calibration");

        let record = workspace
            .save_calibration("arm", &motors, &calibration)
            .expect("save calibration");

        assert!(record.summary(ConnectionStatus::default()).calibrated);
        assert_eq!(
            record.calibration().unwrap().unwrap().motors["gripper"].center,
            2048
        );
        // The saved document is what a fresh reader sees.
        workspace.reload().expect("reload");
        assert!(
            workspace
                .get("arm")
                .unwrap()
                .summary(ConnectionStatus::default())
                .calibrated
        );
    }

    #[test]
    fn deleting_a_robot_removes_its_file_and_lock() {
        let (directory, workspace) = workspace();
        let record = workspace.create("arm", None, None, None).expect("create");
        assert!(directory.path().join("arm.json.lock").is_file());

        workspace.delete("arm").expect("delete");

        assert!(!record.path.exists());
        assert!(!directory.path().join("arm.json.lock").exists());
        assert!(matches!(
            workspace.get("arm"),
            Err(WorkspaceError::NotFound(_))
        ));
    }

    #[test]
    fn unreadable_files_are_reported_rather_than_hidden() {
        let (directory, workspace) = workspace();
        fs::write(directory.path().join("broken.json"), "{ not json").unwrap();
        fs::write(directory.path().join("not a robot.json"), "{}").unwrap();
        fs::write(directory.path().join("notes.txt"), "ignored").unwrap();

        workspace.reload().expect("reload");

        let issues = workspace.issues();
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues.iter().any(|issue| issue.contains("broken.json")));
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("not a robot.json"))
        );
        assert!(workspace.list().is_empty());
    }

    #[test]
    fn an_outside_edit_is_reported_instead_of_being_overwritten() {
        let (directory, workspace) = workspace();
        workspace.create("arm", None, None, None).expect("create");

        // Another process rewrites the file after the studio cached it.
        let path = directory.path().join("arm.json");
        let mut value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["robot_type"] = Value::String("edited_elsewhere".to_owned());
        RobotConfig::from_value(value)
            .unwrap()
            .save_atomic(&path)
            .unwrap();

        let error = workspace
            .update("arm", Some("studio_wins"), None, None)
            .expect_err("a stale write must be refused");

        assert!(matches!(error, WorkspaceError::Conflict(_)));
        assert_eq!(
            workspace.get("arm").unwrap().robot_type().as_deref(),
            Some("edited_elsewhere"),
            "the outside edit survives and is reloaded"
        );
    }
}
