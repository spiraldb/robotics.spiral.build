//! Safe, schema-aware storage for a robot's unified JSON configuration.
//!
//! [`RobotConfig`] keeps the complete JSON object in memory, so fields that a
//! particular command does not understand survive an update.

use std::error::Error;
use std::fmt;
#[cfg(test)]
use std::fs::{self, File, OpenOptions};
#[cfg(test)]
use std::io::{self, BufReader, Write};
#[cfg(test)]
use std::path::{Path, PathBuf};

use serde::Deserialize;
#[cfg(test)]
use serde::de::DeserializeOwned;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use spiral_robotics_core::calibration::{MotorDirection, MotorSpec};
#[cfg(test)]
use tempfile::NamedTempFile;

/// The newest robot-configuration schema understood by this crate.
pub(crate) const SCHEMA_VERSION: u64 = 1;

/// A validated robot configuration document.
///
/// The document's root is always an object and always contains the current
/// `schema_version`.  Loading a legacy object without a version treats it as
/// schema version 1 and adds the version to the in-memory document.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RobotConfig {
    root: Map<String, Value>,
}

pub(crate) struct RobotConfigUpdate {
    robot_id: Option<String>,
    robot_type: Option<String>,
    port: Option<Value>,
    motors: Value,
    calibration: Value,
}

impl RobotConfigUpdate {
    pub(crate) fn new(motors: Value, calibration: Value) -> Self {
        Self {
            robot_id: None,
            robot_type: None,
            port: None,
            motors,
            calibration,
        }
    }

    #[cfg(test)]
    fn identity(mut self, robot_id: Option<&str>, robot_type: Option<&str>) -> Self {
        self.robot_id = robot_id.map(str::to_owned);
        self.robot_type = robot_type.map(str::to_owned);
        self
    }

    #[cfg(test)]
    fn port(mut self, port: Option<Value>) -> Self {
        self.port = port;
        self
    }
}

/// An error encountered while loading, updating, or saving a robot config.
#[derive(Debug)]
pub(crate) enum ConfigError {
    /// A filesystem operation failed.
    #[cfg(test)]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    /// JSON parsing, serialization, or typed deserialization failed.
    Json {
        context: String,
        source: serde_json::Error,
    },
    /// The JSON document's root was not an object.
    RootMustBeObject,
    /// `schema_version` was not a supported positive integer.
    InvalidSchemaVersion(Value),
    /// A config path existed but was a symlink or another non-regular file.
    #[cfg(test)]
    UnsafeTarget { path: PathBuf, kind: &'static str },
    /// The config changed after the caller loaded the expected document.
    #[cfg(test)]
    ConcurrentModification { path: PathBuf },
    /// The replacement committed, but synchronizing its directory failed.
    #[cfg(test)]
    CommittedButNotDurable {
        path: PathBuf,
        directory: PathBuf,
        source: io::Error,
    },
    /// A port supplied to [`RobotConfig::merged`] was not an object.
    PortMustBeObject,
    /// A typed motor map could not be merged into the existing raw document.
    MotorsMustBeObject,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(test)]
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} '{}': {source}",
                path.display()
            ),
            Self::Json { context, source } => {
                write!(formatter, "invalid JSON {context}: {source}")
            }
            Self::RootMustBeObject => {
                formatter.write_str("robot configuration root must be a JSON object")
            }
            Self::InvalidSchemaVersion(version) => write!(
                formatter,
                "unsupported schema_version {version}; expected integer {SCHEMA_VERSION}"
            ),
            #[cfg(test)]
            Self::UnsafeTarget { path, kind } => write!(
                formatter,
                "refusing robot configuration target '{}': target is {kind}",
                path.display()
            ),
            #[cfg(test)]
            Self::ConcurrentModification { path } => write!(
                formatter,
                "robot configuration '{}' changed while calibration was in progress; reload it and try again",
                path.display()
            ),
            #[cfg(test)]
            Self::CommittedButNotDurable {
                path,
                directory,
                source,
            } => write!(
                formatter,
                "robot configuration '{}' was replaced, but synchronizing directory '{}' failed; the update is visible but crash durability is uncertain: {source}",
                path.display(),
                directory.display()
            ),
            Self::PortMustBeObject => {
                formatter.write_str("robot configuration port must be a JSON object")
            }
            Self::MotorsMustBeObject => {
                formatter.write_str("robot configuration motors must be a JSON object")
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            #[cfg(test)]
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            #[cfg(test)]
            Self::CommittedButNotDurable { source, .. } => Some(source),
            Self::RootMustBeObject
            | Self::InvalidSchemaVersion(_)
            | Self::PortMustBeObject
            | Self::MotorsMustBeObject => None,
            #[cfg(test)]
            Self::UnsafeTarget { .. } | Self::ConcurrentModification { .. } => None,
        }
    }
}

