//! The episode being recorded: what it records, how its samples reach its file,
//! and what it was once it stopped.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use spiral_robotics_common::recording::{EpisodeArm, EpisodeInfo, RecordingGap};
use spiral_robotics_common::robots::MotorLimits;

use super::EpisodeId;
pub use spiral_robotics_hardware::Sample;
use spiral_robotics_hardware::{
    ArmConditionSummary as HardwareArmConditionSummary, CaptureClock, CompletedRecording,
    EpisodeCheckpoint, RecordingCompletion, RecordingWriter,
};

use super::episode_file::{EpisodeFile, EpisodeSink};

const NO_SAMPLE_TIMESTAMP_BITS: u64 = u64::MAX;

#[derive(Debug)]
pub enum EpisodeError {
    InvalidId(String),
    NotFound(String),
    Invalid(String),
}

impl EpisodeError {
    pub(super) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

impl fmt::Display for EpisodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId(id) => write!(
                formatter,
                "episode id {id:?} must be letters, digits, '-', or '_'"
            ),
            Self::NotFound(id) => write!(formatter, "no episode named {id:?}"),
            Self::Invalid(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for EpisodeError {}

/// What one arm's motors were like as an episode opened.
///
/// Every vector is parallel to the arm's declared joint order, so a value at
/// index `j` belongs to joint `j`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArmCondition {
    arm: String,
    temperature_celsius: Vec<Option<f64>>,
    voltage_volts: Vec<Option<f64>>,
    limits: Vec<MotorLimits>,
}

impl ArmCondition {
    pub fn new(arm: String) -> Self {
        Self {
            arm,
            temperature_celsius: Vec::new(),
            voltage_volts: Vec::new(),
            limits: Vec::new(),
        }
    }

    pub fn with_measurements(
        mut self,
        temperature_celsius: Vec<Option<f64>>,
        voltage_volts: Vec<Option<f64>>,
    ) -> Self {
        self.temperature_celsius = temperature_celsius;
        self.voltage_volts = voltage_volts;
        self
    }

    pub fn with_limits(mut self, limits: Vec<MotorLimits>) -> Self {
        self.limits = limits;
        self
    }

    pub(super) fn arm(&self) -> &str {
        &self.arm
    }
    pub(super) fn temperature_celsius(&self) -> &[Option<f64>] {
        &self.temperature_celsius
    }
    pub(super) fn voltage_volts(&self) -> &[Option<f64>] {
        &self.voltage_volts
    }
    pub(super) fn limits(&self) -> &[MotorLimits] {
        &self.limits
    }

