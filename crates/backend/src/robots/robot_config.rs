//! Safe, schema-aware storage for a robot's unified JSON configuration.
//!
//! [`RobotConfig`] keeps the complete JSON object in memory, so fields that a
//! particular command does not understand survive an update.

use std::error::Error;
use std::fmt;

use serde_json::{Map, Number, Value};
use spiral_robotics_common::robots::strict_json::StrictValue;
use spiral_robotics_hardware::{MotorDirection, MotorSpec};

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
    motors: Value,
    calibration: Value,
}
impl RobotConfigUpdate {
    pub(crate) fn new(motors: Value, calibration: Value) -> Self {
        Self {
            motors,
            calibration,
        }
    }
}

#[derive(Debug)]
pub(crate) enum ConfigError {
    Json {
        context: String,
        source: serde_json::Error,
    },
    RootMustBeObject,
    InvalidSchemaVersion(Value),
    MotorsMustBeObject,
}
impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { context, source } => write!(formatter, "invalid JSON {context}: {source}"),
            Self::RootMustBeObject => {
                formatter.write_str("robot configuration root must be a JSON object")
            }
            Self::InvalidSchemaVersion(version) => write!(
                formatter,
                "unsupported schema_version {version}; expected integer {SCHEMA_VERSION}"
            ),
            Self::MotorsMustBeObject => {
                formatter.write_str("robot configuration motors must be an object")
            }
        }
    }
}
impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json { source, .. } => Some(source),
            _ => None,
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
    /// Parse an in-memory document while rejecting duplicate object keys at any
    /// depth before a generic JSON value could silently overwrite one.
    pub(crate) fn from_slice(bytes: &[u8]) -> Result<Self, ConfigError> {
        let value =
            serde_json::from_slice::<StrictValue>(bytes).map_err(|source| ConfigError::Json {
                context: "in imported robot configuration".to_owned(),
                source,
            })?;
        Self::from_value(value.into_value())
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

    /// Replace measured calibration and motor fields, preserving the rest of the document.
    pub(crate) fn merged(&self, update: RobotConfigUpdate) -> Result<Self, ConfigError> {
        let mut root = self.root.clone();
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
mod tests {

    use serde_json::json;
    use spiral_robotics_hardware::{MotorDirection, MotorSpec};

    use super::{RobotConfig, RobotConfigUpdate, SCHEMA_VERSION};

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
            .merged(RobotConfigUpdate::new(
                json!([{"id": 1, "name": "shoulder"}]),
                json!({"joints": [{"name": "shoulder"}]}),
            ))
            .expect("merge config");

        assert_eq!(merged.root()["robot_id"], "existing-id");
        assert!(merged.root().get("robot_type").is_none());
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
    fn rejects_invalid_document_encodings() {
        for document in [
            "not json",
            "[]",
            r#"{"schema_version":0}"#,
            r#"{"schema_version":1.5}"#,
            r#"{"schema_version":2,"keep":"me"}"#,
        ] {
            assert!(RobotConfig::from_slice(document.as_bytes()).is_err());
        }
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
}
