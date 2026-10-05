//! Explicit request messages shared by HTTP clients and the backend.
use super::*;

/// Updates the browser's project and its compute/video preferences together.
/// The Spiral endpoints are the backend's own; no browser changes them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsUpdate {
    project_id: String,
    training: TrainingRuntimeSettings,
    video: VideoSettings,
}
impl SettingsUpdate {
    pub fn new(
        project_id: impl Into<String>,
        training: TrainingRuntimeSettings,
        video: VideoSettings,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            training,
            video,
        }
    }
    pub fn into_parts(self) -> (String, TrainingRuntimeSettings, VideoSettings) {
        (self.project_id, self.training, self.video)
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
