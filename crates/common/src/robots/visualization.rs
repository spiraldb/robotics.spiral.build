//! Geometry and telemetry coordinates owned by a robot configuration.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RobotVisualization {
    urdf: String,
    #[serde(default)]
    meshes: BTreeMap<String, String>,
    joints: BTreeMap<String, JointCoordinate>,
    tool_link: Option<String>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct JointCoordinate {
    input: TelemetryCoordinate,
    scale: f64,
    offset: f64,
    rest: f64,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryCoordinate {
    Degrees,
    Normalized,
}

impl RobotVisualization {
    pub fn urdf(&self) -> &str {
        &self.urdf
    }
    pub fn meshes(&self) -> &BTreeMap<String, String> {
        &self.meshes
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
}
