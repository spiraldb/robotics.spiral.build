//! Explicit request messages shared by HTTP clients and the backend.
use super::*;

/// Selects the dataset for a new recording, what drives the arms it records,
/// and whether their cameras are recorded too.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingStart {
    dataset_id: String,
    driver: RecordingDriver,
    video: bool,
}
impl RecordingStart {
    pub fn new(dataset_id: String, driver: RecordingDriver, video: bool) -> Self {
        Self {
            dataset_id,
            driver,
            video,
        }
    }
    pub fn into_parts(self) -> (String, RecordingDriver, bool) {
        (self.dataset_id, self.driver, self.video)
    }
}

/// What moves the follower while a take records. A take starts its driver
/// and ends with it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordingDriver {
    /// The operator moves the leader and the follower mirrors it; both arms
    /// are recorded.
    Teleoperate { leader: String, follower: String },
    /// A checkpoint's policy drives the follower for its duration; the take
    /// records the follower from the policy's first action to its last.
    Policy(crate::rollout::RolloutConfig),
}

/// Chooses whether stopping a recording retains its episode.
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
