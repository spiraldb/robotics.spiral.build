//! Episode recording, inspection, and replay contracts.

use serde::{Deserialize, Serialize};
use spiral_robotics_core::operation::EpisodeOutcome;

use crate::datasets::default_dataset_id;

/// Replay a trajectory fetched directly from SpiralDB.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartSpiralReplayRequest {
    pub episode: String,
    pub robot: String,
    pub stream: String,
}

/// One recorded episode, as summarised from SpiralDB.
///
/// An episode is one take: the operator arms the recorder, performs the task
/// once, and stops. Raw observations and actions are immutable; the operator's
/// judgement may change, and normalized measured state may be re-derived from
/// raw counts after the originating arm is recalibrated.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeSummary {
    /// Stable identifier for the episode and all of its related rows.
    pub id: String,
    /// Dataset row this episode belongs to. Streams and samples reference the
    /// episode; only the episode row carries dataset membership.
    #[serde(default = "default_dataset_id")]
    pub dataset_id: String,
    /// Arms sampled in this take, in the order their joints appear in a frame.
    pub arms: Vec<EpisodeArm>,
    pub frames: u64,
    pub duration_seconds: f64,
    /// Wall-clock intervals deliberately omitted from the episode timeline.
    ///
    /// Samples and recorded video use the active timeline, so pausing does not
    /// create a motionless training-data plateau. Each entry preserves where
    /// the omission happened and how much real time passed there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recording_gaps: Vec<RecordingGap>,
    /// Rate the take was captured at, so a replay can honour its timing.
    pub sample_hz: f64,
    /// When the take started, as milliseconds since the Unix epoch.
    ///
    /// Carried as a number rather than a formatted string so the client can
    /// render it in the operator's own locale and timezone.
    pub started_ms: i64,
    /// The mode that produced the take, in the studio's own words.
    pub source: String,
    /// Largest gap between a measured and a commanded joint angle during the
    /// take, in degrees — the same quantity the inspector shows live as
    /// `Follow Δ`, kept for the whole take instead of for one instant.
    pub max_follow_error_degrees: Option<f64>,
    /// True once the operator has judged this take unusable.
    ///
    /// A discarded take remains in SpiralDB and is excluded from training,
    /// because the judgement is the operator's and deleting on their behalf is not.
    pub discarded: bool,
    /// Why this committed take cannot currently be replayed or trained.
    pub problem: Option<String>,
}

/// One logical dataset known to Studio, including datasets whose native
/// imported relations do not project into Studio's recorded-episode schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetSummary {
    pub id: String,
    pub name: String,
    pub created_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl EpisodeSummary {
    /// True when the take is complete enough to replay or export.
    pub fn usable(&self) -> bool {
        EpisodeOutcome::new(self.frames, self.problem.is_some()).is_usable()
    }

    /// The joints of the arm named, when this take sampled it.
    pub fn arm(&self, id: &str) -> Option<&EpisodeArm> {
        self.arms.iter().find(|arm| arm.id == id)
    }
}

/// One immutable, browser-cacheable projection of a recorded episode.
///
/// The frame clock and arm tracks are columnar: arm and joint identities occur
/// once, while every visual seek indexes the flat frame-major matrices in
/// memory. Video bytes remain separate range-addressable resources.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeInspectionSnapshot {
    pub episode: EpisodeSummary,
    pub sample_times_seconds: Vec<f64>,
    pub follow_error_degrees: Vec<Option<f64>>,
    pub arms: Vec<EpisodeInspectionArm>,
    pub videos: Vec<EpisodeVideoAssetMetadata>,
}

/// All recorded scalar channels for one arm, sharing one frame and joint
/// ordering. A matrix value for frame `f`, joint `j` is stored at
/// `f * joints.len() + j`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeInspectionArm {
    pub arm: String,
    pub joints: Vec<String>,
    pub normalized: Vec<Option<f64>>,
    pub raw: Vec<i64>,
    pub velocity: EpisodeOptionalFrameMatrix,
    pub effort: EpisodeOptionalFrameMatrix,
    pub action: EpisodeOptionalFrameMatrix,
}

/// A channel that may be absent for an entire arm at an individual frame.
/// `present` has one entry per frame; `values` remains frame-major and uses
/// nulls for unavailable joints without repeating arm or joint names.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct EpisodeOptionalFrameMatrix {
    pub present: Vec<bool>,
    pub values: Vec<Option<f64>>,
}

/// Filesystem-free metadata needed to align a browser-native video with the
/// episode clock. The MP4 payload deliberately does not travel in the snapshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeVideoAssetMetadata {
    pub stream_id: String,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u64,
    pub duration_seconds: f64,
    pub segment_index: u32,
    pub timeline_offset_seconds: Option<f64>,
}

