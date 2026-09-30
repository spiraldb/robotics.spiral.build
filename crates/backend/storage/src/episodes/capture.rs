//! Current-session recording samples; no persisted dataset reads.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use spiral_robotics_common::recording::{EpisodeArm, EpisodeInfo, RecordingGap};
use spiral_robotics_common::robots::MotorLimits;

use super::EpisodeId;
pub use spiral_robotics_hardware::Sample;
use spiral_robotics_hardware::{
    ArmConditionSummary as HardwareArmConditionSummary, CaptureClock, CompletedRecording,
    EpisodeCheckpoint, RecordingCompletion, RecordingWriter,
};

use super::episode_schema::EpisodeSample as _;

const SAMPLE_QUEUE_DEPTH: usize = 2_048;
/// Hard storage limit for a single recorded episode.
const MAX_EPISODE_FRAMES: u64 = 1_000_000;
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

/// What one arm's motors were like as a take opened.
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

    pub(super) fn id(&self) -> &str {
        &self.identity.id
    }
    pub(super) fn dataset_id(&self) -> &str {
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
    /// whose hardware answered while the take was opening.
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

/// The extremes one arm's motors reached across a whole take.
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

#[derive(Clone, Debug)]
pub struct FullEpisode {
    start: EpisodeStart,
    samples: Vec<Sample>,
    completion: EpisodeCompletion,
}

impl FullEpisode {
    #[cfg(any(test, feature = "test-support"))]
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

    pub fn summary(&self) -> EpisodeInfo {
        EpisodeInfo::new(self.start.id(), self.start.dataset_id())
            .with_arms(self.start.arms().to_vec())
            .with_frames(self.samples.len() as u64)
    }

    pub(super) const fn completion(&self) -> &EpisodeCompletion {
        &self.completion
    }
}

#[derive(Clone, Debug)]
pub struct EpisodeInspection {
    episode: FullEpisode,
    summary: EpisodeInfo,
}

impl EpisodeInspection {
    pub fn into_parts(self) -> (FullEpisode, EpisodeInfo) {
        (self.episode, self.summary)
    }
}

/// Current-session capture and review only. Persisted datasets are read through
/// dataset/member/topic APIs, never through this buffer.
#[derive(Default)]
pub struct RecordingBuffer {
    completed: Arc<Mutex<Option<FullEpisode>>>,
}

impl RecordingBuffer {
    pub fn new() -> Self {
        Self {
            completed: Arc::new(Mutex::new(None)),
        }
    }

    pub fn inspection(&self, id: &str) -> Result<EpisodeInspection, EpisodeError> {
        let episode = self.completed_episode(id)?;
        let summary = episode.summary();
        Ok(EpisodeInspection { episode, summary })
    }

    pub fn begin(&self, request: EpisodeRecordingRequest) -> Result<EpisodeWriter, EpisodeError> {
        if request.arms.is_empty() {
            return Err(EpisodeError::Invalid(
                "a take needs at least one connected arm to sample".to_owned(),
            ));
        }
        let start = EpisodeStart::new(uuid::Uuid::new_v4().to_string(), request.dataset_id)
            .with_current_timing(request.sample_hz)
            .with_source(request.source)
            .with_arms(request.arms, request.condition);
        EpisodeWriter::start(start, Arc::clone(&self.completed))
    }

    /// Read a completed capture before its MCAP publication finishes.
    pub fn completed_episode(&self, id: &str) -> Result<FullEpisode, EpisodeError> {
        EpisodeId::try_from(id).map_err(|_| EpisodeError::InvalidId(id.into()))?;
        self.completed
            .lock_unpoisoned()
            .as_ref()
            .filter(|episode| episode.start.id() == id)
            .cloned()
            .ok_or_else(|| EpisodeError::NotFound(id.to_owned()))
    }
}

struct WriterShared {
    written: AtomicU64,
    failure: Mutex<Option<String>>,
}
enum WriterMessage {
    Sample(Sample),
    Checkpoint {
        completion: EpisodeCompletion,
        complete: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
}

pub struct EpisodeWriter {
    start: EpisodeStart,
    clock: CaptureClock,
    sender: Option<mpsc::SyncSender<WriterMessage>>,
    thread: Option<JoinHandle<Result<Vec<Sample>, String>>>,
    completed: Arc<Mutex<Option<FullEpisode>>>,
    shared: Arc<WriterShared>,
    last_queued_seconds: AtomicU64,
}

impl EpisodeWriter {
    pub fn clock(&self) -> CaptureClock {
        self.clock.clone()
    }
    fn start(
        start: EpisodeStart,
        completed: Arc<Mutex<Option<FullEpisode>>>,
    ) -> Result<Self, EpisodeError> {
        let (sender, receiver) = mpsc::sync_channel(SAMPLE_QUEUE_DEPTH);
        let shared = Arc::new(WriterShared {
            written: AtomicU64::new(0),
            failure: Mutex::new(None),
        });
        let worker_shared = shared.clone();
        let worker_completed = Arc::clone(&completed);
        let validation_start = start.clone();
        let thread = std::thread::Builder::new()
            .name("spiral-episode-encoder".to_owned())
            .spawn(move || {
                EpisodeDrain {
                    start: &validation_start,
                    completed: &worker_completed,
                    shared: &worker_shared,
                }
                .run(receiver)
            })
            .map_err(|source| {
                EpisodeError::Invalid(format!("could not start the episode encoder: {source}"))
            })?;
        Ok(Self {
            start,
            clock: CaptureClock::default(),
            sender: Some(sender),
            thread: Some(thread),
            completed,
            shared,
            last_queued_seconds: AtomicU64::new(NO_SAMPLE_TIMESTAMP_BITS),
        })
    }
    pub fn id(&self) -> &str {
        self.start.id()
    }
    fn fail(&self, message: &str) -> bool {
        let mut failure = self.shared.failure.lock_unpoisoned();
        if failure.is_none() {
            *failure = Some(message.to_owned());
        }
        false
    }
    fn finish(mut self, completion: EpisodeCompletion) -> Result<EpisodeInfo, EpisodeError> {
        self.sender.take();
        let samples = self
            .thread
            .take()
            .expect("episode worker exists")
            .join()
            .map_err(|_| EpisodeError::Invalid("episode encoder worker panicked".to_owned()))?
            .map_err(EpisodeError::Invalid)?;
        if let Some(error) = self.failure() {
            return Err(EpisodeError::Invalid(error));
        }
        let full_episode = FullEpisode {
            start: self.start.clone(),
            samples,
            completion,
        };
        *self.completed.lock_unpoisoned() = Some(full_episode.clone());
        Ok(full_episode.summary())
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
        let Some(sender) = &self.sender else {
            return false;
        };
        match sender.try_send(WriterMessage::Sample(sample)) {
            Ok(()) => true,
            Err(error) => self.fail(&format!(
                "Spiral recording queue could not accept a sample: {error}"
            )),
        }
    }
    fn checkpoint(&self, completion: RecordingCompletion) -> Result<EpisodeCheckpoint, String> {
        let (complete, receiver) = tokio::sync::oneshot::channel();
        self.sender
            .as_ref()
            .ok_or_else(|| "episode encoder is closed".to_owned())?
            .try_send(WriterMessage::Checkpoint {
                completion: EpisodeCompletion::from(&completion),
                complete,
            })
            .map_err(|error| error.to_string())?;
        Ok(receiver)
    }
    fn failure(&self) -> Option<String> {
        self.shared.failure.lock_unpoisoned().clone()
    }
    fn finish(
        self: Box<Self>,
        completion: RecordingCompletion,
    ) -> Result<CompletedRecording, String> {
        EpisodeWriter::finish(*self, EpisodeCompletion::from(&completion))
            .map(|summary| CompletedRecording::new(summary.id(), summary.frames()))
            .map_err(|error| error.to_string())
    }
}

impl Drop for EpisodeWriter {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct EpisodeDrain<'a> {
    start: &'a EpisodeStart,
    completed: &'a Mutex<Option<FullEpisode>>,
    shared: &'a WriterShared,
}

impl EpisodeDrain<'_> {
    fn run(self, receiver: mpsc::Receiver<WriterMessage>) -> Result<Vec<Sample>, String> {
        let mut samples = Vec::new();
        for message in receiver {
            match message {
                WriterMessage::Sample(sample) => {
                    if self.shared.written.load(Ordering::Relaxed) >= MAX_EPISODE_FRAMES {
                        let error = format!(
                            "recording reached the supported {MAX_EPISODE_FRAMES}-sample limit"
                        );
                        *self.shared.failure.lock_unpoisoned() = Some(error.clone());
                        return Err(error);
                    }
                    sample.validate_for(self.start).map_err(|error| {
                        let error = error.to_string();
                        *self.shared.failure.lock_unpoisoned() = Some(error.clone());
                        error
                    })?;
                    samples.push(sample);
                    self.shared.written.fetch_add(1, Ordering::Relaxed);
                }
                WriterMessage::Checkpoint {
                    completion,
                    complete,
                } => {
                    let full_episode = FullEpisode {
                        start: self.start.clone(),
                        samples: samples.clone(),
                        completion,
                    };
                    *self.completed.lock_unpoisoned() = Some(full_episode);
                    let _ = complete.send(Ok(()));
                }
            }
        }
        Ok(samples)
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
trait UnpoisonedMutex<T> {
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
    fn review_is_scoped_to_current_session_without_storage_fallback() {
        let buffer = RecordingBuffer::new();
        assert!(matches!(
            buffer.completed_episode("unknown-take"),
            Err(EpisodeError::NotFound(_))
        ));
        let arms = vec![EpisodeArm::new("arm", None, vec!["joint".into()])];
        for id in ["first", "second"] {
            let writer = EpisodeWriter::start(
                EpisodeStart::new(id.into(), "destination".into()).with_arms(arms.clone(), vec![]),
                buffer.completed.clone(),
            )
            .unwrap();
            assert!(writer.push(Sample::new(0.0, vec![vec![Some(0.25)]], vec![vec![123]])));
            writer
                .finish(EpisodeCompletion::new(0.1, vec![], vec![]))
                .unwrap();
            assert_eq!(buffer.completed_episode(id).unwrap().start.id(), id);
            assert_eq!(
                buffer
                    .inspection(id)
                    .unwrap()
                    .into_parts()
                    .0
                    .samples()
                    .len(),
                1
            );
        }
        assert!(matches!(
            buffer.completed_episode("first"),
            Err(EpisodeError::NotFound(_))
        ));
        assert!(buffer.completed_episode("second").is_ok());
        assert!(matches!(
            RecordingBuffer::new().completed_episode("second"),
            Err(EpisodeError::NotFound(_))
        ));
    }
}

#[cfg(test)]
mod recording_golden_tests {
    use super::*;
    use sha2::Digest as _;

    /// One deterministic take touching every recorded field and presence case.
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

    fn camera(stream: &str, camera_id: &str) -> crate::episodes::CameraRecording {
        let key: &[u8] = &[
            0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 0x67, 0x64, 0, 0x1e, 0, 0, 1, 0x68, 0x88, 0, 0, 1,
            0x65, 0x88,
        ];
        let delta: &[u8] = &[0, 0, 0, 1, 9, 0xf0, 0, 0, 1, 0x41, 0x88];
        crate::episodes::CameraRecording::from_annex_b(
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
    fn recording_messages_match_the_golden_take() {
        let cameras = [
            camera("follower.camera.wrist", "camera-1"),
            camera("leader.camera.top", "camera-2"),
        ];
        let bytes = crate::episodes::McapRecording::encode(&episode(), &cameras).unwrap();
        let rendering = messages(&bytes);
        let digest = sha2::Sha256::digest(rendering.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            digest,
            "af8f79ce60bbf5e62a10c8a3c1aed9a927faea1d1a224d096b2b4cdf8792c778"
        );
    }
}
