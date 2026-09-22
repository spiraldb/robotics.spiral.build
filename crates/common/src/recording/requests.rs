//! Explicit request messages shared by HTTP clients and the backend.
use super::*;

/// Selects the dataset for a new recording.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingStart {
    dataset_id: String,
    leader: String,
    follower: String,
}
impl RecordingStart {
    pub fn new(dataset_id: String, pair: (String, String)) -> Self {
        Self {
            dataset_id,
            leader: pair.0,
            follower: pair.1,
        }
    }
    pub fn into_parts(self) -> (String, (String, String)) {
        (self.dataset_id, (self.leader, self.follower))
    }
}

/// Chooses whether stopping a recording retains its episode.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingFinish {
    keep: bool,
}
impl RecordingFinish {
    pub fn new(keep: bool) -> Self {
        Self { keep }
    }
    pub fn into_parts(self) -> (bool,) {
        (self.keep,)
    }
}

/// Sets the paused state of the current recording.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PauseUpdate {
    paused: bool,
}
impl PauseUpdate {
    pub fn new(paused: bool) -> Self {
        Self { paused }
    }
    pub fn into_parts(self) -> (bool,) {
        (self.paused,)
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingCameraRequest {
    stream: String,
}
impl RecordingCameraRequest {
    pub fn new(stream: String) -> Self {
        Self { stream }
    }
    pub fn stream(&self) -> &str {
        &self.stream
    }
}
