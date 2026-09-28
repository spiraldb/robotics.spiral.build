//! Episode recording and inspection contracts.

use serde::{Deserialize, Serialize};

/// One take in the current Studio recording session.
///
/// An episode is one take: the operator arms the recorder, performs the task
/// once, and stops. Raw observations and actions are immutable; the operator's
/// judgement may change. Historical values are never recalculated using current
/// calibration. Published dataset members use datasets::EpisodeSelection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeInfo {
    /// Stable identifier for the episode and all of its related rows.
    id: String,
    /// Explicit destination for publishing this take as an MCAP source.
    dataset_id: String,
    /// Arms sampled in this take, in the order their joints appear in a frame.
    arms: Vec<EpisodeArm>,
    frames: u64,
}

impl EpisodeInfo {
    pub fn new(id: impl Into<String>, dataset_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            dataset_id: dataset_id.into(),
            arms: Vec::new(),
            frames: 0,
        }
    }

    pub fn with_arms(mut self, arms: Vec<EpisodeArm>) -> Self {
        self.arms = arms;
        self
    }

    pub const fn with_frames(mut self, frames: u64) -> Self {
        self.frames = frames;
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn arms(&self) -> &[EpisodeArm] {
        &self.arms
    }

    pub const fn frames(&self) -> u64 {
        self.frames
    }

    pub fn dataset_id(&self) -> &str {
        &self.dataset_id
    }
}

/// Current-session take review; published datasets use topic messages and member identities.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeInspectionSnapshot {
    episode: EpisodeInfo,
    sample_times_seconds: Vec<f64>,
    arms: Vec<EpisodeInspectionArm>,
}

impl EpisodeInspectionSnapshot {
    pub fn new(
        episode: EpisodeInfo,
        sample_times_seconds: Vec<f64>,
        arms: Vec<EpisodeInspectionArm>,
    ) -> Self {
        Self {
            episode,
            sample_times_seconds,
            arms,
        }
    }

    pub const fn episode(&self) -> &EpisodeInfo {
        &self.episode
    }
    pub fn sample_times_seconds(&self) -> &[f64] {
        &self.sample_times_seconds
    }
    pub fn arms(&self) -> &[EpisodeInspectionArm] {
        &self.arms
    }
}

/// One arm's normalized positions, sharing one frame and joint ordering. The
/// value for frame `f`, joint `j` is stored at `f * joints.len() + j`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeInspectionArm {
    arm: String,
    joints: Vec<String>,
    normalized: Vec<Option<f64>>,
}

impl EpisodeInspectionArm {
    pub fn new(arm: impl Into<String>, joints: Vec<String>, normalized: Vec<Option<f64>>) -> Self {
        Self {
            arm: arm.into(),
            joints,
            normalized,
        }
    }

    pub fn arm(&self) -> &str {
        &self.arm
    }
    pub fn joints(&self) -> &[String] {
        &self.joints
    }
    pub fn normalized(&self) -> &[Option<f64>] {
        &self.normalized
    }
}

/// One arm's presence in a take.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EpisodeArm {
    id: String,
    /// Stable hardware family/role captured with the episode, such as
    /// `so101_follower`. Training uses this instead of guessing from the ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    robot_type: Option<String>,
    /// Joint names, in the order their values appear in every frame.
    joints: Vec<String>,
}

impl EpisodeArm {
    pub fn new(id: impl Into<String>, robot_type: Option<String>, joints: Vec<String>) -> Self {
        Self {
            id: id.into(),
            robot_type,
            joints,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn robot_type(&self) -> Option<&str> {
        self.robot_type.as_deref()
    }

    pub fn joints(&self) -> &[String] {
        &self.joints
    }
}

/// A take being captured right now.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecordingProgress {
    #[serde(flatten)]
    identity: RecordingIdentity,
    #[serde(flatten)]
    clock: RecordingClock,
    #[serde(flatten)]
    state: RecordingState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordingIdentity {
    /// Identifier the finished take will have.
    episode: String,
    /// Logical dataset row this take will reference when it completes.
    dataset_id: String,
}

impl RecordingIdentity {
    pub fn new(episode: impl Into<String>, dataset_id: impl Into<String>) -> Self {
        Self {
            episode: episode.into(),
            dataset_id: dataset_id.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RecordingClock {
    frames: u64,
    /// Active time on the episode timeline, excluding deliberate pauses.
    elapsed_seconds: f64,
}

impl RecordingClock {
    pub const fn new(frames: u64, elapsed_seconds: f64) -> Self {
        Self {
            frames,
            elapsed_seconds,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RecordingState {
    /// Whether samples and camera frames are currently being omitted.
    paused: bool,
    /// Set when the capture stopped itself — a full disk, or an arm that went
    /// away mid-take.
    error: Option<String>,
}

impl RecordingState {
    pub fn new(paused: bool, error: Option<String>) -> Self {
        Self { paused, error }
    }
}

impl RecordingProgress {
    pub fn new(identity: RecordingIdentity, clock: RecordingClock, state: RecordingState) -> Self {
        Self {
            identity,
            clock,
            state,
        }
    }
    pub fn episode(&self) -> &str {
        &self.identity.episode
    }
    pub fn dataset_id(&self) -> &str {
        &self.identity.dataset_id
    }
    pub const fn frames(&self) -> u64 {
        self.clock.frames
    }
    pub const fn elapsed_seconds(&self) -> f64 {
        self.clock.elapsed_seconds
    }
    pub const fn paused(&self) -> bool {
        self.state.paused
    }
    pub fn error(&self) -> Option<&str> {
        self.state.error.as_deref()
    }
}

/// A deliberate pause in an otherwise continuous recording session.
///
/// `after_seconds` is expressed on the gap-free active timeline. This keeps
/// review and dataset sampling contiguous while retaining enough provenance to
/// display or audit every omitted wall-clock interval.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecordingGap {
    after_seconds: f64,
    duration_seconds: f64,
}

impl RecordingGap {
    pub const fn new(after_seconds: f64, duration_seconds: f64) -> Self {
        Self {
            after_seconds,
            duration_seconds,
        }
    }

    pub const fn after_seconds(self) -> f64 {
        self.after_seconds
    }

    pub const fn duration_seconds(self) -> f64 {
        self.duration_seconds
    }
}

mod requests;
pub use requests::{PauseUpdate, RecordingCameraRequest, RecordingFinish, RecordingStart};
