//! Explicit request messages shared by HTTP clients and the backend.
use super::*;

/// Updates project connection and compute/video preferences together.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsUpdate {
    connection: SpiralConnectionUpdate,
    training: TrainingRuntimeSettings,
    video: VideoSettings,
}
impl SettingsUpdate {
    pub fn new(
        connection: SpiralConnectionUpdate,
        training: TrainingRuntimeSettings,
        video: VideoSettings,
    ) -> Self {
        Self {
            connection,
            training,
            video,
        }
    }
    pub fn into_parts(
        self,
    ) -> (
        SpiralConnectionUpdate,
        TrainingRuntimeSettings,
        VideoSettings,
    ) {
        (self.connection, self.training, self.video)
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
