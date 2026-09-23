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
    #[serde(flatten)]
    identity: EpisodeIdentity,
    #[serde(flatten)]
    timeline: EpisodeTimeline,
    #[serde(flatten)]
    assessment: EpisodeAssessment,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct EpisodeIdentity {
    /// Stable identifier for the episode and all of its related rows.
    id: String,
    /// Explicit destination for publishing this take as an MCAP source.
    dataset_id: String,
    /// Arms sampled in this take, in the order their joints appear in a frame.
    arms: Vec<EpisodeArm>,
    source: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct EpisodeTimeline {
    frames: u64,
    duration_seconds: f64,
    /// Wall-clock intervals deliberately omitted from the episode timeline.
    ///
    /// Samples and recorded video use the active timeline, so pausing does not
    /// create a motionless training-data plateau. Each entry preserves where
    /// the omission happened and how much real time passed there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recording_gaps: Vec<RecordingGap>,
    /// Rate the take was captured at, so review and export honour its timing.
    sample_hz: f64,
    /// When the take started, as milliseconds since the Unix epoch.
    ///
    /// Carried as a number rather than a formatted string so the client can
    /// render it in the operator's own locale and timezone.
    started_ms: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct EpisodeAssessment {
    /// Largest gap between a measured and a commanded joint angle during the
    /// take, in degrees — the same quantity the inspector shows live as
    /// `Follow Δ`, kept for the whole take instead of for one instant.
    max_follow_error_degrees: Option<f64>,
    /// True once the operator has judged this take unusable.
    ///
    /// A discarded take remains in SpiralDB and is excluded from training,
    /// because the judgement is the operator's and deleting on their behalf is not.
    discarded: bool,
    /// Why this committed take cannot currently be reviewed or trained.
    problem: Option<String>,
}

impl EpisodeInfo {
    pub fn new(id: impl Into<String>, dataset_id: impl Into<String>) -> Self {
        Self {
            identity: EpisodeIdentity {
                id: id.into(),
                dataset_id: dataset_id.into(),
                arms: Vec::new(),
                source: String::new(),
            },
            timeline: EpisodeTimeline {
                frames: 0,
                duration_seconds: 0.0,
                recording_gaps: Vec::new(),
                sample_hz: 0.0,
                started_ms: 0,
            },
            assessment: EpisodeAssessment {
                max_follow_error_degrees: None,
                discarded: false,
                problem: None,
            },
        }
    }

    pub fn with_arms(mut self, arms: Vec<EpisodeArm>) -> Self {
        self.identity.arms = arms;
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.identity.source = source.into();
        self
    }

    pub const fn with_timeline(mut self, frames: u64, duration_seconds: f64) -> Self {
        self.timeline.frames = frames;
        self.timeline.duration_seconds = duration_seconds;
        self
    }

    pub const fn with_sample_hz(mut self, sample_hz: f64) -> Self {
        self.timeline.sample_hz = sample_hz;
        self
    }

    pub fn with_recording_gaps(mut self, recording_gaps: Vec<RecordingGap>) -> Self {
        self.timeline.recording_gaps = recording_gaps;
        self
    }

    pub const fn with_started_ms(mut self, started_ms: i64) -> Self {
        self.timeline.started_ms = started_ms;
        self
    }

    pub const fn with_assessment(
        mut self,
        discarded: bool,
        max_follow_error_degrees: Option<f64>,
    ) -> Self {
        self.assessment.discarded = discarded;
        self.assessment.max_follow_error_degrees = max_follow_error_degrees;
        self
    }

    pub fn id(&self) -> &str {
        &self.identity.id
    }

    pub fn arms(&self) -> &[EpisodeArm] {
        &self.identity.arms
    }

    pub const fn frames(&self) -> u64 {
        self.timeline.frames
    }

    pub fn dataset_id(&self) -> &str {
        &self.identity.dataset_id
    }
}

/// Current-session take review; published datasets use topic messages and member identities.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeInspectionSnapshot {
    episode: EpisodeInfo,
    sample_times_seconds: Vec<f64>,
    follow_error_degrees: Vec<Option<f64>>,
    arms: Vec<EpisodeInspectionArm>,
}

impl EpisodeInspectionSnapshot {
    pub fn new(
        episode: EpisodeInfo,
        sample_times_seconds: Vec<f64>,
        follow_error_degrees: Vec<Option<f64>>,
    ) -> Self {
        Self {
            episode,
            sample_times_seconds,
            follow_error_degrees,
            arms: Vec::new(),
        }
    }

    pub fn with_arms(mut self, arms: Vec<EpisodeInspectionArm>) -> Self {
        self.arms = arms;
        self
    }

    pub const fn episode(&self) -> &EpisodeInfo {
        &self.episode
    }
    pub fn sample_times_seconds(&self) -> &[f64] {
        &self.sample_times_seconds
    }
    pub fn follow_error_degrees(&self) -> &[Option<f64>] {
        &self.follow_error_degrees
    }
    pub fn arms(&self) -> &[EpisodeInspectionArm] {
        &self.arms
    }
}

/// All recorded scalar channels for one arm, sharing one frame and joint
/// ordering. A matrix value for frame `f`, joint `j` is stored at
/// `f * joints.len() + j`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeInspectionArm {
    arm: String,
    joints: Vec<String>,
    #[serde(flatten)]
    samples: EpisodeArmScalarSamples,
    velocity: EpisodeOptionalFrameMatrix,
    effort: EpisodeOptionalFrameMatrix,
    action: EpisodeOptionalFrameMatrix,
}

