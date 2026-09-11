//! Complete robot configuration documents and compare-and-save requests.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RobotDocument(Map<String, Value>);
impl RobotDocument {
    pub fn from_object(object: Map<String, Value>) -> Self {
        Self(object)
    }
    pub fn object(&self) -> &Map<String, Value> {
        &self.0
    }
    pub fn id(&self) -> Result<&str, String> {
        let id = self
            .0
            .get("robot_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err("Robot ID must contain 1–64 letters, digits, '-' or '_'".into());
        }
        Ok(id)
    }
    pub fn validate(&self) -> Result<(), String> {
        self.id()?;
        if self
            .0
            .get("schema_version")
            .is_some_and(|v| v.as_u64() != Some(1))
        {
            return Err("Unsupported robot configuration schema version".into());
        }
        if let Some(kind) = self.0.get("robot_type") {
            let kind = kind.as_str().ok_or("Robot type must be a string")?;
            if kind.trim().is_empty() || kind.len() > 64 || kind.chars().any(char::is_control) {
                return Err(
                    "Robot type must contain 1–64 characters without control characters".into(),
                );
            }
        }
        if let Some(port) = self.0.get("port") {
            let path = port
                .get("path")
                .and_then(Value::as_str)
                .ok_or("Port needs a path")?;
            if path.trim().is_empty()
                || path.contains('\0')
                || port
                    .get("baud_rate")
                    .is_some_and(|v| !v.as_u64().is_some_and(|v| v > 0 && v <= u32::MAX as u64))
            {
                return Err(
                    "Port needs a non-empty path without NUL bytes and a positive baud rate".into(),
                );
            }
        }
        if let Some(cameras) = self.0.get("cameras") {
            for (role, camera) in cameras.as_object().ok_or("Cameras must be an object")? {
                if role.is_empty()
                    || role.len() > 64
                    || !role
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                {
                    return Err("Camera roles must contain 1–64 letters, digits, '-' or '_'".into());
                }
                let id = camera
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("Camera needs a device ID")?;
                if id.trim().is_empty() || id.contains('\0') {
                    return Err("Camera needs a non-empty device ID without NUL bytes".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobotDocumentSave {
    document: RobotDocument,
    expected: Option<RobotDocument>,
}
impl RobotDocumentSave {
    pub fn new(document: RobotDocument, expected: Option<RobotDocument>) -> Self {
        Self { document, expected }
    }
    pub fn document(&self) -> &RobotDocument {
        &self.document
    }
    pub fn changes_control_configuration(&self) -> bool {
        ["port", "motors", "calibration"].iter().any(|field| {
            self.expected
                .as_ref()
                .and_then(|old| old.object().get(*field))
                != self.document.object().get(*field)
        })
    }
    pub fn expected(&self) -> Option<&RobotDocument> {
        self.expected.as_ref()
    }
}

/// The stock SO-101 motor layout shared by authoring and native hardware defaults.
pub struct So101;
impl So101 {
    pub const fn motors() -> [(&'static str, u32); 6] {
        [
            ("shoulder_pan", 1),
            ("shoulder_lift", 2),
            ("elbow_flex", 3),
            ("wrist_flex", 4),
            ("wrist_roll", 5),
            ("gripper", 6),
        ]
    }
    pub const fn motor_model() -> &'static str {
        "sts3215"
    }
}

impl<'de> Deserialize<'de> for RobotDocument {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match super::strict_json::StrictValue::deserialize(deserializer)?.into_value() {
            Value::Object(object) => Ok(Self(object)),
            _ => Err(serde::de::Error::custom(
                "Robot configuration must be an object",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn imports_reject_duplicate_keys_at_every_depth() {
        for bytes in [
            r#"{"robot_id":"a","robot_id":"b"}"#,
            r#"{"robot_id":"a","motors":{"joint":{"id":1,"id":2}}}"#,
        ] {
            assert!(serde_json::from_str::<RobotDocument>(bytes).is_err());
        }
    }

    #[test]
    fn documents_validate_identity_ports_cameras_and_schema_before_storage() {
        for value in [
            json!({"robot_id":"../arm"}),
            json!({"robot_id":"arm", "schema_version":2}),
            json!({"robot_id":"arm", "port":{"path":"/dev/arm", "baud_rate":0}}),
            json!({"robot_id":"arm", "cameras":{"bad role":{"id":"camera"}}}),
        ] {
            assert!(
                RobotDocument::from_object(value.as_object().unwrap().clone())
                    .validate()
                    .is_err()
            );
        }
        assert!(
            RobotDocument::from_object(
                json!({"robot_id":"arm", "extension":42})
                    .as_object()
                    .unwrap()
                    .clone()
            )
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn only_camera_edits_can_leave_live_control_configuration_unchanged() {
        let baseline = RobotDocument::from_object(
            json!({"robot_id":"arm", "port":{"path":"/dev/arm"}, "calibration":{"version":1}})
                .as_object()
                .unwrap()
                .clone(),
        );
        let mut edited = baseline.object().clone();
        edited.insert("cameras".into(), json!({"wrist":{"id":"camera"}}));
        assert!(
            !RobotDocumentSave::new(
                RobotDocument::from_object(edited.clone()),
                Some(baseline.clone())
            )
            .changes_control_configuration()
        );
        for field in ["port", "motors", "calibration"] {
            let mut changed = edited.clone();
            changed.insert(field.into(), Value::Null);
            assert!(
                RobotDocumentSave::new(RobotDocument::from_object(changed), Some(baseline.clone()))
                    .changes_control_configuration()
            );
        }
    }
}
