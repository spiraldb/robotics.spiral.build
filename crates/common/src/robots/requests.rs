//! Explicit request messages shared by HTTP clients and the backend.
use super::*;
use std::collections::BTreeMap;

/// Lists the ports observed before connecting a device to identify it.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SerialPortScan {
    before: Vec<String>,
}
impl SerialPortScan {
    pub fn new(before: Vec<String>) -> Self {
        Self { before }
    }
    pub fn into_parts(self) -> (Vec<String>,) {
        (self.before,)
    }
}

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

/// Named HTTP response fields for SerialPortIdentification.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SerialPortIdentification {
    port: Option<String>,
    message: String,
}
impl From<(Option<String>, String)> for SerialPortIdentification {
    fn from(value: (Option<String>, String)) -> Self {
        let (port, message) = value;
        Self { port, message }
    }
}
impl SerialPortIdentification {
    pub fn port(&self) -> Option<&str> {
        self.port.as_deref()
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}
