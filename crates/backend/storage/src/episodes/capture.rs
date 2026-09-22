//! Current-session recording samples; no persisted dataset reads.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use spiral_robotics_common::recording::{EpisodeArm, EpisodeInfo, RecordingGap};

use super::EpisodeId;
use spiral_robotics_hardware::{
    ArmConditionSummary as HardwareArmConditionSummary, CompletedRecording,
    MotorLimits as HardwareMotorLimits, RecordingCompletion, RecordingWriter,
};
pub use spiral_robotics_hardware::{EpisodeCheckpoint, Sample};

use super::episode_schema::SpiralEpisodeEncoder;

const SAMPLE_QUEUE_DEPTH: usize = 2_048;
/// Hard storage limit for a single recorded episode.
pub const MAX_EPISODE_FRAMES: u64 = 1_000_000;
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

/// The limits one motor reports for itself, captured with the take.
///
/// Storage owns this shape because it is persisted: the equivalent wire type
/// exposes only its temperature ceiling, so a stored row could never be read
/// back through it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct JointMotorLimits {
    max_temperature_celsius: Option<f64>,
    min_voltage_volts: Option<f64>,
    max_voltage_volts: Option<f64>,
    /// Configured torque ceiling as a fraction of the motor's rated torque.
    max_load: Option<f64>,
}

impl JointMotorLimits {
    const fn new(
        max_temperature_celsius: Option<f64>,
        voltage: JointVoltageRange,
        max_load: Option<f64>,
    ) -> Self {
        Self {
            max_temperature_celsius,
            min_voltage_volts: voltage.minimum,
            max_voltage_volts: voltage.maximum,
            max_load,
        }
    }

    pub(super) const fn max_temperature_celsius(self) -> Option<f64> {
        self.max_temperature_celsius
    }
    pub(super) const fn min_voltage_volts(self) -> Option<f64> {
        self.min_voltage_volts
    }
    pub(super) const fn max_voltage_volts(self) -> Option<f64> {
        self.max_voltage_volts
    }
    pub(super) const fn max_load(self) -> Option<f64> {
        self.max_load
    }
}

/// The lower and upper supply voltages one motor accepts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct JointVoltageRange {
    minimum: Option<f64>,
    maximum: Option<f64>,
}

impl JointVoltageRange {
    const fn new(minimum: Option<f64>, maximum: Option<f64>) -> Self {
        Self { minimum, maximum }
    }
}

impl From<&HardwareMotorLimits> for JointMotorLimits {
    fn from(limits: &HardwareMotorLimits) -> Self {
        Self::new(
            limits.max_temperature_celsius(),
            JointVoltageRange::new(limits.min_voltage_volts(), limits.max_voltage_volts()),
            limits.max_load(),
        )
    }
}

/// What one arm's motors were like as a take opened.
///
/// Every vector is parallel to the arm's declared joint order, so a value at
/// index `j` belongs to joint `j`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArmCondition {
    arm: String,
    temperature_celsius: Vec<Option<f64>>,
    voltage_volts: Vec<Option<f64>>,
    limits: Vec<JointMotorLimits>,
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

    pub fn with_limits(mut self, limits: Vec<JointMotorLimits>) -> Self {
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
    pub(super) fn limits(&self) -> &[JointMotorLimits] {
        &self.limits
    }

    /// True when nothing was actually measured, so storage writes null columns
    /// instead of inventing an all-empty observation.
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
    note: Option<String>,
}