/// Dense scalar matrices that are present for every recorded arm frame.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct EpisodeArmScalarSamples {
    normalized: Vec<Option<f64>>,
    raw: Vec<i64>,
    #[serde(default)]
    degrees: Vec<Option<f64>>,
}

impl EpisodeArmScalarSamples {
    pub const fn new(normalized: Vec<Option<f64>>, raw: Vec<i64>) -> Self {
        Self {
            normalized,
            raw,
            degrees: Vec::new(),
        }
    }
    pub fn with_degrees(mut self, degrees: Vec<Option<f64>>) -> Self {
        self.degrees = degrees;
        self
    }
}

impl EpisodeInspectionArm {
    pub fn new(
        arm: impl Into<String>,
        joints: Vec<String>,
        samples: EpisodeArmScalarSamples,
    ) -> Self {
        Self {
            arm: arm.into(),
            joints,
            samples,
            velocity: EpisodeOptionalFrameMatrix::default(),
            effort: EpisodeOptionalFrameMatrix::default(),
            action: EpisodeOptionalFrameMatrix::default(),
        }
    }

    pub fn with_velocity(mut self, velocity: EpisodeOptionalFrameMatrix) -> Self {
        self.velocity = velocity;
        self
    }

    pub fn with_effort(mut self, effort: EpisodeOptionalFrameMatrix) -> Self {
        self.effort = effort;
        self
    }

    pub fn with_action(mut self, action: EpisodeOptionalFrameMatrix) -> Self {
        self.action = action;
        self
    }

    pub fn arm(&self) -> &str {
        &self.arm
    }
    pub fn joints(&self) -> &[String] {
        &self.joints
    }
    pub fn normalized(&self) -> &[Option<f64>] {
        &self.samples.normalized
    }
    pub fn raw(&self) -> &[i64] {
        &self.samples.raw
    }
    pub fn degrees(&self) -> &[Option<f64>] {
        &self.samples.degrees
    }
    pub const fn velocity(&self) -> &EpisodeOptionalFrameMatrix {
        &self.velocity
    }
    pub const fn effort(&self) -> &EpisodeOptionalFrameMatrix {
        &self.effort
    }
    pub const fn action(&self) -> &EpisodeOptionalFrameMatrix {
        &self.action
    }
}

/// A channel that may be absent for an entire arm at an individual frame.
/// `present` has one entry per frame; `values` remains frame-major and uses
/// nulls for unavailable joints without repeating arm or joint names.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct EpisodeOptionalFrameMatrix {
    present: Vec<bool>,
    values: Vec<Option<f64>>,
}

impl EpisodeOptionalFrameMatrix {
    pub const fn new(present: Vec<bool>, values: Vec<Option<f64>>) -> Self {
        Self { present, values }
    }

    pub fn present(&self) -> &[bool] {
        &self.present
    }
    pub fn values(&self) -> &[Option<f64>] {
        &self.values
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
    #[serde(default = "RecordingIdentity::default_dataset_id")]
    dataset_id: String,
    arms: Vec<String>,
}

impl RecordingIdentity {
    fn default_dataset_id() -> String {
        "default".to_owned()
    }

    pub fn new(
        episode: impl Into<String>,
        dataset_id: impl Into<String>,
        arms: Vec<String>,
    ) -> Self {
        Self {
            episode: episode.into(),
            dataset_id: dataset_id.into(),
            arms,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RecordingClock {
    frames: u64,
    elapsed_seconds: f64,
    /// Real time since Record was pressed, including deliberate pauses.
    #[serde(default)]
    wall_elapsed_seconds: f64,
    /// Real time omitted from the active episode timeline so far.
    #[serde(default)]
    paused_seconds: f64,
}

impl RecordingClock {
    pub const fn new(frames: u64, elapsed_seconds: f64, wall_elapsed_seconds: f64) -> Self {
        Self {
            frames,
            elapsed_seconds,
            wall_elapsed_seconds,
            paused_seconds: 0.0,
        }
    }
    pub const fn with_paused_seconds(mut self, paused_seconds: f64) -> Self {
        self.paused_seconds = paused_seconds;
        self
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RecordingState {
    /// Whether samples and camera frames are currently being omitted.
    #[serde(default)]
    paused: bool,
    /// Largest `Follow Δ` seen so far in this take, in degrees.
    max_follow_error_degrees: Option<f64>,
    /// Set when the capture stopped itself — a full disk, or an arm that went
    /// away mid-take.
    error: Option<String>,
}

impl RecordingState {
    pub fn new(paused: bool, max_follow_error_degrees: Option<f64>, error: Option<String>) -> Self {
        Self {
            paused,
            max_follow_error_degrees,
            error,
        }
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
