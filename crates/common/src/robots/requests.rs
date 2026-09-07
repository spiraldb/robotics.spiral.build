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

/// Named HTTP response fields for CalibrationApplication.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalibrationApplication {
    episodes_updated: u64,
    frames_updated: u64,
    episodes_unchanged: u64,
    episodes_skipped: u64,
    warnings: Vec<String>,
}
impl From<(u64, u64, u64, u64, Vec<String>)> for CalibrationApplication {
    fn from(value: (u64, u64, u64, u64, Vec<String>)) -> Self {
        let (episodes_updated, frames_updated, episodes_unchanged, episodes_skipped, warnings) =
            value;
        Self {
            episodes_updated,
            frames_updated,
            episodes_unchanged,
            episodes_skipped,
            warnings,
        }
    }
}
impl CalibrationApplication {
    pub fn episodes_updated(&self) -> u64 {
        self.episodes_updated
    }
    pub fn frames_updated(&self) -> u64 {
        self.frames_updated
    }
    pub fn episodes_unchanged(&self) -> u64 {
        self.episodes_unchanged
    }
    pub fn episodes_skipped(&self) -> u64 {
        self.episodes_skipped
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}
