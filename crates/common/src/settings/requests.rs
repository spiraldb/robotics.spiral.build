//! Explicit request messages shared by HTTP clients and the backend.
use super::*;

/// Renames the request's workspace. The Spiral endpoints are the backend's
/// own; no browser changes them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsUpdate {
    name: String,
}
impl SettingsUpdate {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Points the workspace's robot connector at another policy runtime.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRuntimeUpdate {
    policy_runtime: String,
}
impl PolicyRuntimeUpdate {
    pub fn new(policy_runtime: impl Into<String>) -> Self {
        Self {
            policy_runtime: policy_runtime.into(),
        }
    }
    pub fn policy_runtime(&self) -> &str {
        &self.policy_runtime
    }
}

/// Named HTTP response fields for ConnectionTest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionTest {
    connected: bool,
    message: String,
}
impl From<(bool, String)> for ConnectionTest {
    fn from(value: (bool, String)) -> Self {
        let (connected, message) = value;
        Self { connected, message }
    }
}
impl ConnectionTest {
    pub fn new(connected: bool, message: String) -> Self {
        Self { connected, message }
    }
    pub fn connected(&self) -> bool {
        self.connected
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}
