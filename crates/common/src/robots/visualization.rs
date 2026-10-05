//! A robot's 3D model, and how its telemetry poses that model.
//!
//! The model — a URDF and every mesh it names — is large and never changes, so
//! a configuration names it by content and the robot connector stores and
//! serves it once as [`RobotModelFiles`]. What a configuration keeps is small:
//! which model, how each joint's telemetry maps onto it, and its tool link.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

use super::JointTelemetry;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RobotVisualization {
    model: RobotModelId,
    joints: BTreeMap<String, JointCoordinate>,
    tool_link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct JointCoordinate {
    input: TelemetryCoordinate,
    scale: f64,
    offset: f64,
    rest: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryCoordinate {
    Degrees,
    Normalized,
}

impl RobotVisualization {
    /// The stored model this robot is drawn with.
    pub const fn model(&self) -> &RobotModelId {
        &self.model
    }
    pub fn tool_link(&self) -> Option<&str> {
        self.tool_link.as_deref()
    }
    pub fn positions(
        &self,
        mut read: impl FnMut(&str, TelemetryCoordinate) -> Option<f64>,
    ) -> BTreeMap<String, f32> {
        self.joints
            .iter()
            .map(|(name, coordinate)| {
                let position = read(name, coordinate.input)
                    .filter(|value| value.is_finite())
                    .map_or(coordinate.rest, |value| {
                        value * coordinate.scale + coordinate.offset
                    });
                (name.clone(), position as f32)
            })
            .collect()
    }

    /// Where every joint stands in a live telemetry sample.
    pub fn measured_positions(&self, joints: &[JointTelemetry]) -> BTreeMap<String, f32> {
        self.positions(|name, input| {
            let joint = joints.iter().find(|joint| joint.name() == name)?;
            match input {
                TelemetryCoordinate::Degrees => joint.degrees(),
                TelemetryCoordinate::Normalized => joint.normalized(),
            }
        })
    }
}

/// A robot description and every mesh it names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "RobotModelFields")]
pub struct RobotModelFiles(RobotModelFields);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RobotModelFields {
    urdf: String,
    /// Base64 mesh bytes, keyed by the exact filename the URDF uses.
    #[serde(default)]
    meshes: BTreeMap<String, String>,
}

impl TryFrom<RobotModelFields> for RobotModelFiles {
    type Error = String;

    fn try_from(fields: RobotModelFields) -> Result<Self, String> {
        if fields.urdf.trim().is_empty() {
            return Err("A robot model needs a URDF".into());
        }
        Ok(Self(fields))
    }
}

impl RobotModelFiles {
    pub fn new(urdf: String, meshes: BTreeMap<String, String>) -> Result<Self, String> {
        Self::try_from(RobotModelFields { urdf, meshes })
    }
    pub fn urdf(&self) -> &str {
        &self.0.urdf
    }
    pub fn meshes(&self) -> &BTreeMap<String, String> {
        &self.0.meshes
    }

    /// The content address this model is stored and served under: the SHA-256
    /// of its JSON, whose map keys are always in order.
    pub fn id(&self) -> RobotModelId {
        let bytes = serde_json::to_vec(self).expect("a robot model always serializes");
        let digest = Sha256::digest(bytes);
        RobotModelId(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }
}

/// The SHA-256 a stored robot model is addressed by, as lowercase hex.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RobotModelId(String);

impl TryFrom<String> for RobotModelId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, String> {
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value))
        } else {
            Err(format!("{value:?} is not a robot model's SHA-256"))
        }
    }
}

impl From<RobotModelId> for String {
    fn from(id: RobotModelId) -> Self {
        id.0
    }
}

impl fmt::Display for RobotModelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_model_is_addressed_by_its_content() {
        let model = |mesh: &str| {
            RobotModelFiles::new(
                "<robot name=\"arm\"/>".into(),
                BTreeMap::from([("base.stl".into(), mesh.into())]),
            )
            .unwrap()
        };
        let id = model("AAAA").id();
        assert_eq!(id, model("AAAA").id());
        assert_ne!(id, model("BBBB").id());
        let decoded: RobotModelId = serde_json::from_value(json!(id.to_string())).unwrap();
        assert_eq!(decoded, id);
        for invalid in ["../robots", "ABC", &"g".repeat(64)] {
            assert!(serde_json::from_value::<RobotModelId>(json!(invalid)).is_err());
        }
        assert!(RobotModelFiles::new(" ".into(), BTreeMap::new()).is_err());
    }
}