impl ConfigError {
    fn invalid_json(context: impl Into<String>, message: &str) -> Self {
        Self::Json {
            context: context.into(),
            source: <serde_json::Error as serde::de::Error>::custom(message),
        }
    }
}

impl Default for RobotConfig {
    fn default() -> Self {
        let mut root = Map::new();
        root.insert(
            "schema_version".to_owned(),
            Value::Number(Number::from(SCHEMA_VERSION)),
        );
        Self { root }
    }
}

impl RobotConfig {
    /// Load and validate `path`, or return a new version-1 document if it does
    /// not exist.
    ///
    /// Existing symlinks, directories, sockets, and other non-regular targets
    /// are refused.  A missing `schema_version` is accepted as version 1;
    /// malformed, zero, negative, fractional, and newer versions are rejected.
    #[cfg(test)]
    fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let Some(file) = ConfigFile::new(path).open_regular()? else {
            return Ok(Self::default());
        };
        let StrictValue(value) =
            serde_json::from_reader(BufReader::new(file)).map_err(|source| ConfigError::Json {
                context: format!("in '{}'", path.display()),
                source,
            })?;

        Self::from_value(value)
    }

    /// Parse an in-memory document while rejecting duplicate object keys at any
    /// depth before a generic JSON value could silently overwrite one.
    pub(crate) fn from_slice(bytes: &[u8]) -> Result<Self, ConfigError> {
        let StrictValue(value) =
            serde_json::from_slice(bytes).map_err(|source| ConfigError::Json {
                context: "in imported robot configuration".to_owned(),
                source,
            })?;
        Self::from_value(value)
    }

    /// Construct a document from an in-memory JSON value.
    pub(crate) fn from_value(value: Value) -> Result<Self, ConfigError> {
        let Value::Object(mut root) = value else {
            return Err(ConfigError::RootMustBeObject);
        };

        Self::validate_schema(&root)?;
        root.insert(
            "schema_version".to_owned(),
            Value::Number(Number::from(SCHEMA_VERSION)),
        );
        Ok(Self { root })
    }

    /// Borrow the complete root object, including fields unknown to this crate.
    pub(crate) fn root(&self) -> &Map<String, Value> {
        &self.root
    }

    /// Borrow the existing top-level `calibration` value, if present.
    pub(crate) fn calibration(&self) -> Option<&Value> {
        self.root.get("calibration")
    }

    /// Borrow the existing top-level `motors` value, if present.
    pub(crate) fn motors(&self) -> Option<&Value> {
        self.root.get("motors")
    }

    /// Deserialize the existing calibration into a caller-selected type.
    #[cfg(test)]
    fn deserialize_calibration<T>(&self) -> Result<Option<T>, ConfigError>
    where
        T: DeserializeOwned,
    {
        self.calibration().deserialize_optional("in calibration")
    }

    /// Deserialize the existing motor specifications into an
    /// caller-selected type.
    #[cfg(test)]
    fn deserialize_motors<T>(&self) -> Result<Option<T>, ConfigError>
    where
        T: DeserializeOwned,
    {
        self.motors().deserialize_optional("in motors")
    }

    /// Decode domain motor specifications while keeping raw unknown fields in
    /// this document for the next read-modify-write.
    pub(crate) fn motor_specs(
        &self,
    ) -> Result<Option<std::collections::BTreeMap<String, MotorSpec>>, ConfigError> {
        let Some(value) = self.motors().filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        let object = value.as_object().ok_or(ConfigError::MotorsMustBeObject)?;
        object
            .iter()
            .map(|(name, value)| {
                let context = format!("for motor {name:?}");
                let fields = value.as_object().ok_or_else(|| {
                    ConfigError::invalid_json(
                        context.clone(),
                        "motor specification must be an object",
                    )
                })?;
                let id = fields
                    .get("id")
                    .and_then(Value::as_u64)
                    .and_then(|id| u32::try_from(id).ok())
                    .ok_or_else(|| {
                        ConfigError::invalid_json(
                            context.clone(),
                            "motor id must be a non-negative 32-bit integer",
                        )
                    })?;
                let direction = match fields.get("direction") {
                    None => MotorDirection::Normal,
                    Some(Value::String(direction)) if direction == "normal" => {
                        MotorDirection::Normal
                    }
                    Some(Value::String(direction)) if direction == "reversed" => {
                        MotorDirection::Reversed
                    }
                    Some(_) => {
                        return Err(ConfigError::invalid_json(
                            context,
                            "motor direction must be 'normal' or 'reversed'",
                        ));
                    }
                };
                let model = match fields.get("model") {
                    None => None,
                    Some(Value::String(model)) => Some(model.clone()),
                    Some(_) => {
                        return Err(ConfigError::invalid_json(
                            context,
                            "motor model must be a string",
                        ));
                    }
                };
                let specification = model.map_or_else(
                    || MotorSpec::new(id, direction),
                    |model| MotorSpec::new(id, direction).with_model(model),
                );
                Ok((name.clone(), specification))
            })
            .collect::<Result<_, _>>()
            .map(Some)
    }

    /// Merge domain motor specifications into the raw robot-file representation.
    ///
    /// Fields unknown to this build survive for existing motors. Removed motors
    /// are removed, and new motors receive only the explicit domain fields.
    pub(crate) fn encode_motor_specs(
        &self,
        specs: &std::collections::BTreeMap<String, MotorSpec>,
    ) -> Result<Value, ConfigError> {
        let existing = match self.motors() {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(motors)) => motors.clone(),
            Some(_) => return Err(ConfigError::MotorsMustBeObject),
        };
        let motors = specs
            .iter()
            .map(|(name, spec)| {
                let mut fields = existing
                    .get(name)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                fields.insert("id".to_owned(), Value::from(spec.id()));
                fields.insert(
                    "direction".to_owned(),
                    Value::String(
                        match spec.direction() {
                            MotorDirection::Normal => "normal",
                            MotorDirection::Reversed => "reversed",
                        }
                        .to_owned(),
                    ),
                );
                match spec.model() {
                    Some(model) => {
                        fields.insert("model".to_owned(), Value::String(model.to_owned()));
                    }
                    None => {
                        fields.remove("model");
                    }
                }
                (name.clone(), Value::Object(fields))
            })
            .collect();
        Ok(Value::Object(motors))
    }

    /// Return an updated clone while preserving every unrelated field.
    ///
    /// `robot_id`, `robot_type`, and `port` are replaced only when supplied;
    /// omitting one preserves an existing value.  A supplied port must be a
    /// JSON object.  `motors` and `calibration` are always replaced.
    pub(crate) fn merged(&self, update: RobotConfigUpdate) -> Result<Self, ConfigError> {
        if update.port.as_ref().is_some_and(|value| !value.is_object()) {
            return Err(ConfigError::PortMustBeObject);
        }

        let mut root = self.root.clone();
        root.insert(
            "schema_version".to_owned(),
            Value::Number(Number::from(SCHEMA_VERSION)),
        );

        if let Some(robot_id) = update.robot_id {
            root.insert("robot_id".to_owned(), Value::String(robot_id));
        }
        if let Some(robot_type) = update.robot_type {
            root.insert("robot_type".to_owned(), Value::String(robot_type));
        }
        if let Some(port) = update.port {
            root.insert("port".to_owned(), port);
        }
        root.insert("motors".to_owned(), update.motors);
        root.insert("calibration".to_owned(), update.calibration);

        Self::from_value(Value::Object(root))
    }

    /// Serialize the complete document as deterministic pretty JSON ending in
    /// exactly one newline.
    pub(crate) fn to_pretty_json(&self) -> Result<String, ConfigError> {
        let canonical = Value::Object(self.root.clone()).canonicalized();
        let mut json =
            serde_json::to_string_pretty(&canonical).map_err(|source| ConfigError::Json {
                context: "while serializing robot configuration".to_owned(),
                source,
            })?;
        json.push('\n');
        Ok(json)
    }

    /// Atomically save the document to `path`.
    ///
    /// Data is written to a private temporary file in the destination
    /// directory, flushed and synchronized, and then renamed over the target.
    /// On Unix, an existing file's mode is retained, a new file is mode 0600,
    /// and the containing directory is synchronized after the rename. Other
    /// platforms receive the strongest replacement and flush guarantees
    /// exposed by the standard library and `tempfile`.
    #[cfg(test)]
    fn save_atomic(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let path = path.as_ref();
        let _lock_guard = ConfigFile::new(path).acquire_lock()?;
        self.save_atomic_unlocked(path)
    }

    /// Save only if `path` still contains `expected`.
    ///
    /// A persistent companion lock serializes cooperating writers. The file is
    /// reloaded while that lock is held, so a caller that loaded `expected`,
    /// performed a long-running calibration, and then calls this method cannot
    /// silently overwrite intervening updates. The lock file is intentionally
    /// retained: deleting it would allow different processes to lock different
    /// inodes for the same configuration path.
    #[cfg(test)]
    fn save_atomic_if_unchanged(
        &self,
        path: impl AsRef<Path>,
        expected: &Self,
    ) -> Result<(), ConfigError> {
        let path = path.as_ref();
        let _lock_guard = ConfigFile::new(path).acquire_lock()?;
        let current = Self::load(path)?;
        if current != *expected {
            return Err(ConfigError::ConcurrentModification {
                path: path.to_owned(),
            });
        }

        self.save_atomic_unlocked(path)
    }

    #[cfg(test)]
    fn save_atomic_unlocked(&self, path: &Path) -> Result<(), ConfigError> {
        self.save_atomic_unlocked_with_sync(path, |parent| ConfigFile::new(parent).sync_directory())
    }

    #[cfg(test)]
    fn save_atomic_unlocked_with_sync<F>(
        &self,
        path: &Path,
        sync_directory: F,
    ) -> Result<(), ConfigError>
    where
        F: FnOnce(&Path) -> io::Result<()>,
    {
        Self::validate_schema(&self.root)?;

        let existing_metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => {
                ConfigFile::new(path).validate_regular_target(&metadata)?;
                Some(metadata)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(source) => return Err(ConfigFile::new(path).io_error("inspect", source)),
        };

        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut temporary = NamedTempFile::new_in(parent).map_err(|source| {
            ConfigFile::new(parent).io_error("create temporary file in", source)
        })?;

        let json = self.to_pretty_json()?;
        temporary
            .as_file_mut()
            .write_all(json.as_bytes())
            .map_err(|source| ConfigFile::new(path).io_error("write temporary file for", source))?;
        temporary
            .as_file_mut()
            .flush()
            .map_err(|source| ConfigFile::new(path).io_error("flush temporary file for", source))?;
        ConfigFile::new(temporary.path())
            .set_temporary_permissions(temporary.as_file(), existing_metadata.as_ref())?;
        temporary.as_file().sync_all().map_err(|source| {
            ConfigFile::new(path).io_error("synchronize temporary file for", source)
        })?;

        temporary
            .persist(path)
            .map_err(|error| ConfigFile::new(path).io_error("atomically replace", error.error))?;

        sync_directory(parent).map_err(|source| ConfigError::CommittedButNotDurable {
            path: path.to_owned(),
            directory: parent.to_owned(),
            source,
        })?;
        Ok(())
    }

    fn validate_schema(root: &Map<String, Value>) -> Result<(), ConfigError> {
        let Some(version) = root.get("schema_version") else {
            return Ok(());
        };

        match version.as_u64() {
            Some(SCHEMA_VERSION) => Ok(()),
            _ => Err(ConfigError::InvalidSchemaVersion(version.clone())),
        }
    }
}

