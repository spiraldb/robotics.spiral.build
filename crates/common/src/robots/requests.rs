//! Explicit request messages shared by HTTP clients and the backend.
use super::*;
use std::collections::BTreeMap;

/// Normalized joint targets, keyed by motor name.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobotGoal {
    normalized: BTreeMap<String, f64>,
}
impl RobotGoal {
    pub fn new(normalized: BTreeMap<String, f64>) -> Self {
        Self { normalized }
    }
    pub fn into_parts(self) -> (BTreeMap<String, f64>,) {
        (self.normalized,)
    }
}