    /// True when nothing was actually measured, so the recording leaves these
    /// fields absent instead of inventing an all-empty observation.
    pub(super) fn is_empty(&self) -> bool {
        self.temperature_celsius.is_empty()
            && self.voltage_volts.is_empty()
            && self.limits.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct EpisodeStart {
    identity: EpisodeIdentity,
    recording: EpisodeRecordingMetadata,
    arms: Vec<EpisodeArm>,
    condition: Vec<ArmCondition>,
}

#[derive(Clone, Debug)]
struct EpisodeIdentity {
    id: String,
    dataset_id: String,
}

#[derive(Clone, Debug)]
struct EpisodeRecordingMetadata {
    started_ms: i64,
    sample_hz: f64,
    source: String,
}

impl EpisodeStart {
    pub(super) fn new(id: String, dataset_id: String) -> Self {
        Self {
            identity: EpisodeIdentity { id, dataset_id },
            recording: EpisodeRecordingMetadata {
                started_ms: 0,
                sample_hz: 0.0,
                source: String::new(),
            },
            arms: Vec::new(),
            condition: Vec::new(),
        }
    }

    pub(super) fn with_timing(mut self, started_ms: i64, sample_hz: f64) -> Self {
        self.recording.started_ms = started_ms;
        self.recording.sample_hz = sample_hz;
        self
    }

    fn with_current_timing(self, sample_hz: f64) -> Self {
        let started_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        self.with_timing(started_ms, sample_hz)
    }

    pub(super) fn with_source(mut self, source: String) -> Self {
        self.recording.source = source;
        self
    }

    pub(super) fn with_arms(mut self, arms: Vec<EpisodeArm>, condition: Vec<ArmCondition>) -> Self {
        self.arms = arms;
        self.condition = condition;
        self
    }

    pub fn id(&self) -> &str {
        &self.identity.id
    }
    pub fn dataset_id(&self) -> &str {
        &self.identity.dataset_id
    }
    pub(super) const fn started_ms(&self) -> i64 {
        self.recording.started_ms
    }
    pub(super) const fn sample_hz(&self) -> f64 {
        self.recording.sample_hz
    }
    pub(super) fn source(&self) -> &str {
        &self.recording.source
    }
    pub fn arms(&self) -> &[EpisodeArm] {
        &self.arms
    }
    /// Opening motor readings, at most one entry per arm and only for arms
    /// whose hardware answered while the episode was opening.
    pub(super) fn condition(&self) -> &[ArmCondition] {
        &self.condition
    }
}

/// Inputs supplied by the recording workflow before storage assigns identity
/// and a durable start time.
pub struct EpisodeRecordingRequest {
    dataset_id: String,
    source: String,
    sample_hz: f64,
    arms: Vec<EpisodeArm>,
    condition: Vec<ArmCondition>,
}

impl EpisodeRecordingRequest {
    pub fn new(dataset_id: String, source: String, sample_hz: f64) -> Self {
        Self {
            dataset_id,
            source,
            sample_hz,
            arms: Vec::new(),
            condition: Vec::new(),
        }
    }

    pub fn with_arms(mut self, arms: Vec<EpisodeArm>, condition: Vec<ArmCondition>) -> Self {
        self.arms = arms;
        self.condition = condition;
        self
    }
}

#[derive(Clone, Debug)]
pub(super) struct EpisodeCompletion {
    duration_seconds: f64,
    recording_gaps: Vec<RecordingGap>,
    max_follow_error_degrees: Option<f64>,
    discarded: bool,
    condition: Vec<ArmConditionSummary>,
}

/// The extremes one arm's motors reached across a whole episode.
///
/// Every vector is parallel to the arm's declared joint order.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ArmConditionSummary {
    arm: String,
    peak_temperature_celsius: Vec<Option<f64>>,
    min_voltage_volts: Vec<Option<f64>>,
    peak_load: Vec<Option<f64>>,
    peak_current_amps: Vec<Option<f64>>,
}

impl ArmConditionSummary {
    pub(super) fn arm(&self) -> &str {
        &self.arm
    }
    pub(super) fn peak_temperature_celsius(&self) -> &[Option<f64>] {
        &self.peak_temperature_celsius
    }
    pub(super) fn min_voltage_volts(&self) -> &[Option<f64>] {
        &self.min_voltage_volts
    }
    pub(super) fn peak_load(&self) -> &[Option<f64>] {
        &self.peak_load
    }
    pub(super) fn peak_current_amps(&self) -> &[Option<f64>] {
        &self.peak_current_amps
    }

