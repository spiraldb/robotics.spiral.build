//! Read-only episode catalog use cases and delivery-neutral projections.

use std::collections::BTreeMap;

use spiral_robotics_core::OperationError;
use spiral_robotics_core::identity::EpisodeId;

const MAX_TRACE_BUCKETS: usize = 2_048;
const MAX_TIMELINE_BUCKETS: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EpisodeArmRecord {
    id: String,
    robot_type: Option<String>,
    joints: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RecordingGapRecord {
    after_seconds: f64,
    duration_seconds: f64,
}

impl EpisodeArmRecord {
    pub(crate) fn new(
        id: impl Into<String>,
        robot_type: Option<String>,
        joints: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            robot_type,
            joints,
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn robot_type(&self) -> Option<&str> {
        self.robot_type.as_deref()
    }
    pub(crate) fn joints(&self) -> &[String] {
        &self.joints
    }
}

impl RecordingGapRecord {
    pub(crate) fn new(after_seconds: f64, duration_seconds: f64) -> Self {
        Self {
            after_seconds,
            duration_seconds,
        }
    }

    pub(crate) fn after_seconds(&self) -> f64 {
        self.after_seconds
    }
    pub(crate) fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodeRecord {
    identity: EpisodeRecordIdentity,
    capture: EpisodeCaptureSummary,
    disposition: EpisodeDisposition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EpisodeRecordIdentity {
    id: String,
    dataset_id: String,
    source: String,
    started_ms: i64,
}

impl EpisodeRecordIdentity {
    pub(crate) fn new(
        id: impl Into<String>,
        dataset_id: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            dataset_id: dataset_id.into(),
            source: source.into(),
            started_ms: 0,
        }
    }

    pub(crate) fn started_at(mut self, started_ms: i64) -> Self {
        self.started_ms = started_ms;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodeCaptureSummary {
    arms: Vec<EpisodeArmRecord>,
    frames: u64,
    duration_seconds: f64,
    recording_gaps: Vec<RecordingGapRecord>,
    sample_hz: f64,
    max_follow_error_degrees: Option<f64>,
}

impl EpisodeCaptureSummary {
    pub(crate) fn new(arms: Vec<EpisodeArmRecord>, frames: u64, duration_seconds: f64) -> Self {
        Self {
            arms,
            frames,
            duration_seconds,
            recording_gaps: Vec::new(),
            sample_hz: 0.0,
            max_follow_error_degrees: None,
        }
    }

    pub(crate) fn with_timing(
        mut self,
        recording_gaps: Vec<RecordingGapRecord>,
        sample_hz: f64,
    ) -> Self {
        self.recording_gaps = recording_gaps;
        self.sample_hz = sample_hz;
        self
    }

    pub(crate) fn with_maximum_follow_error(mut self, error_degrees: Option<f64>) -> Self {
        self.max_follow_error_degrees = error_degrees;
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct EpisodeDisposition {
    discarded: bool,
    problem: Option<String>,
}

impl EpisodeDisposition {
    pub(crate) fn new(discarded: bool, problem: Option<String>) -> Self {
        Self { discarded, problem }
    }
}

impl EpisodeRecord {
    pub(crate) fn new(
        identity: EpisodeRecordIdentity,
        capture: EpisodeCaptureSummary,
        disposition: EpisodeDisposition,
    ) -> Self {
        Self {
            identity,
            capture,
            disposition,
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.identity.id
    }
    pub(crate) fn dataset_id(&self) -> &str {
        &self.identity.dataset_id
    }
    pub(crate) fn source(&self) -> &str {
        &self.identity.source
    }
    pub(crate) fn started_ms(&self) -> i64 {
        self.identity.started_ms
    }
    pub(crate) fn arms(&self) -> &[EpisodeArmRecord] {
        &self.capture.arms
    }
    pub(crate) fn frames(&self) -> u64 {
        self.capture.frames
    }
    pub(crate) fn duration_seconds(&self) -> f64 {
        self.capture.duration_seconds
    }
    pub(crate) fn recording_gaps(&self) -> &[RecordingGapRecord] {
        &self.capture.recording_gaps
    }
    pub(crate) fn sample_hz(&self) -> f64 {
        self.capture.sample_hz
    }
    pub(crate) fn max_follow_error_degrees(&self) -> Option<f64> {
        self.capture.max_follow_error_degrees
    }
    pub(crate) fn discarded(&self) -> bool {
        self.disposition.discarded
    }
    pub(crate) fn problem(&self) -> Option<&str> {
        self.disposition.problem.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EpisodeTraceQuery {
    episode: String,
    buckets: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineMetadataRecord {
    samples: u64,
    first_seconds: Option<f64>,
    last_seconds: Option<f64>,
}

impl EpisodeTraceQuery {
    pub(crate) fn new(episode: impl Into<String>, buckets: usize) -> Self {
        Self {
            episode: episode.into(),
            buckets,
        }
    }

    pub(crate) fn episode(&self) -> &str {
        &self.episode
    }

    pub(crate) fn buckets(&self) -> usize {
        self.buckets
    }
}

impl TimelineMetadataRecord {
    pub(crate) fn new(samples: u64, first_seconds: Option<f64>, last_seconds: Option<f64>) -> Self {
        Self {
            samples,
            first_seconds,
            last_seconds,
        }
    }

    pub(crate) fn samples(&self) -> u64 {
        self.samples
    }
    pub(crate) fn first_seconds(&self) -> Option<f64> {
        self.first_seconds
    }
    pub(crate) fn last_seconds(&self) -> Option<f64> {
        self.last_seconds
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VideoAssetRecord {
    identity: VideoAssetIdentity,
    format: VideoAssetFormat,
    timeline: VideoAssetTimeline,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VideoAssetIdentity {
    stream_id: String,
    camera_id: String,
    segment_index: u32,
}

impl VideoAssetIdentity {
    pub(crate) fn new(
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VideoAssetFormat {
    width: u32,
    height: u32,
    fps: u32,
}

impl VideoAssetFormat {
    pub(crate) fn new(width: u32, height: u32, fps: u32) -> Self {
        Self { width, height, fps }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VideoAssetTimeline {
    frames: u64,
    duration_seconds: f64,
    timeline_offset_seconds: Option<f64>,
}

impl VideoAssetTimeline {
    pub(crate) fn new(
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

impl VideoAssetRecord {
    pub(crate) fn new(
        identity: VideoAssetIdentity,
        format: VideoAssetFormat,
        timeline: VideoAssetTimeline,
    ) -> Self {
        Self {
            identity,
            format,
            timeline,
        }
    }

    pub(crate) fn stream_id(&self) -> &str {
        &self.identity.stream_id
    }
    pub(crate) fn camera_id(&self) -> &str {
        &self.identity.camera_id
    }
    pub(crate) fn segment_index(&self) -> u32 {
        self.identity.segment_index
    }
    pub(crate) fn width(&self) -> u32 {
        self.format.width
    }
    pub(crate) fn height(&self) -> u32 {
        self.format.height
    }
    pub(crate) fn fps(&self) -> u32 {
        self.format.fps
    }
    pub(crate) fn frames(&self) -> u64 {
        self.timeline.frames
    }
    pub(crate) fn duration_seconds(&self) -> f64 {
        self.timeline.duration_seconds
    }
    pub(crate) fn timeline_offset_seconds(&self) -> Option<f64> {
        self.timeline.timeline_offset_seconds
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodeInspectionRecord {
    episode: EpisodeRecord,
    sample_times_seconds: Vec<f64>,
    follow_error_degrees: Vec<Option<f64>>,
    arms: Vec<EpisodeInspectionArmRecord>,
    videos: Vec<VideoAssetRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodeInspectionArmRecord {
    arm: String,
    joints: Vec<String>,
    normalized: Vec<Option<f64>>,
    raw: Vec<i64>,
    velocity: OptionalFrameMatrixRecord,
    effort: OptionalFrameMatrixRecord,
    action: OptionalFrameMatrixRecord,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OptionalFrameMatrixRecord {
    present: Vec<bool>,
    values: Vec<Option<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodeTimelineRecord {
    episode: EpisodeRecord,
    samples: TimelineMetadataRecord,
    videos: Vec<VideoAssetRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineArmSampleRecord {
    arm: String,
    normalized: BTreeMap<String, Option<f64>>,
    raw: BTreeMap<String, i64>,
    velocity: Option<BTreeMap<String, Option<f64>>>,
    effort: Option<BTreeMap<String, Option<f64>>>,
    action: Option<BTreeMap<String, Option<f64>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineSampleRecord {
    frame: u64,
    seconds: f64,
    follow_error_degrees: Option<f64>,
    arms: Vec<TimelineArmSampleRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineTraceCollection {
    episode: String,
    samples: u64,
    buckets: usize,
    start_seconds: Option<f64>,
    end_seconds: Option<f64>,
    lanes: Vec<TimelineTraceLaneRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineTraceLaneRecord {
    joint: String,
    series: Vec<TimelineTraceSeriesRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineTraceSeriesRecord {
    arm: String,
    points: Vec<TimelineTracePointRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TimelineTracePointRecord {
    seconds: f64,
    value: Option<f64>,
}

type EpisodeInspectionParts = (
    EpisodeRecord,
    Vec<f64>,
    Vec<Option<f64>>,
    Vec<EpisodeInspectionArmRecord>,
    Vec<VideoAssetRecord>,
);

type EpisodeInspectionArmParts = (
    String,
    Vec<String>,
    Vec<Option<f64>>,
    Vec<i64>,
    OptionalFrameMatrixRecord,
    OptionalFrameMatrixRecord,
    OptionalFrameMatrixRecord,
);

type TimelineArmSampleParts = (
    String,
    BTreeMap<String, Option<f64>>,
    BTreeMap<String, i64>,
    Option<BTreeMap<String, Option<f64>>>,
    Option<BTreeMap<String, Option<f64>>>,
    Option<BTreeMap<String, Option<f64>>>,
);

impl EpisodeInspectionRecord {
    pub(crate) fn new(
        episode: EpisodeRecord,
        sample_times_seconds: Vec<f64>,
        arms: Vec<EpisodeInspectionArmRecord>,
    ) -> Self {
        Self {
            episode,
            sample_times_seconds,
            follow_error_degrees: Vec::new(),
            arms,
            videos: Vec::new(),
        }
    }

    pub(crate) fn with_follow_error(mut self, values: Vec<Option<f64>>) -> Self {
        self.follow_error_degrees = values;
        self
    }

    pub(crate) fn with_videos(mut self, videos: Vec<VideoAssetRecord>) -> Self {
        self.videos = videos;
        self
    }

    pub(crate) fn into_parts(self) -> EpisodeInspectionParts {
        (
            self.episode,
            self.sample_times_seconds,
            self.follow_error_degrees,
            self.arms,
            self.videos,
        )
    }
}

impl EpisodeInspectionArmRecord {
    pub(crate) fn new(
        arm: impl Into<String>,
        joints: Vec<String>,
        normalized: Vec<Option<f64>>,
    ) -> Self {
        Self {
            arm: arm.into(),
            joints,
            normalized,
            raw: Vec::new(),
            velocity: OptionalFrameMatrixRecord::default(),
            effort: OptionalFrameMatrixRecord::default(),
            action: OptionalFrameMatrixRecord::default(),
        }
    }

    pub(crate) fn with_raw(mut self, raw: Vec<i64>) -> Self {
        self.raw = raw;
        self
    }

    pub(crate) fn with_dynamics(
        mut self,
        velocity: OptionalFrameMatrixRecord,
        effort: OptionalFrameMatrixRecord,
    ) -> Self {
        self.velocity = velocity;
        self.effort = effort;
        self
    }

    pub(crate) fn with_action(mut self, action: OptionalFrameMatrixRecord) -> Self {
        self.action = action;
        self
    }

    pub(crate) fn into_parts(self) -> EpisodeInspectionArmParts {
        (
            self.arm,
            self.joints,
            self.normalized,
            self.raw,
            self.velocity,
            self.effort,
            self.action,
        )
    }
}

impl OptionalFrameMatrixRecord {
    pub(crate) fn new(present: Vec<bool>, values: Vec<Option<f64>>) -> Self {
        Self { present, values }
    }

    pub(crate) fn into_parts(self) -> (Vec<bool>, Vec<Option<f64>>) {
        (self.present, self.values)
    }
}

impl EpisodeTimelineRecord {
    pub(crate) fn new(
        episode: EpisodeRecord,
        samples: TimelineMetadataRecord,
        videos: Vec<VideoAssetRecord>,
    ) -> Self {
        Self {
            episode,
            samples,
            videos,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (EpisodeRecord, TimelineMetadataRecord, Vec<VideoAssetRecord>) {
        (self.episode, self.samples, self.videos)
    }
}

impl TimelineArmSampleRecord {
    pub(crate) fn new(
        arm: impl Into<String>,
        normalized: BTreeMap<String, Option<f64>>,
        raw: BTreeMap<String, i64>,
    ) -> Self {
        Self {
            arm: arm.into(),
            normalized,
            raw,
            velocity: None,
            effort: None,
            action: None,
        }
    }

    pub(crate) fn with_dynamics(
        mut self,
        velocity: Option<BTreeMap<String, Option<f64>>>,
        effort: Option<BTreeMap<String, Option<f64>>>,
    ) -> Self {
        self.velocity = velocity;
        self.effort = effort;
        self
    }

    pub(crate) fn with_action(mut self, action: Option<BTreeMap<String, Option<f64>>>) -> Self {
        self.action = action;
        self
    }

    pub(crate) fn into_parts(self) -> TimelineArmSampleParts {
        (
            self.arm,
            self.normalized,
            self.raw,
            self.velocity,
            self.effort,
            self.action,
        )
    }
}

impl TimelineSampleRecord {
    pub(crate) fn new(frame: u64, seconds: f64, arms: Vec<TimelineArmSampleRecord>) -> Self {
        Self {
            frame,
            seconds,
            follow_error_degrees: None,
            arms,
        }
    }

    pub(crate) fn with_follow_error(mut self, error_degrees: Option<f64>) -> Self {
        self.follow_error_degrees = error_degrees;
        self
    }

    pub(crate) fn into_parts(self) -> (u64, f64, Option<f64>, Vec<TimelineArmSampleRecord>) {
        (
            self.frame,
            self.seconds,
            self.follow_error_degrees,
            self.arms,
        )
    }
}

impl TimelineTraceCollection {
    pub(crate) fn new(episode: impl Into<String>, samples: u64, buckets: usize) -> Self {
        Self {
            episode: episode.into(),
            samples,
            buckets,
            start_seconds: None,
            end_seconds: None,
            lanes: Vec::new(),
        }
    }

    pub(crate) fn with_range(
        mut self,
        start_seconds: Option<f64>,
        end_seconds: Option<f64>,
    ) -> Self {
        self.start_seconds = start_seconds;
        self.end_seconds = end_seconds;
        self
    }

    pub(crate) fn with_lanes(mut self, lanes: Vec<TimelineTraceLaneRecord>) -> Self {
        self.lanes = lanes;
        self
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        String,
        u64,
        usize,
        Option<f64>,
        Option<f64>,
        Vec<TimelineTraceLaneRecord>,
    ) {
        (
            self.episode,
            self.samples,
            self.buckets,
            self.start_seconds,
            self.end_seconds,
            self.lanes,
        )
    }
}

impl TimelineTraceLaneRecord {
    pub(crate) fn new(joint: impl Into<String>, series: Vec<TimelineTraceSeriesRecord>) -> Self {
        Self {
            joint: joint.into(),
            series,
        }
    }

    pub(crate) fn into_parts(self) -> (String, Vec<TimelineTraceSeriesRecord>) {
        (self.joint, self.series)
    }
}

impl TimelineTraceSeriesRecord {
    pub(crate) fn new(arm: impl Into<String>, points: Vec<TimelineTracePointRecord>) -> Self {
        Self {
            arm: arm.into(),
            points,
        }
    }

    pub(crate) fn into_parts(self) -> (String, Vec<TimelineTracePointRecord>) {
        (self.arm, self.points)
    }
}

impl TimelineTracePointRecord {
    pub(crate) fn new(seconds: f64, value: Option<f64>) -> Self {
        Self { seconds, value }
    }

    pub(crate) fn seconds(&self) -> f64 {
        self.seconds
    }
    pub(crate) fn value(&self) -> Option<f64> {
        self.value
    }
}

pub(crate) trait EpisodeQueryPort: Send + Sync {
    fn episodes(&self) -> Vec<EpisodeRecord>;
    fn ensure_episode_exists(&self, episode: &str) -> Result<(), OperationError>;
    fn follow_error_trace(
        &self,
        query: EpisodeTraceQuery,
    ) -> Result<Vec<Option<f64>>, OperationError>;
    fn timeline_traces(
        &self,
        query: EpisodeTraceQuery,
    ) -> Result<TimelineTraceCollection, OperationError>;
    fn timeline(&self, episode: String) -> Result<EpisodeTimelineRecord, OperationError>;
    fn inspection(&self, episode: String) -> Result<EpisodeInspectionRecord, OperationError>;
    fn timeline_sample(
        &self,
        episode: String,
        seconds: f64,
    ) -> Result<TimelineSampleRecord, OperationError>;
}

pub(crate) struct EpisodeQueries<'a, P: ?Sized> {
    port: &'a P,
}

impl<'a, P: EpisodeQueryPort + ?Sized> EpisodeQueries<'a, P> {
    pub(crate) fn new(port: &'a P) -> Self {
        Self { port }
    }

    pub(crate) fn authorize_asset(&self, episode: String) -> Result<(), OperationError> {
        let episode = EpisodeId::try_from(episode.as_str())
            .map(String::from)
            .map_err(|_| {
                OperationError::invalid_input(
                    "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
                )
            })?;
        self.port.ensure_episode_exists(&episode)
    }

    pub(crate) fn list(&self) -> Vec<EpisodeRecord> {
        self.port.episodes()
    }

    pub(crate) fn follow_error_trace(
        &self,
        mut query: EpisodeTraceQuery,
    ) -> Result<Vec<Option<f64>>, OperationError> {
        query.episode = EpisodeId::try_from(query.episode.as_str())
            .map(String::from)
            .map_err(|_| {
                OperationError::invalid_input(
                    "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
                )
            })?;
        query.buckets = query.buckets.clamp(1, MAX_TRACE_BUCKETS);
        self.port.follow_error_trace(query)
    }

    pub(crate) fn timeline_traces(
        &self,
        mut query: EpisodeTraceQuery,
    ) -> Result<TimelineTraceCollection, OperationError> {
        query.episode = EpisodeId::try_from(query.episode.as_str())
            .map(String::from)
            .map_err(|_| {
                OperationError::invalid_input(
                    "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
                )
            })?;
        query.buckets = query.buckets.clamp(1, MAX_TIMELINE_BUCKETS);
        self.port.timeline_traces(query)
    }

    pub(crate) fn timeline(
        &self,
        episode: String,
    ) -> Result<EpisodeTimelineRecord, OperationError> {
        let episode = EpisodeId::try_from(episode.as_str())
            .map(String::from)
            .map_err(|_| {
                OperationError::invalid_input(
                    "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
                )
            })?;
        self.port.timeline(episode)
    }

    pub(crate) fn inspection(
        &self,
        episode: String,
    ) -> Result<EpisodeInspectionRecord, OperationError> {
        let episode = EpisodeId::try_from(episode.as_str())
            .map(String::from)
            .map_err(|_| {
                OperationError::invalid_input(
                    "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
                )
            })?;
        self.port.inspection(episode)
    }

    pub(crate) fn timeline_sample(
        &self,
        episode: String,
        seconds: f64,
    ) -> Result<TimelineSampleRecord, OperationError> {
        let episode = EpisodeId::try_from(episode.as_str())
            .map(String::from)
            .map_err(|_| {
                OperationError::invalid_input(
                    "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
                )
            })?;
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(OperationError::InvalidInput(
                "timeline time must be a finite non-negative number".to_owned(),
            ));
        }
        self.port.timeline_sample(episode, seconds)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Queries {
        calls: Mutex<Vec<String>>,
    }

    impl EpisodeQueryPort for Queries {
        fn episodes(&self) -> Vec<EpisodeRecord> {
            self.calls.lock().unwrap().push("list".to_owned());
            Vec::new()
        }

        fn ensure_episode_exists(&self, episode: &str) -> Result<(), OperationError> {
            self.calls.lock().unwrap().push(format!("exists:{episode}"));
            Ok(())
        }

        fn follow_error_trace(
            &self,
            query: EpisodeTraceQuery,
        ) -> Result<Vec<Option<f64>>, OperationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("trace:{}:{}", query.episode, query.buckets));
            Ok(vec![Some(0.25)])
        }

        fn timeline_traces(
            &self,
            query: EpisodeTraceQuery,
        ) -> Result<TimelineTraceCollection, OperationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("traces:{}:{}", query.episode, query.buckets));
            Ok(TimelineTraceCollection {
                episode: query.episode,
                samples: 0,
                buckets: query.buckets,
                start_seconds: None,
                end_seconds: None,
                lanes: Vec::new(),
            })
        }

        fn timeline(&self, episode: String) -> Result<EpisodeTimelineRecord, OperationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("timeline:{episode}"));
            Err(OperationError::NotFound(episode))
        }

        fn inspection(&self, episode: String) -> Result<EpisodeInspectionRecord, OperationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("inspection:{episode}"));
            Err(OperationError::NotFound(episode))
        }

        fn timeline_sample(
            &self,
            episode: String,
            seconds: f64,
        ) -> Result<TimelineSampleRecord, OperationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("sample:{episode}:{seconds}"));
            Ok(TimelineSampleRecord {
                frame: 0,
                seconds,
                follow_error_degrees: None,
                arms: Vec::new(),
            })
        }
    }

    #[test]
    fn read_queries_validate_and_bound_selection_before_exactly_one_port_call() {
        let queries = Queries::default();
        let episode_queries = EpisodeQueries::new(&queries);
        assert_eq!(
            episode_queries
                .follow_error_trace(EpisodeTraceQuery {
                    episode: "episode_000001".to_owned(),
                    buckets: usize::MAX,
                },)
                .unwrap(),
            vec![Some(0.25)]
        );
        let sample = episode_queries
            .timeline_sample("episode_000001".to_owned(), 0.5)
            .unwrap();
        assert_eq!(sample.seconds, 0.5);
        assert!(
            episode_queries
                .timeline_sample("episode_000001".to_owned(), f64::NAN)
                .is_err()
        );
        assert!(
            episode_queries
                .inspection("episode_000001".to_owned())
                .is_err()
        );
        assert!(episode_queries.inspection("../episode".to_owned()).is_err());
        episode_queries
            .authorize_asset("episode_000001".to_owned())
            .unwrap();
        assert!(
            episode_queries
                .authorize_asset("../episode".to_owned())
                .is_err()
        );
        assert!(
            episode_queries
                .authorize_asset(" episode".to_owned())
                .is_err()
        );
        assert_eq!(
            queries.calls.lock().unwrap().as_slice(),
            [
                "trace:episode_000001:2048",
                "sample:episode_000001:0.5",
                "inspection:episode_000001",
                "exists:episode_000001",
            ]
        );
    }
}