#[cfg(test)]
struct ConfigFile<'a> {
    path: &'a Path,
}

#[cfg(test)]
impl<'a> ConfigFile<'a> {
    fn new(path: &'a Path) -> Self {
        Self { path }
    }

    fn validate_regular_target(&self, metadata: &fs::Metadata) -> Result<(), ConfigError> {
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(ConfigError::UnsafeTarget {
                path: self.path.to_owned(),
                kind: "a symbolic link",
            });
        }
        if !file_type.is_file() {
            return Err(ConfigError::UnsafeTarget {
                path: self.path.to_owned(),
                kind: "not a regular file",
            });
        }
        Ok(())
    }

    #[cfg(unix)]
    fn open_regular(&self) -> Result<Option<File>, ConfigError> {
        use std::os::unix::fs::OpenOptionsExt;

        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);

        let file = match options.open(self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                if error.raw_os_error() == Some(libc::ELOOP) {
                    return Err(ConfigError::UnsafeTarget {
                        path: self.path.to_owned(),
                        kind: "a symbolic link",
                    });
                }
                if let Ok(metadata) = fs::symlink_metadata(self.path) {
                    self.validate_regular_target(&metadata)?;
                }
                return Err(self.io_error("open", error));
            }
        };

        let metadata = file
            .metadata()
            .map_err(|source| self.io_error("inspect opened", source))?;
        self.validate_regular_target(&metadata)?;
        Ok(Some(file))
    }

    #[cfg(not(unix))]
    fn open_regular(&self) -> Result<Option<File>, ConfigError> {
        let metadata = match fs::symlink_metadata(self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(self.io_error("inspect", source)),
        };
        self.validate_regular_target(&metadata)?;

        let file = File::open(self.path).map_err(|source| self.io_error("open", source))?;
        let opened_metadata = file
            .metadata()
            .map_err(|source| self.io_error("inspect opened", source))?;
        self.validate_regular_target(&opened_metadata)?;
        Ok(Some(file))
    }

    fn companion_lock_path(&self) -> Result<PathBuf, ConfigError> {
        let Some(file_name) = self.path.file_name().filter(|name| !name.is_empty()) else {
            return Err(ConfigError::UnsafeTarget {
                path: self.path.to_owned(),
                kind: "a path without a file name",
            });
        };

        let mut lock_name = file_name.to_os_string();
        lock_name.push(".lock");
        Ok(self.path.with_file_name(lock_name))
    }

    fn acquire_lock(&self) -> Result<File, ConfigError> {
        let lock_path = self.companion_lock_path()?;
        let lock_file = ConfigFile::new(&lock_path);
        match fs::symlink_metadata(&lock_path) {
            Ok(metadata) => lock_file.validate_regular_target(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(lock_file.io_error("inspect companion lock", source)),
        }

        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }

        let lock = match options.open(&lock_path) {
            Ok(lock) => lock,
            #[cfg(unix)]
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                return Err(ConfigError::UnsafeTarget {
                    path: lock_path,
                    kind: "a symbolic link",
                });
            }
            Err(source) => return Err(lock_file.io_error("open companion lock", source)),
        };
        let metadata = lock
            .metadata()
            .map_err(|source| lock_file.io_error("inspect opened companion lock", source))?;
        lock_file.validate_regular_target(&metadata)?;
        lock.lock()
            .map_err(|source| lock_file.io_error("lock companion file", source))?;
        Ok(lock)
    }
}