/// Browser-owned episode catalog loaded on startup or explicit refresh.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct EpisodeCatalogResponse {
    /// Every logical dataset, oldest first.
    #[serde(default)]
    pub datasets: Vec<DatasetSummary>,
    /// Every stored episode, oldest first.
    pub episodes: Vec<EpisodeSummary>,
    /// Catalog rows that could not be decoded.
    pub issues: Vec<String>,
}

/// One arm's presence in a take.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EpisodeArm {
    pub id: String,
    /// Stable hardware family/role captured with the episode, such as
    /// `so101_follower`. Training uses this instead of guessing from the ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robot_type: Option<String>,
    /// Joint names, in the order their values appear in every frame.
    pub joints: Vec<String>,
}

impl spiral_robotics_core::episode::ArmLayout for EpisodeArm {
    fn id(&self) -> &str {
        &self.id
    }

    fn joint_count(&self) -> usize {
        self.joints.len()
    }
}

/// A take being captured right now.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecordingProgress {
    /// Identifier the finished take will have.
    pub episode: String,
    /// Logical dataset row this take will reference when it completes.
    #[serde(default = "default_dataset_id")]
    pub dataset_id: String,
    pub frames: u64,
    pub elapsed_seconds: f64,
    /// Real time since Record was pressed, including deliberate pauses.
    #[serde(default)]
    pub wall_elapsed_seconds: f64,
    /// Real time omitted from the active episode timeline so far.
    #[serde(default)]
    pub paused_seconds: f64,
    /// Whether samples and camera frames are currently being omitted.
    #[serde(default)]
    pub paused: bool,
    /// Arms being sampled.
    pub arms: Vec<String>,
    /// Largest `Follow Δ` seen so far in this take, in degrees.
    pub max_follow_error_degrees: Option<f64>,
    /// Set when the capture stopped itself — a full disk, or an arm that went
    /// away mid-take.
    pub error: Option<String>,
}

/// A deliberate pause in an otherwise continuous recording session.
///
/// `after_seconds` is expressed on the gap-free active timeline. This keeps
/// replay and dataset sampling contiguous while retaining enough provenance to
/// display or audit every omitted wall-clock interval.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecordingGap {
    pub after_seconds: f64,
    pub duration_seconds: f64,
}

/// Why a replay trajectory is loaded.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPurpose {
    /// User-facing episode transport controlled by Play/Pause.
    #[default]
    Timeline,
    /// One committed Timeline scrub moving the robot to a single pose.
    PhysicalSeek,
}

/// Whether a replay is positioning the arm or advancing through the episode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPhase {
    Seeking,
    /// Default preserves the behavior of progress sent by an older backend.
    #[default]
    Playing,
}

/// An episode being played back right now.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReplayProgress {
    pub episode: String,
    /// The arm being driven.
    pub robot: String,
    /// Frames played so far.
    pub frame: u64,
    pub frames: u64,
    pub elapsed_seconds: f64,
    pub duration_seconds: f64,
    /// Whether this is Timeline transport or a one-pose physical seek.
    #[serde(default)]
    pub purpose: ReplayPurpose,
    /// Positioning never advances the episode clock or recorded media.
    #[serde(default)]
    pub phase: ReplayPhase,
    /// Whether playback is holding its current frame without discarding the
    /// loaded trajectory. Resuming continues from this exact position.
    #[serde(default)]
    pub paused: bool,
}

/// Begin capturing a take.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartRecordingRequest {
    /// Dataset row that will own the completed episode.
    #[serde(default = "default_dataset_id")]
    pub dataset_id: String,
    /// Arms to sample. Empty means every connected arm, which is the useful
    /// default: a take is a record of the whole scene, not of one arm.
    #[serde(default)]
    pub arms: Vec<String>,
    /// A short operator note stored with the take.
    #[serde(default)]
    pub note: Option<String>,
}

impl Default for StartRecordingRequest {
    fn default() -> Self {
        Self {
            dataset_id: default_dataset_id(),
            arms: Vec::new(),
            note: None,
        }
    }
}

/// Finish the take in progress.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StopRecordingRequest {
    /// False marks the finished take discarded in the same gesture that ends
    /// it, for the common case of knowing the take was bad while it ran.
    pub keep: bool,
}

/// Pause or resume the take in progress without changing its episode ID.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetRecordingPausedRequest {
    pub paused: bool,
}

/// Pause or resume a replay without unloading its trajectory.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetReplayPausedRequest {
    pub paused: bool,
}

/// Change the operator's judgement of a stored take.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct JudgeEpisodeRequest {
    pub discarded: bool,
}

/// Play a stored episode back onto one arm.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct StartReplayRequest {
    pub episode: String,
    /// The arm to drive. It must be connected and calibrated.
    pub robot: String,
    /// Which recorded arm's trajectory to play, when the take holds more than
    /// one. Defaults to the arm being driven, then to the take's only arm.
    #[serde(default)]
    pub source_arm: Option<String>,
    /// Episode-clock position where replay should begin. Older clients omit
    /// this and retain the original frame-zero behavior.
    #[serde(default)]
    pub start_seconds: f64,
}