    /// True when nothing was actually measured; see [`ArmCondition::is_empty`].
    pub(super) fn is_empty(&self) -> bool {
        self.peak_temperature_celsius.is_empty()
            && self.min_voltage_volts.is_empty()
            && self.peak_load.is_empty()
            && self.peak_current_amps.is_empty()
    }
}

impl From<&HardwareArmConditionSummary> for ArmConditionSummary {
    fn from(summary: &HardwareArmConditionSummary) -> Self {
        Self {
            arm: summary.arm().to_owned(),
            peak_temperature_celsius: summary.peak_temperature_celsius().to_vec(),
            min_voltage_volts: summary.min_voltage_volts().to_vec(),
            peak_load: summary.peak_load().to_vec(),
            peak_current_amps: summary.peak_current_amps().to_vec(),
        }
    }
}

impl From<&RecordingCompletion> for EpisodeCompletion {
    fn from(completion: &RecordingCompletion) -> Self {
        Self::new(
            completion.duration_seconds(),
            completion.recording_gaps().to_vec(),
            completion
                .condition()
                .iter()
                .map(ArmConditionSummary::from)
                .collect(),
        )
        .with_max_follow_error_degrees(completion.max_follow_error_degrees())
        .with_discarded(completion.discarded())
    }
}

impl EpisodeCompletion {
    pub(super) fn new(
        duration_seconds: f64,
        recording_gaps: Vec<RecordingGap>,
        condition: Vec<ArmConditionSummary>,
    ) -> Self {
        Self {
            duration_seconds,
            recording_gaps,
            max_follow_error_degrees: None,
            discarded: false,
            condition,
        }
    }

    const fn with_max_follow_error_degrees(
        mut self,
        max_follow_error_degrees: Option<f64>,
    ) -> Self {
        self.max_follow_error_degrees = max_follow_error_degrees;
        self
    }

    const fn with_discarded(mut self, discarded: bool) -> Self {
        self.discarded = discarded;
        self
    }

    pub(super) const fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }

    pub(super) fn recording_gaps(&self) -> &[RecordingGap] {
        &self.recording_gaps
    }

    pub(super) const fn max_follow_error_degrees(&self) -> Option<f64> {
        self.max_follow_error_degrees
    }

    pub(super) const fn discarded(&self) -> bool {
        self.discarded
    }

    /// Closing motor extremes, at most one entry per arm. See
    /// [`EpisodeStart::condition`].
    pub(super) fn condition(&self) -> &[ArmConditionSummary] {
        &self.condition
    }
}

/// What an episode was once it stopped, or as of its last pause: how it started,
/// how it ended, and how many samples it holds.
#[derive(Clone, Debug)]
pub struct EpisodeSummary {
    start: EpisodeStart,
    completion: EpisodeCompletion,
    frames: u64,
}

impl EpisodeSummary {
    pub(super) const fn new(
        start: EpisodeStart,
        completion: EpisodeCompletion,
        frames: u64,
    ) -> Self {
        Self {
            start,
            completion,
            frames,
        }
    }

    pub const fn start(&self) -> &EpisodeStart {
        &self.start
    }

    pub(super) const fn completion(&self) -> &EpisodeCompletion {
        &self.completion
    }

    pub(super) const fn frames(&self) -> u64 {
        self.frames
    }

    pub fn info(&self) -> EpisodeInfo {
        EpisodeInfo::new(self.start.id(), self.start.dataset_id())
            .with_arms(self.start.arms().to_vec())
            .with_frames(self.frames)
    }
}

/// An episode built whole, for tests that need a recording file.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug)]
pub struct FullEpisode {
    start: EpisodeStart,
    samples: Vec<Sample>,
    completion: EpisodeCompletion,
}

#[cfg(any(test, feature = "test-support"))]
impl FullEpisode {
    pub(super) fn from_recording(
        start: EpisodeStart,
        samples: Vec<Sample>,
        completion: EpisodeCompletion,
    ) -> Self {
        Self {
            start,
            samples,
            completion,
        }
    }
    pub const fn start(&self) -> &EpisodeStart {
        &self.start
    }
    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }
    pub(super) const fn completion(&self) -> &EpisodeCompletion {
        &self.completion
    }
}

/// The summary of the episode this connector recorded last, for its review.
/// Recorded episodes are read from their files, never from here.
#[derive(Default)]
pub struct RecordingBuffer {
    completed: Arc<Mutex<Option<EpisodeSummary>>>,
}

impl RecordingBuffer {
    pub fn new() -> Self {
        Self {
            completed: Arc::new(Mutex::new(None)),
        }
    }

