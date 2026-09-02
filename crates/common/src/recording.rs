//! Episode recording, inspection, and replay contracts.

use serde::{Deserialize, Serialize};

/// One recorded episode, as summarised from SpiralDB.
///
/// An episode is one take: the operator arms the recorder, performs the task
/// once, and stops. Raw observations and actions are immutable; the operator's
/// judgement may change, and normalized measured state may be re-derived from
/// raw counts after the originating arm is recalibrated.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeInfo {
    #[serde(flatten)]
    identity: EpisodeIdentity,
    #[serde(flatten)]
    timeline: EpisodeTimeline,
    #[serde(flatten)]
    assessment: EpisodeAssessment,
}

/// Episode-level fields shown in the dataset list. Arm and stream layout is
/// intentionally absent and is loaded only through episode inspection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeListItem {
    #[serde(flatten)]
    identity: EpisodeListIdentity,
    #[serde(flatten)]
    timeline: EpisodeTimeline,
    #[serde(flatten)]
    assessment: EpisodeAssessment,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct EpisodeListIdentity {
    id: String,
    dataset_id: String,
    source: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct EpisodeIdentity {
    /// Stable identifier for the episode and all of its related rows.
    id: String,
    /// Dataset row this episode belongs to. Streams and samples reference the
    /// episode; only the episode row carries dataset membership.
    #[serde(default = "EpisodeIdentity::default_dataset_id")]
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
    /// Rate the take was captured at, so a replay can honour its timing.
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
    /// Why this committed take cannot currently be replayed or trained.
    problem: Option<String>,
}

/// One logical dataset known to Studio, including datasets whose native
/// imported relations do not project into Studio's recorded-episode schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetSummary {
    id: String,
    name: String,
    created_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

impl DatasetSummary {
    pub fn new(id: impl Into<String>, name: impl Into<String>, created_ms: i64) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            created_ms,
            description: None,
        }
    }
    pub fn with_description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }
    pub fn id(&self) -> &str {
        &self.id
    }
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

    pub fn with_problem(mut self, problem: Option<String>) -> Self {
        self.assessment.problem = problem;
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

    /// True when the take is complete enough to replay or export.
    pub fn usable(&self) -> bool {
        self.timeline.frames > 0 && self.assessment.problem.is_none()
    }

    /// The joints of the arm named, when this take sampled it.
    pub fn arm(&self, id: &str) -> Option<&EpisodeArm> {
        self.identity.arms.iter().find(|arm| arm.id == id)
    }

    pub fn dataset_id(&self) -> &str {
        &self.identity.dataset_id
    }
    pub const fn duration_seconds(&self) -> f64 {
        self.timeline.duration_seconds
    }
    pub const fn sample_hz(&self) -> f64 {
        self.timeline.sample_hz
    }
    pub const fn started_ms(&self) -> i64 {
        self.timeline.started_ms
    }
    pub fn source(&self) -> &str {
        &self.identity.source
    }
    pub const fn discarded(&self) -> bool {
        self.assessment.discarded
    }
    pub fn problem(&self) -> Option<&str> {
        self.assessment.problem.as_deref()
    }
}

impl From<&EpisodeInfo> for EpisodeListItem {
    fn from(episode: &EpisodeInfo) -> Self {
        Self {
            identity: EpisodeListIdentity {
                id: episode.identity.id.clone(),
                dataset_id: episode.identity.dataset_id.clone(),
                source: episode.identity.source.clone(),
            },
            timeline: episode.timeline.clone(),
            assessment: episode.assessment.clone(),
        }
    }
}

impl EpisodeListItem {
    pub fn id(&self) -> &str {
        &self.identity.id
    }
    pub fn dataset_id(&self) -> &str {
        &self.identity.dataset_id
    }
    pub fn source(&self) -> &str {
        &self.identity.source
    }
    pub const fn frames(&self) -> u64 {
        self.timeline.frames
    }
    pub const fn duration_seconds(&self) -> f64 {
        self.timeline.duration_seconds
    }
    pub const fn sample_hz(&self) -> f64 {
        self.timeline.sample_hz
    }
    pub const fn started_ms(&self) -> i64 {
        self.timeline.started_ms
    }
    pub fn usable(&self) -> bool {
        self.timeline.frames > 0 && self.assessment.problem.is_none()
    }
    pub const fn discarded(&self) -> bool {
        self.assessment.discarded
    }
}

impl EpisodeIdentity {
    fn default_dataset_id() -> String {
        "default".to_owned()
    }
}