#[cfg(test)]
trait OptionalJsonValue {
    fn deserialize_optional<T>(self, context: &str) -> Result<Option<T>, ConfigError>
    where
        T: DeserializeOwned;
}

#[cfg(test)]
impl OptionalJsonValue for Option<&Value> {
    fn deserialize_optional<T>(self, context: &str) -> Result<Option<T>, ConfigError>
    where
        T: DeserializeOwned,
    {
        if self.is_none_or(Value::is_null) {
            return Ok(None);
        }

        self.cloned()
            .map(|value| {
                serde_json::from_value(value).map_err(|source| ConfigError::Json {
                    context: context.to_owned(),
                    source,
                })
            })
            .transpose()
    }
}

/// A JSON value decoder that refuses duplicate keys recursively.
struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StrictValueVisitor;

        impl<'de> Visitor<'de> for StrictValueVisitor {
            type Value = StrictValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON value without duplicate object keys")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Bool(value)))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(value.into())))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(value.into())))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Number::from_f64(value)
                    .map(Value::Number)
                    .map(StrictValue)
                    .ok_or_else(|| E::custom("JSON numbers must be finite"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(value.to_owned())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(value)))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                StrictValue::deserialize(deserializer)
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(StrictValue(value)) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(StrictValue(Value::Array(values)))
            }

            fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = Map::new();
                while let Some((key, StrictValue(value))) =
                    object.next_entry::<String, StrictValue>()?
                {
                    if values.insert(key.clone(), value).is_some() {
                        return Err(de::Error::custom(format!("duplicate object key {key:?}")));
                    }
                }
                Ok(StrictValue(Value::Object(values)))
            }
        }

        deserializer.deserialize_any(StrictValueVisitor)
    }
}