    /// A new episode's identity and start.
    pub fn begin(&self, request: EpisodeRecordingRequest) -> Result<EpisodeStart, EpisodeError> {
        if request.arms.is_empty() {
            return Err(EpisodeError::Invalid(
                "an episode needs at least one connected arm to sample".to_owned(),
            ));
        }
        Ok(
            EpisodeStart::new(uuid::Uuid::new_v4().to_string(), request.dataset_id)
                .with_current_timing(request.sample_hz)
                .with_source(request.source)
                .with_arms(request.arms, request.condition),
        )
    }

    /// The writer the control loop records `start`'s samples into `file` with.
    pub fn writer(&self, start: EpisodeStart, file: &EpisodeFile) -> EpisodeWriter {
        EpisodeWriter {
            start,
            clock: CaptureClock::default(),
            sink: file.sink(),
            completed: Arc::clone(&self.completed),
            accepted: AtomicU64::new(0),
            last_queued_seconds: AtomicU64::new(NO_SAMPLE_TIMESTAMP_BITS),
        }
    }

    /// The summary of episode `id`, once it stopped or paused.
    pub fn summary(&self, id: &str) -> Result<EpisodeSummary, EpisodeError> {
        EpisodeId::try_from(id).map_err(|_| EpisodeError::InvalidId(id.into()))?;
        self.completed
            .lock_unpoisoned()
            .as_ref()
            .filter(|summary| summary.start.id() == id)
            .cloned()
            .ok_or_else(|| EpisodeError::NotFound(id.to_owned()))
    }
}

/// How the control loop records one episode: each sample it pushes queues for
/// the episode's file, and its summary is kept once it pauses or stops.
pub struct EpisodeWriter {
    start: EpisodeStart,
    clock: CaptureClock,
    sink: EpisodeSink,
    completed: Arc<Mutex<Option<EpisodeSummary>>>,
    accepted: AtomicU64,
    last_queued_seconds: AtomicU64,
}

impl EpisodeWriter {
    pub fn clock(&self) -> CaptureClock {
        self.clock.clone()
    }
    pub fn id(&self) -> &str {
        self.start.id()
    }
    fn fail(&self, message: &str) -> bool {
        self.sink.fail(message.to_owned());
        false
    }
    /// Keep the episode's summary as of `completion`.
    fn summarize(&self, completion: EpisodeCompletion) -> EpisodeSummary {
        let summary = EpisodeSummary::new(
            self.start.clone(),
            completion,
            self.accepted.load(Ordering::Relaxed),
        );
        *self.completed.lock_unpoisoned() = Some(summary.clone());
        summary
    }
}

impl RecordingWriter for EpisodeWriter {
    fn update_clock(&self, now: std::time::Instant, elapsed: Option<std::time::Duration>) {
        self.clock.update(now, elapsed);
    }
    fn id(&self) -> &str {
        self.start.id()
    }
    fn dataset_id(&self) -> &str {
        self.start.dataset_id()
    }
    fn push(&self, sample: Sample) -> bool {
        if !sample.seconds().is_finite() || sample.seconds() < 0.0 {
            return self.fail("sample time must be finite and non-negative");
        }
        let next = sample.seconds().to_bits();
        let mut previous = self.last_queued_seconds.load(Ordering::Relaxed);
        loop {
            if previous != NO_SAMPLE_TIMESTAMP_BITS && sample.seconds() < f64::from_bits(previous) {
                return self.fail("sample time moved backwards");
            }
            match self.last_queued_seconds.compare_exchange_weak(
                previous,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => previous = actual,
            }
        }
        let queued = self.sink.sample(sample);
        if queued {
            self.accepted.fetch_add(1, Ordering::Relaxed);
        }
        queued
    }
    /// Keep the episode's summary, and put every sample pushed before this on
    /// disk, for a review of the paused episode.
    fn checkpoint(&self, completion: RecordingCompletion) -> Result<EpisodeCheckpoint, String> {
        self.summarize(EpisodeCompletion::from(&completion));
        self.sink.flush()
    }
    fn failure(&self) -> Option<String> {
        self.sink.failure()
    }
    fn finish(
        self: Box<Self>,
        completion: RecordingCompletion,
    ) -> Result<CompletedRecording, String> {
        if let Some(failure) = self.failure() {
            return Err(failure);
        }
        let summary = self.summarize(EpisodeCompletion::from(&completion));
        Ok(CompletedRecording::new(summary.start.id(), summary.frames))
    }
}