/// One immutable, browser-cacheable projection of a recorded episode.
///
/// The frame clock and arm tracks are columnar: arm and joint identities occur
/// once, while every visual seek indexes the flat frame-major matrices in
/// memory. Video bytes remain separate range-addressable resources.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeInspectionSnapshot {
    episode: EpisodeInfo,
    sample_times_seconds: Vec<f64>,
    follow_error_degrees: Vec<Option<f64>>,
    arms: Vec<EpisodeInspectionArm>,
    videos: Vec<EpisodeVideoAssetMetadata>,
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
            videos: Vec::new(),
        }
    }

    pub fn with_arms(mut self, arms: Vec<EpisodeInspectionArm>) -> Self {
        self.arms = arms;
        self
    }

    pub fn with_videos(mut self, videos: Vec<EpisodeVideoAssetMetadata>) -> Self {
        self.videos = videos;
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
    pub fn videos(&self) -> &[EpisodeVideoAssetMetadata] {
        &self.videos
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

/// Filesystem-free metadata needed to align a browser-native video with the
/// episode clock. The MP4 payload deliberately does not travel in the snapshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeVideoAssetMetadata {
    #[serde(flatten)]
    identity: EpisodeVideoIdentity,
    #[serde(flatten)]
    format: EpisodeVideoFormat,
    #[serde(flatten)]
    timeline: EpisodeVideoTimeline,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeVideoIdentity {
    stream_id: String,
    camera_id: String,
    segment_index: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeVideoFormat {
    width: u32,
    height: u32,
    fps: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeVideoTimeline {
    frames: u64,
    duration_seconds: f64,
    timeline_offset_seconds: Option<f64>,
}

impl EpisodeVideoIdentity {
    pub fn new(
        stream_id: impl Into<String>,
        camera_id: impl Into<String>,
        segment_index: u32,
    ) -> Self {
        Self {
            stream_id: stream_id.into(),
            camera_id: camera_id.into(),
            segment_index,
        }
    }
}
impl EpisodeVideoFormat {
    pub const fn new(width: u32, height: u32, fps: u32) -> Self {
        Self { width, height, fps }
    }
}
impl EpisodeVideoTimeline {
    pub const fn new(
        frames: u64,
        duration_seconds: f64,
        timeline_offset_seconds: Option<f64>,
    ) -> Self {
        Self {
            frames,
            duration_seconds,
            timeline_offset_seconds,
        }
    }
}
impl EpisodeVideoAssetMetadata {
    pub const fn new(
        identity: EpisodeVideoIdentity,
        format: EpisodeVideoFormat,
        timeline: EpisodeVideoTimeline,
    ) -> Self {
        Self {
            identity,
            format,
            timeline,
        }
    }
    pub fn stream_id(&self) -> &str {
        &self.identity.stream_id
    }
    pub const fn duration_seconds(&self) -> f64 {
        self.timeline.duration_seconds
    }
    pub const fn segment_index(&self) -> u32 {
        self.identity.segment_index
    }
    pub const fn timeline_offset_seconds(&self) -> Option<f64> {
        self.timeline.timeline_offset_seconds
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_stream_id(mut self, stream_id: impl Into<String>) -> Self {
        self.identity.stream_id = stream_id.into();
        self
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episode_list_items_do_not_serialize_stream_layout() {
        let episode = EpisodeInfo::new("episode_000001", "default")
            .with_arms(vec![EpisodeArm::new(
                "follower",
                Some("so101_follower".to_owned()),
                vec!["shoulder_pan".to_owned()],
            )])
            .with_source("teleoperation")
            .with_timeline(120, 2.0)
            .with_sample_hz(60.0);

        let serialized = serde_json::to_value(EpisodeListItem::from(&episode)).unwrap();

        assert_eq!(serialized["id"], "episode_000001");
        assert_eq!(serialized["dataset_id"], "default");
        assert!(serialized.get("arms").is_none());
        assert!(serialized.get("robot_type").is_none());
        assert!(serialized.get("joints").is_none());
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
    pub fn arms(&self) -> &[String] {
        &self.identity.arms
    }
    pub const fn frames(&self) -> u64 {
        self.clock.frames
    }
    pub const fn elapsed_seconds(&self) -> f64 {
        self.clock.elapsed_seconds
    }
    pub const fn wall_elapsed_seconds(&self) -> f64 {
        self.clock.wall_elapsed_seconds
    }
    pub const fn paused_seconds(&self) -> f64 {
        self.clock.paused_seconds
    }
    pub const fn paused(&self) -> bool {
        self.state.paused
    }
    pub const fn max_follow_error_degrees(&self) -> Option<f64> {
        self.state.max_follow_error_degrees
    }
    pub fn error(&self) -> Option<&str> {
        self.state.error.as_deref()
    }
}

/// A deliberate pause in an otherwise continuous recording session.
///
/// `after_seconds` is expressed on the gap-free active timeline. This keeps
/// replay and dataset sampling contiguous while retaining enough provenance to
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
    #[serde(flatten)]
    identity: ReplayIdentity,
    #[serde(flatten)]
    timeline: ReplayTimeline,
    #[serde(flatten)]
    transport: ReplayTransport,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplayIdentity {
    episode: String,
    /// The arm being driven.
    robot: String,
}

impl ReplayIdentity {
    pub fn new(episode: impl Into<String>, robot: impl Into<String>) -> Self {
        Self {
            episode: episode.into(),
            robot: robot.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ReplayTimeline {
    /// Frames played so far.
    frame: u64,
    frames: u64,
    elapsed_seconds: f64,
    duration_seconds: f64,
}

impl ReplayTimeline {
    pub const fn new(frame: u64, frames: u64, elapsed_seconds: f64) -> Self {
        Self {
            frame,
            frames,
            elapsed_seconds,
            duration_seconds: 0.0,
        }
    }
    pub const fn with_duration_seconds(mut self, duration_seconds: f64) -> Self {
        self.duration_seconds = duration_seconds;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplayTransport {
    /// Whether this is Timeline transport or a one-pose physical seek.
    #[serde(default)]
    purpose: ReplayPurpose,
    /// Positioning never advances the episode clock or recorded media.
    #[serde(default)]
    phase: ReplayPhase,
    /// Whether playback is holding its current frame without discarding the
    /// loaded trajectory. Resuming continues from this exact position.
    #[serde(default)]
    paused: bool,
}

impl ReplayTransport {
    pub const fn new(purpose: ReplayPurpose, phase: ReplayPhase, paused: bool) -> Self {
        Self {
            purpose,
            phase,
            paused,
        }
    }
}

impl ReplayProgress {
    pub fn new(
        identity: ReplayIdentity,
        timeline: ReplayTimeline,
        transport: ReplayTransport,
    ) -> Self {
        Self {
            identity,
            timeline,
            transport,
        }
    }
    pub fn episode(&self) -> &str {
        &self.identity.episode
    }
    pub fn robot(&self) -> &str {
        &self.identity.robot
    }
    pub const fn frame(&self) -> u64 {
        self.timeline.frame
    }
    pub const fn frames(&self) -> u64 {
        self.timeline.frames
    }
    pub const fn elapsed_seconds(&self) -> f64 {
        self.timeline.elapsed_seconds
    }
    pub const fn duration_seconds(&self) -> f64 {
        self.timeline.duration_seconds
    }
    pub const fn purpose(&self) -> ReplayPurpose {
        self.transport.purpose
    }
    pub const fn phase(&self) -> ReplayPhase {
        self.transport.phase
    }
    pub const fn paused(&self) -> bool {
        self.transport.paused
    }
    #[cfg(any(test, feature = "test-support"))]
    pub const fn set_transport(&mut self, transport: ReplayTransport) {
        self.transport = transport;
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[test]
    fn older_replay_progress_defaults_to_timeline_playback() {
        let replay: ReplayProgress = serde_json::from_str(
            r#"{
                "episode":"episode_000001",
                "robot":"follower",
                "frame":12,
                "frames":60,
                "elapsed_seconds":0.2,
                "duration_seconds":1.0
            }"#,
        )
        .unwrap();

        assert_eq!(replay.purpose(), ReplayPurpose::Timeline);
        assert_eq!(replay.phase(), ReplayPhase::Playing);
        assert!(!replay.paused());
    }

    #[test]
    fn an_unfinished_take_is_listed_but_not_offered() {
        struct EpisodeTest;

        impl EpisodeTest {
            fn episode(frames: u64, problem: Option<String>) -> EpisodeInfo {
                EpisodeInfo::new("episode_000001", "default")
                    .with_arms(vec![EpisodeArm::new(
                        "follower",
                        Some("so101_follower".to_owned()),
                        vec!["shoulder_pan".to_owned()],
                    )])
                    .with_source("Teleoperation")
                    .with_timeline(frames, 2.0)
                    .with_sample_hz(60.0)
                    .with_started_ms(1_770_000_000_000)
                    .with_assessment(false, Some(3.5))
                    .with_problem(problem)
            }
        }

        let episode = EpisodeTest::episode(120, None);
        assert!(episode.usable());
        assert_eq!(
            episode.arm("follower").map(|arm| arm.joints().len()),
            Some(1)
        );
        assert_eq!(episode.arm("leader"), None);
        assert!(!EpisodeTest::episode(120, Some("recording did not finish".to_owned())).usable());
        assert!(!EpisodeTest::episode(0, None).usable());
    }
}
