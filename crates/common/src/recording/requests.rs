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

/// Sets the paused state of the current recording or replay.
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

/// Selects a robot and source arm for a physical seek within an episode.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeSeek {
    robot: String,
    source_arm: Option<String>,
    seconds: f64,
}
impl EpisodeSeek {
    pub fn new(robot: String, seconds: f64) -> Self {
        Self {
            robot,
            source_arm: None,
            seconds,
        }
    }
    pub fn with_source_arm(mut self, source_arm: Option<String>) -> Self {
        self.source_arm = source_arm;
        self
    }
    pub fn into_parts(self) -> (String, Option<String>, f64) {
        (self.robot, self.source_arm, self.seconds)
    }
}

/// Selects the episode, target robot, and start time for replay.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayStart {
    episode: String,
    robot: String,
    source_arm: Option<String>,
    start_seconds: f64,
}
impl ReplayStart {
    pub fn new(episode: String, robot: String, start_seconds: f64) -> Self {
        Self {
            episode,
            robot,
            source_arm: None,
            start_seconds,
        }
    }
    pub fn with_source_arm(mut self, source_arm: Option<String>) -> Self {
        self.source_arm = source_arm;
        self
    }
    pub fn into_parts(self) -> (String, String, Option<String>, f64) {
        (
            self.episode,
            self.robot,
            self.source_arm,
            self.start_seconds,
        )
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
