//! Explicit request messages shared by HTTP clients and the backend.
use super::*;

/// Renames the request's workspace and sets its training runtime together.
/// The Spiral endpoints are the backend's own; no browser changes them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsUpdate {
    name: String,
    training: TrainingRuntimeSettings,
}
impl SettingsUpdate {
    pub fn new(name: impl Into<String>, training: TrainingRuntimeSettings) -> Self {
        Self {
            name: name.into(),
            training,
        }
    }
    pub fn into_parts(self) -> (String, TrainingRuntimeSettings) {
        (self.name, self.training)
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