trait CanonicalJson {
    fn canonicalized(self) -> Self;
}

impl CanonicalJson for Value {
    fn canonicalized(self) -> Self {
        match self {
            Value::Object(object) => {
                let mut fields = object.into_iter().collect::<Vec<_>>();
                fields.sort_unstable_by(|left, right| left.0.cmp(&right.0));

                let mut sorted = Map::new();
                for (key, value) in fields {
                    sorted.insert(key, value.canonicalized());
                }
                Value::Object(sorted)
            }
            Value::Array(values) => {
                Value::Array(values.into_iter().map(Value::canonicalized).collect())
            }
            scalar => scalar,
        }
    }
}

#[cfg(test)]
impl ConfigFile<'_> {
    fn io_error(&self, operation: &'static str, source: io::Error) -> ConfigError {
        ConfigError::Io {
            operation,
            path: self.path.to_owned(),
            source,
        }
    }

    #[cfg(unix)]
    fn set_temporary_permissions(
        &self,
        file: &File,
        existing_metadata: Option<&fs::Metadata>,
    ) -> Result<(), ConfigError> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let mode = existing_metadata.map_or(0o600, |metadata| metadata.mode() & 0o7777);
        file.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|source| self.io_error("set permissions on", source))
    }

    #[cfg(not(unix))]
    fn set_temporary_permissions(
        &self,
        _file: &File,
        _existing_metadata: Option<&fs::Metadata>,
    ) -> Result<(), ConfigError> {
        Ok(())
    }

    #[cfg(unix)]
    fn sync_directory(&self) -> io::Result<()> {
        File::open(self.path).and_then(|directory| directory.sync_all())
    }

    #[cfg(not(unix))]
    fn sync_directory(&self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::{fs, io};

    use serde_json::{Value, json};
    use spiral_robotics_core::calibration::{MotorDirection, MotorSpec};
    use tempfile::tempdir;

    use super::{ConfigError, RobotConfig, RobotConfigUpdate, SCHEMA_VERSION};

    #[test]
    fn missing_file_creates_a_default_document() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("robot.json");

        let config = RobotConfig::load(&path).expect("load missing config");

        assert_eq!(
            config.root().get("schema_version"),
            Some(&json!(SCHEMA_VERSION))
        );
        assert!(!path.exists());

        config.save_atomic(&path).expect("save new config");
        assert!(path.is_file());
    }

    #[test]
    fn merge_preserves_unknown_and_omitted_fields() {
        let config = RobotConfig::from_value(json!({
            "schema_version": 1,
            "robot_id": "existing-id",
            "port": {"path": "/dev/old", "baud": 1_000_000},
            "cameras": {"wrist": {"index": 2}},
            "extension": {"nested": [1, 2, 3]},
            "calibration": {"old": true}
        }))
        .expect("valid config");

        let merged = config
            .merged(
                RobotConfigUpdate::new(
                    json!([{"id": 1, "name": "shoulder"}]),
                    json!({"joints": [{"name": "shoulder"}]}),
                )
                .identity(None, Some("follower")),
            )
            .expect("merge config");

        assert_eq!(merged.root()["robot_id"], "existing-id");
        assert_eq!(merged.root()["robot_type"], "follower");
        assert_eq!(merged.root()["port"]["path"], "/dev/old");
        assert_eq!(merged.root()["cameras"]["wrist"]["index"], 2);
        assert_eq!(merged.root()["extension"]["nested"], json!([1, 2, 3]));
        assert_eq!(merged.root()["motors"][0]["id"], 1);
        assert_eq!(
            merged.root()["calibration"]["joints"][0]["name"],
            "shoulder"
        );
    }

    #[test]
    fn accepts_missing_schema_as_version_one() {
        let config = RobotConfig::from_value(json!({"unknown": true})).expect("legacy config");

        assert_eq!(config.root()["schema_version"], SCHEMA_VERSION);
        assert_eq!(config.root()["unknown"], true);
    }

    #[test]
    fn rejects_invalid_documents_without_changing_them() {
        let directory = tempdir().expect("temporary directory");
        let cases = [
            ("malformed", "{ definitely not json"),
            ("array", "[]"),
            ("zero", r#"{"schema_version":0}"#),
            ("fractional", r#"{"schema_version":1.5}"#),
            ("newer", r#"{"schema_version":2,"keep":"me"}"#),
        ];

        for (name, contents) in cases {
            let path = directory.path().join(format!("{name}.json"));
            fs::write(&path, contents).expect("write invalid document");

            assert!(RobotConfig::load(&path).is_err(), "{name} should fail");
            assert_eq!(
                fs::read_to_string(&path).expect("read original document"),
                contents,
                "{name} changed after failed load"
            );
        }
    }

    #[test]
    fn generic_accessors_deserialize_existing_values() {
        let config = RobotConfig::from_value(json!({
            "motors": {"shoulder": 1, "elbow": 2},
            "calibration": [10, 20, 30]
        }))
        .expect("valid config");

        let motors = config
            .deserialize_motors::<BTreeMap<String, u8>>()
            .expect("deserialize motors")
            .expect("motors present");
        let calibration = config
            .deserialize_calibration::<Vec<i64>>()
            .expect("deserialize calibration")
            .expect("calibration present");

        assert_eq!(motors["shoulder"], 1);
        assert_eq!(calibration, [10, 20, 30]);
    }

    #[test]
    fn motor_codec_preserves_unknown_raw_fields_during_domain_updates() {
        let config = RobotConfig::from_value(json!({
            "motors": {
                "joint": {
                    "id": 1,
                    "direction": "reversed",
                    "model": "sts3215",
                    "future": {"registers": [7, 8, 9]}
                }
            }
        }))
        .unwrap();
        let mut motors = config.motor_specs().unwrap().unwrap();
        motors.insert(
            "joint".to_owned(),
            MotorSpec::new(4, MotorDirection::Reversed).with_model("sts3215"),
        );

        let encoded = config.encode_motor_specs(&motors).unwrap();
        assert_eq!(encoded["joint"]["id"], 4);
        assert_eq!(encoded["joint"]["direction"], "reversed");
        assert_eq!(encoded["joint"]["model"], "sts3215");
        assert_eq!(encoded["joint"]["future"]["registers"], json!([7, 8, 9]));
    }

    #[test]
    fn motor_codec_is_strict_about_domain_fields_but_accepts_unknown_extensions() {
        for invalid in [
            json!({"motors": {"joint": {"id": -1}}}),
            json!({"motors": {"joint": {"id": 1, "direction": "sideways"}}}),
            json!({"motors": {"joint": {"id": 1, "model": null}}}),
            json!({"motors": {"joint": {"id": 1, "model": 3215}}}),
        ] {
            assert!(
                RobotConfig::from_value(invalid)
                    .unwrap()
                    .motor_specs()
                    .is_err()
            );
        }

        let extended = RobotConfig::from_value(json!({
            "motors": {"joint": {"id": 1, "future": {"anything": true}}}
        }))
        .unwrap();
        assert!(extended.motor_specs().unwrap().is_some());
    }

    #[test]
    fn raw_parser_rejects_duplicate_keys_before_values_can_overwrite_them() {
        for invalid in [
            br#"{"schema_version":1,"schema_version":1}"#.as_slice(),
            br#"{"motors":{"joint":{"id":1,"id":2}}}"#.as_slice(),
            br#"{"calibration":{"motors":{"joint":{"id":1},"joint":{"id":2}}}}"#.as_slice(),
        ] {
            assert!(RobotConfig::from_slice(invalid).is_err());
        }
    }

    #[test]
    fn supplied_port_must_be_an_object() {
        let error = RobotConfig::default()
            .merged(RobotConfigUpdate::new(json!([]), json!({})).port(Some(json!("/dev/tty0"))))
            .expect_err("string port must fail");

        assert!(matches!(error, ConfigError::PortMustBeObject));
    }

    #[test]
    fn output_is_recursive_key_sorted_and_has_one_trailing_newline() {
        let config = RobotConfig::from_value(json!({
            "z": {"z": 1, "a": 2},
            "a": true
        }))
        .expect("valid config");

        let output = config.to_pretty_json().expect("serialize config");

        assert!(output.ends_with("}\n"));
        assert!(!output.ends_with("}\n\n"));
        assert!(output.find("\"a\"").unwrap() < output.find("\"schema_version\"").unwrap());
        let nested_a = output.rfind("\"a\": 2").unwrap();
        let nested_z = output.rfind("\"z\": 1").unwrap();
        assert!(nested_a < nested_z);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_for_load_and_save() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().expect("temporary directory");
        let real = directory.path().join("real.json");
        let link = directory.path().join("link.json");
        let original = "{\"schema_version\":1,\"safe\":true}";
        fs::write(&real, original).expect("write target");
        symlink(&real, &link).expect("create symlink");

        assert!(matches!(
            RobotConfig::load(&link),
            Err(ConfigError::UnsafeTarget { .. })
        ));
        assert!(matches!(
            RobotConfig::default().save_atomic(&link),
            Err(ConfigError::UnsafeTarget { .. })
        ));
        assert_eq!(fs::read_to_string(&real).expect("read real file"), original);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_fifo_without_blocking_on_open() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("robot.pipe");
        let c_path = CString::new(path.as_os_str().as_bytes()).expect("path has no NUL");
        // SAFETY: `c_path` is a valid, NUL-terminated path and the mode is valid.
        let result = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(result, 0, "mkfifo failed: {}", io::Error::last_os_error());

        assert!(matches!(
            RobotConfig::load(&path),
            Err(ConfigError::UnsafeTarget { .. })
        ));
    }

    #[test]
    fn conditional_save_rejects_an_intervening_update() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("robot.json");
        let original = RobotConfig::from_value(json!({
            "robot_id": "arm-1",
            "cameras": {"wrist": {"index": 1}}
        }))
        .expect("original config");
        original.save_atomic(&path).expect("save original");
        let expected = RobotConfig::load(&path).expect("load expected config");

        let intervening = RobotConfig::from_value(json!({
            "robot_id": "arm-1",
            "cameras": {"wrist": {"index": 2}},
            "extension": {"keep": true}
        }))
        .expect("intervening config");
        intervening
            .save_atomic(&path)
            .expect("save intervening update");
        let stale_replacement = RobotConfig::from_value(json!({
            "robot_id": "arm-1",
            "calibration": {"format_version": 1}
        }))
        .expect("stale replacement");

        let error = stale_replacement
            .save_atomic_if_unchanged(&path, &expected)
            .expect_err("stale save must fail");

        assert!(matches!(error, ConfigError::ConcurrentModification { .. }));
        assert_eq!(
            RobotConfig::load(&path).expect("reload current config"),
            intervening
        );
    }

    #[test]
    fn conditional_save_succeeds_when_the_document_is_unchanged() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("robot.json");
        let expected = RobotConfig::load(&path).expect("load missing config");
        let replacement = RobotConfig::from_value(json!({
            "robot_id": "arm-1",
            "extension": {"keep": true}
        }))
        .expect("replacement config");

        replacement
            .save_atomic_if_unchanged(&path, &expected)
            .expect("conditional save");

        assert_eq!(
            RobotConfig::load(&path).expect("reload replacement"),
            replacement
        );
        assert!(directory.path().join("robot.json.lock").is_file());
    }

    #[test]
    fn directory_sync_failure_reports_that_the_replacement_committed() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("robot.json");
        let replacement =
            RobotConfig::from_value(json!({"robot_id": "arm-1"})).expect("replacement config");

        let error = replacement
            .save_atomic_unlocked_with_sync(&path, |_| {
                Err(io::Error::other("injected directory sync failure"))
            })
            .expect_err("directory sync must fail");

        assert!(matches!(error, ConfigError::CommittedButNotDurable { .. }));
        assert_eq!(
            RobotConfig::load(&path).expect("the replacement is visible"),
            replacement
        );
    }

    #[cfg(unix)]
    #[test]
    fn new_files_are_private_and_existing_modes_are_preserved() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let directory = tempdir().expect("temporary directory");
        let new_path = directory.path().join("new.json");
        let existing_path = directory.path().join("existing.json");
        let config = RobotConfig::default();

        config.save_atomic(&new_path).expect("save new file");
        assert_eq!(fs::metadata(&new_path).unwrap().mode() & 0o777, 0o600);

        fs::write(&existing_path, "{}").expect("write existing file");
        fs::set_permissions(&existing_path, fs::Permissions::from_mode(0o640))
            .expect("set existing permissions");
        config
            .save_atomic(&existing_path)
            .expect("replace existing file");
        assert_eq!(fs::metadata(&existing_path).unwrap().mode() & 0o777, 0o640);
    }

    #[test]
    fn saved_file_has_a_trailing_newline_and_loads_again() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("robot.json");
        let config = RobotConfig::default()
            .merged(
                RobotConfigUpdate::new(json!([]), json!({"format_version": 1}))
                    .identity(Some("arm-1"), None)
                    .port(Some(json!({"path": "/dev/ttyACM0"}))),
            )
            .expect("merge config");

        config.save_atomic(&path).expect("save config");
        let bytes = fs::read(&path).expect("read saved config");

        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(RobotConfig::load(&path).expect("reload config"), config);
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            Value::Object(config.root().clone())
        );
    }
}