impl EpisodeStart {
    pub(super) fn new(id: String, dataset_id: String) -> Self {
        Self {
            identity: EpisodeIdentity { id, dataset_id },
            recording: EpisodeRecordingMetadata {
                started_ms: 0,
                sample_hz: 0.0,
                source: String::new(),
                note: None,
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

    pub(super) fn with_source(mut self, source: String, note: Option<String>) -> Self {
        self.recording.source = source;
        self.recording.note = note;
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
    pub(super) fn note(&self) -> Option<&str> {
        self.recording.note.as_deref()
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
    fn new(arm: String) -> Self {
        Self {
            arm,
            peak_temperature_celsius: Vec::new(),
            min_voltage_volts: Vec::new(),
            peak_load: Vec::new(),
            peak_current_amps: Vec::new(),
        }
    }

    fn with_extremes(
        mut self,
        temperature: ArmConditionThermalExtremes,
        effort: ArmConditionEffortExtremes,
    ) -> Self {
        self.peak_temperature_celsius = temperature.peak_temperature_celsius;
        self.min_voltage_volts = temperature.min_voltage_volts;
        self.peak_load = effort.peak_load;
        self.peak_current_amps = effort.peak_current_amps;
        self
    }

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

/// Per-joint thermal and supply extremes for one arm.
pub(super) struct ArmConditionThermalExtremes {
    peak_temperature_celsius: Vec<Option<f64>>,
    min_voltage_volts: Vec<Option<f64>>,
}

impl ArmConditionThermalExtremes {
    const fn new(
        peak_temperature_celsius: Vec<Option<f64>>,
        min_voltage_volts: Vec<Option<f64>>,
    ) -> Self {
        Self {
            peak_temperature_celsius,
            min_voltage_volts,
        }
    }
}

/// Per-joint mechanical extremes for one arm.
pub(super) struct ArmConditionEffortExtremes {
    peak_load: Vec<Option<f64>>,
    peak_current_amps: Vec<Option<f64>>,
}

impl ArmConditionEffortExtremes {
    const fn new(peak_load: Vec<Option<f64>>, peak_current_amps: Vec<Option<f64>>) -> Self {
        Self {
            peak_load,
            peak_current_amps,
        }
    }
}

impl From<&HardwareArmConditionSummary> for ArmConditionSummary {
    fn from(summary: &HardwareArmConditionSummary) -> Self {
        Self::new(summary.arm().to_owned()).with_extremes(
            ArmConditionThermalExtremes::new(
                summary.peak_temperature_celsius().to_vec(),
                summary.min_voltage_volts().to_vec(),
            ),
            ArmConditionEffortExtremes::new(
                summary.peak_load().to_vec(),
                summary.peak_current_amps().to_vec(),
            ),
        )
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

    pub(super) const fn with_max_follow_error_degrees(
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
    #[cfg(test)]
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
        self.start
            .summary(&self.completion, self.samples.len() as u64)
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

    pub fn get(&self, id: &str) -> Result<EpisodeInfo, EpisodeError> {
        Ok(self.completed_episode(id)?.summary())
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
            .with_source(request.source, None)
            .with_arms(request.arms, request.condition);
        SpiralEpisodeEncoder::new(start.clone())?;
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

    pub fn forget_completed(&self, id: &str) {
        let mut completed = self.completed.lock_unpoisoned();
        if completed
            .as_ref()
            .is_some_and(|episode| episode.start.id() == id)
        {
            *completed = None;
        }
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

trait EpisodeWriteSession: Send {
    fn start(&self) -> &EpisodeStart;
    fn push(&self, sample: Sample) -> bool;
    fn checkpoint(&self, completion: EpisodeCompletion) -> Result<EpisodeCheckpoint, String>;
    fn failure(&self) -> Option<String>;
    fn finish(self: Box<Self>, completion: EpisodeCompletion) -> Result<EpisodeInfo, EpisodeError>;
}

pub struct EpisodeWriter {
    clock: spiral_robotics_hardware::CaptureClock,
    session: Box<dyn EpisodeWriteSession>,
}

impl EpisodeWriter {
    pub fn clock(&self) -> spiral_robotics_hardware::CaptureClock {
        self.clock.clone()
    }
    fn from_session(session: impl EpisodeWriteSession + 'static) -> Self {
        Self {
            session: Box::new(session),
            clock: Default::default(),
        }
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
        Ok(Self::from_session(SpiralEpisodeWriter {
            start,
            sender: Some(sender),
            thread: Some(thread),
            completed,
            shared,
            last_queued_seconds: AtomicU64::new(NO_SAMPLE_TIMESTAMP_BITS),
        }))
    }
    pub fn id(&self) -> &str {
        self.session.start().id()
    }
    fn dataset_id(&self) -> &str {
        self.session.start().dataset_id()
    }
    pub fn started_ms(&self) -> i64 {
        self.session.start().started_ms()
    }
    fn push(&self, sample: Sample) -> bool {
        self.session.push(sample)
    }
    fn checkpoint(&self, completion: EpisodeCompletion) -> Result<EpisodeCheckpoint, String> {
        self.session.checkpoint(completion)
    }
    fn failure(&self) -> Option<String> {
        self.session.failure()
    }
    fn finish(self, completion: EpisodeCompletion) -> Result<EpisodeInfo, EpisodeError> {
        self.session.finish(completion)
    }
}

impl RecordingWriter for EpisodeWriter {
    fn update_clock(&self, now: std::time::Instant, elapsed: Option<std::time::Duration>) {
        self.clock.update(now, elapsed);
    }
    fn id(&self) -> &str {
        EpisodeWriter::id(self)
    }

    fn dataset_id(&self) -> &str {
        EpisodeWriter::dataset_id(self)
    }

    fn push(&self, sample: Sample) -> bool {
        EpisodeWriter::push(self, sample)
    }

    fn checkpoint(&self, completion: RecordingCompletion) -> Result<EpisodeCheckpoint, String> {
        EpisodeWriter::checkpoint(
            self,
            EpisodeCompletion::new(
                completion.duration_seconds(),
                completion.recording_gaps().to_vec(),
                completion
                    .condition()
                    .iter()
                    .map(ArmConditionSummary::from)
                    .collect(),
            )
            .with_max_follow_error_degrees(completion.max_follow_error_degrees()),
        )
    }

    fn failure(&self) -> Option<String> {
        EpisodeWriter::failure(self)
    }

    fn finish(
        self: Box<Self>,
        completion: RecordingCompletion,
    ) -> Result<CompletedRecording, String> {
        EpisodeWriter::finish(
            *self,
            EpisodeCompletion::new(
                completion.duration_seconds(),
                completion.recording_gaps().to_vec(),
                completion
                    .condition()
                    .iter()
                    .map(ArmConditionSummary::from)
                    .collect(),
            )
            .with_max_follow_error_degrees(completion.max_follow_error_degrees())
            .with_discarded(completion.discarded()),
        )
        .map(|summary| CompletedRecording::new(summary.id(), summary.frames()))
        .map_err(|error| error.to_string())
    }
}

struct SpiralEpisodeWriter {
    start: EpisodeStart,
    sender: Option<mpsc::SyncSender<WriterMessage>>,
    thread: Option<JoinHandle<Result<Vec<Sample>, String>>>,
    completed: Arc<Mutex<Option<FullEpisode>>>,
    shared: Arc<WriterShared>,
    last_queued_seconds: AtomicU64,
}

impl EpisodeWriteSession for SpiralEpisodeWriter {
    fn start(&self) -> &EpisodeStart {
        &self.start
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
    fn checkpoint(&self, completion: EpisodeCompletion) -> Result<EpisodeCheckpoint, String> {
        let (complete, receiver) = tokio::sync::oneshot::channel();
        self.sender
            .as_ref()
            .ok_or_else(|| "episode encoder is closed".to_owned())?
            .try_send(WriterMessage::Checkpoint {
                completion,
                complete,
            })
            .map_err(|error| error.to_string())?;
        Ok(receiver)
    }
    fn failure(&self) -> Option<String> {
        self.shared.failure.lock_unpoisoned().clone()
    }
    fn finish(
        mut self: Box<Self>,
        completion: EpisodeCompletion,
    ) -> Result<EpisodeInfo, EpisodeError> {
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
        Ok(full_episode
            .start
            .summary(&full_episode.completion, full_episode.samples.len() as u64))
    }
}

impl SpiralEpisodeWriter {
    fn fail(&self, message: &str) -> bool {
        let mut failure = self.shared.failure.lock_unpoisoned();
        if failure.is_none() {
            *failure = Some(message.to_owned());
        }
        false
    }
}

impl Drop for SpiralEpisodeWriter {
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
        let mut encoder =
            SpiralEpisodeEncoder::new(self.start.clone()).map_err(|error| error.to_string())?;
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
                    encoder.encode(&sample).map_err(|error| {
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

impl EpisodeStart {
    fn summary(&self, completion: &EpisodeCompletion, frames: u64) -> EpisodeInfo {
        let start = self;
        EpisodeInfo::new(start.id(), start.dataset_id())
            .with_arms(start.arms().to_vec())
            .with_source(start.source())
            .with_timeline(frames, completion.duration_seconds())
            .with_sample_hz(start.sample_hz())
            .with_recording_gaps(completion.recording_gaps().to_vec())
            .with_started_ms(start.started_ms())
            .with_assessment(
                completion.discarded(),
                completion.max_follow_error_degrees(),
            )
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
            buffer.get("unknown-take"),
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
            assert_eq!(buffer.get(id).unwrap().id(), id);
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
            buffer.get("first"),
            Err(EpisodeError::NotFound(_))
        ));
        buffer.forget_completed("first");
        assert!(buffer.get("second").is_ok());
        buffer.forget_completed("second");
        assert!(matches!(
            buffer.get("second"),
            Err(EpisodeError::NotFound(_))
        ));
        assert!(matches!(
            RecordingBuffer::new().get("second"),
            Err(EpisodeError::NotFound(_))
        ));
    }
}