pub trait EpisodeArmCollection: Sized {
    fn from_joint_lists(arms: &[(String, Vec<String>)]) -> Result<Vec<Self>, EpisodeError>;
}

impl EpisodeArmCollection for EpisodeArm {
    fn from_joint_lists(arms: &[(String, Vec<String>)]) -> Result<Vec<Self>, EpisodeError> {
        let mut seen = BTreeSet::new();
        arms.iter()
            .map(|(id, joints)| {
                if joints.is_empty() {
                    return Err(EpisodeError::Invalid(format!(
                        "arm {id:?} reported no joints"
                    )));
                }
                if !seen.insert(id.clone()) {
                    return Err(EpisodeError::Invalid(format!("arm {id:?} was named twice")));
                }
                Ok(Self::new(id, None, joints.clone()))
            })
            .collect()
    }
}
pub(super) trait UnpoisonedMutex<T> {
    fn lock_unpoisoned(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> UnpoisonedMutex<T> for Mutex<T> {
    fn lock_unpoisoned(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod recording_buffer_tests {
    use super::*;

    #[test]
    fn only_the_last_episode_is_reviewable_once_its_summary_is_kept() {
        let directory = tempfile::tempdir().unwrap();
        let buffer = RecordingBuffer::new();
        assert!(matches!(
            buffer.summary("unknown-episode"),
            Err(EpisodeError::NotFound(_))
        ));
        let arms = vec![EpisodeArm::new("arm", None, vec!["joint".into()])];
        for id in ["first", "second"] {
            let start =
                EpisodeStart::new(id.into(), "destination".into()).with_arms(arms.clone(), vec![]);
            let file = EpisodeFile::create(directory.path().join(id), &start, vec![]).unwrap();
            let writer = buffer.writer(start, &file);
            assert!(writer.push(Sample::new(0.0, vec![vec![Some(0.25)]], vec![vec![123]])));
            let summary = writer.summarize(EpisodeCompletion::new(0.1, vec![], vec![]));
            file.finish(summary).unwrap();
            assert_eq!(buffer.summary(id).unwrap().info().frames(), 1);
        }
        assert!(matches!(
            buffer.summary("first"),
            Err(EpisodeError::NotFound(_))
        ));
        assert!(buffer.summary("second").is_ok());
    }
}

#[cfg(test)]
mod recording_golden_tests {
    use super::*;
    use sha2::Digest as _;

    /// One deterministic episode touching every recorded field and presence case.
    fn episode() -> FullEpisode {
        let arms = vec![
            EpisodeArm::new(
                "leader",
                Some("so101_leader".to_owned()),
                vec!["shoulder".to_owned(), "elbow".to_owned()],
            ),
            EpisodeArm::new(
                "follower",
                None,
                vec!["shoulder".to_owned(), "elbow".to_owned()],
            ),
        ];
        let limits = MotorLimits::default()
            .with_max_temperature_celsius(Some(70.0))
            .with_voltage_range(Some(4.5), Some(14.0))
            .with_max_load(Some(0.9));
        let condition = vec![
            ArmCondition::new("follower".to_owned())
                .with_measurements(vec![Some(31.5), None], vec![Some(12.1), Some(12.0)])
                .with_limits(vec![limits, MotorLimits::default()]),
        ];
        let start = EpisodeStart::new("episode_000042".to_owned(), "default".to_owned())
            .with_timing(1_700_000_000_000, 60.0)
            .with_source("teleoperation".to_owned())
            .with_arms(arms, condition);
        let samples = vec![
            Sample::new(
                0.0,
                vec![vec![Some(0.1), None], vec![Some(0.2), Some(0.3)]],
                vec![vec![10, 20], vec![30, -40]],
            )
            .with_follow_error_degrees(Some(1.25))
            .with_velocity(Some(vec![
                vec![Some(2.0), None],
                vec![Some(4.0), Some(5.0)],
            ]))
            .with_action(vec![None, Some(vec![Some(0.25), None])]),
            Sample::new(
                0.5,
                vec![vec![Some(0.4), Some(0.5)], vec![Some(0.6), Some(0.7)]],
                vec![vec![11, 21], vec![31, 41]],
            )
            .with_action(vec![
                Some(vec![Some(0.1), Some(0.2)]),
                Some(vec![Some(0.3), Some(0.4)]),
            ]),
            Sample::new(
                1.25,
                vec![vec![None, None], vec![Some(0.8), Some(0.9)]],
                vec![vec![12, 22], vec![32, 42]],
            ),
        ];
        let peaks = ArmConditionSummary {
            arm: "leader".to_owned(),
            peak_temperature_celsius: vec![Some(40.0), None],
            min_voltage_volts: vec![Some(11.9), None],
            peak_load: vec![Some(0.5), None],
            peak_current_amps: vec![None, Some(1.2)],
        };
        let completion = EpisodeCompletion::new(
            2.5,
            vec![RecordingGap::new(0.75, 0.25), RecordingGap::new(1.0, 0.125)],
            vec![peaks],
        )
        .with_max_follow_error_degrees(Some(2.0))
        .with_discarded(true);
        FullEpisode::from_recording(start, samples, completion)
    }

    fn camera(stream: &str, camera_id: &str) -> crate::CameraRecording {
        let key: &[u8] = &[
            0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 0x67, 0x64, 0, 0x1e, 0, 0, 1, 0x68, 0x88, 0, 0, 1,
            0x65, 0x88,
        ];
        let delta: &[u8] = &[0, 0, 0, 1, 9, 0xf0, 0, 0, 1, 0x41, 0x88];
        crate::CameraRecording::from_annex_b(
            stream.to_owned(),
            (640, 480, 30),
            ([key, delta].concat(), vec![0, 33_333_333]),
        )
        .unwrap()
        .with_camera_id(camera_id.to_owned())
    }

    /// Every message with its channel, schema, and exact payload bytes. The
    /// file bytes themselves are not stable: the MCAP summary lists schemas in
    /// hash-map order.
    fn messages(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let mut rendering = String::new();
        for message in mcap::MessageStream::new(bytes).unwrap() {
            let message = message.unwrap();
            let channel = &message.channel;
            let schema = channel.schema.as_ref().unwrap();
            writeln!(
                rendering,
                "{} {:?} {} {} {} {} {} {} {}",
                channel.topic,
                channel.metadata,
                channel.message_encoding,
                schema.name,
                hex(&sha2::Sha256::digest(&schema.data)),
                message.sequence,
                message.log_time,
                message.publish_time,
                hex(&message.data),
            )
            .unwrap();
        }
        rendering
    }

    #[test]
    fn recording_messages_match_the_golden_episode() {
        let cameras = [
            camera("follower.camera.wrist", "camera-1"),
            camera("leader.camera.top", "camera-2"),
        ];
        let bytes = crate::McapRecording::encode(&episode(), &cameras).unwrap();
        // Messages are written as the episode produces them, so the rendering
        // is sorted: the golden episode pins what is recorded, not its order.
        let mut lines = messages(&bytes)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        lines.sort();
        let rendering = lines.join("\n");
        let digest = sha2::Sha256::digest(rendering.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            digest,
            "1201ceed88b8a833ffc64107bfa97ec08d6e8312e65ca65c42240153fabc43f3"
        );
    }
}
