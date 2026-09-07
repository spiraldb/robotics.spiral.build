//! Episode capture and inspection backed directly by SpiralDB.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::{Table, column as db};
use futures::future;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use spiral_robotics_common::recording::{EpisodeArm, EpisodeInfo, EpisodeListItem, RecordingGap};
use spiral_robotics_common::sql::QualifiedRelation;

use super::EpisodeId;
use spiral_robotics_hardware::Calibration;
use spiral_robotics_hardware::{
    ArmConditionSummary as HardwareArmConditionSummary, CompletedRecording,
    MotorLimits as HardwareMotorLimits, RecordingCompletion, RecordingWriter,
};
pub use spiral_robotics_hardware::{EpisodeCheckpoint, Sample};

use super::episode_schema::{
    EpisodeStreamCatalog, EpisodeStreamRow, JointSampleRows, SpiralEpisodeEncoder,
    StoredEpisodeRow, VideoStreamProperties, VideoStreamRow, VideoStreamRows,
};
use super::video::{VideoAsset, VideoAssetMetadata, VideoSegment, VideoSize, VideoTiming};
use crate::db::SpiralSql;
use crate::db::{ProjectConnection, ProjectStore};

const SAMPLE_QUEUE_DEPTH: usize = 2_048;
/// Hard storage limit for a single recorded episode.
pub const MAX_EPISODE_FRAMES: u64 = 1_000_000;
const MAX_EPISODE_ROWS: usize = MAX_EPISODE_FRAMES as usize * 16;
const NO_SAMPLE_TIMESTAMP_BITS: u64 = u64::MAX;
const JOINT_BATCH_ROWS: usize = 512;

type NormalizedPose = BTreeMap<String, f64>;
pub type TimelineJointValues = BTreeMap<String, Option<f64>>;

/// Every owned value of one arm at a timeline sample, in declaration order.
pub type TimelineArmSampleParts = (
    String,
    TimelineJointValues,
    BTreeMap<String, i64>,
    Option<TimelineJointValues>,
    Option<TimelineJointValues>,
    Option<TimelineJointValues>,
);

#[derive(Debug)]
pub struct ReplayTrajectory {
    frames: Vec<NormalizedPose>,
    start_frame: usize,
    resolved_start_seconds: f64,
}

impl ReplayTrajectory {
    pub fn into_frames(self) -> Vec<NormalizedPose> {
        self.frames
    }

    pub const fn start_frame(&self) -> usize {
        self.start_frame
    }

    pub const fn resolved_start_seconds(&self) -> f64 {
        self.resolved_start_seconds
    }
}

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
    pub(super) const fn new(
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
    pub(super) const fn new(minimum: Option<f64>, maximum: Option<f64>) -> Self {
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

    pub(super) fn with_current_timing(self, sample_hz: f64) -> Self {
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
    note: Option<String>,
    sample_hz: f64,
    arms: Vec<EpisodeArm>,
    condition: Vec<ArmCondition>,
}

impl EpisodeRecordingRequest {
    pub fn new(dataset_id: String, source: String, sample_hz: f64) -> Self {
        Self {
            dataset_id,
            source,
            note: None,
            sample_hz,
            arms: Vec::new(),
            condition: Vec::new(),
        }
    }

    pub fn with_note(mut self, note: Option<String>) -> Self {
        self.note = note;
        self
    }

    pub fn with_arms(mut self, arms: Vec<EpisodeArm>, condition: Vec<ArmCondition>) -> Self {
        self.arms = arms;
        self.condition = condition;
        self
    }
}

pub struct RobotPlaybackRequest<'a> {
    arm: &'a str,
    seconds: f64,
}

impl<'a> RobotPlaybackRequest<'a> {
    pub fn new(arm: &'a str, seconds: f64) -> Self {
        Self { arm, seconds }
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
    pub(super) fn new(arm: String) -> Self {
        Self {
            arm,
            peak_temperature_celsius: Vec::new(),
            min_voltage_volts: Vec::new(),
            peak_load: Vec::new(),
            peak_current_amps: Vec::new(),
        }
    }

    pub(super) fn with_extremes(
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
    pub(super) const fn new(
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
    pub(super) const fn new(
        peak_load: Vec<Option<f64>>,
        peak_current_amps: Vec<Option<f64>>,
    ) -> Self {
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

    pub(super) const fn with_discarded(mut self, discarded: bool) -> Self {
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
    pub(super) fn fixture_without_samples(
        start: EpisodeStart,
        completion: EpisodeCompletion,
    ) -> Self {
        Self {
            start,
            samples: Vec::new(),
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
    videos: Vec<VideoAssetMetadata>,
}

impl EpisodeInspection {
    pub fn into_parts(self) -> (FullEpisode, EpisodeInfo, Vec<VideoAssetMetadata>) {
        (self.episode, self.summary, self.videos)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReappliedEpisodePositions {
    updated_episode_ids: Vec<String>,
    frames_updated: u64,
    episodes_unchanged: u64,
    episodes_skipped: u64,
    warnings: Vec<String>,
}

impl ReappliedEpisodePositions {
    pub fn into_parts(self) -> (Vec<String>, u64, u64, u64, Vec<String>) {
        (
            self.updated_episode_ids,
            self.frames_updated,
            self.episodes_unchanged,
            self.episodes_skipped,
            self.warnings,
        )
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        updated_episode_ids: Vec<String>,
        frames_updated: u64,
        episodes_unchanged: u64,
    ) -> Self {
        Self {
            updated_episode_ids,
            frames_updated,
            episodes_unchanged,
            episodes_skipped: 0,
            warnings: Vec::new(),
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_skipped(mut self, episodes_skipped: u64, warnings: Vec<String>) -> Self {
        self.episodes_skipped = episodes_skipped;
        self.warnings = warnings;
        self
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TimelineArmSample {
    arm: String,
    normalized: TimelineJointValues,
    raw: BTreeMap<String, i64>,
    velocity: Option<TimelineJointValues>,
    effort: Option<TimelineJointValues>,
    action: Option<TimelineJointValues>,
}

impl TimelineArmSample {
    pub fn into_parts(self) -> TimelineArmSampleParts {
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TimelineSample {
    frame: u64,
    seconds: f64,
    follow_error_degrees: Option<f64>,
    arms: Vec<TimelineArmSample>,
}

impl TimelineSample {
    pub fn into_parts(self) -> (u64, f64, Option<f64>, Vec<TimelineArmSample>) {
        (
            self.frame,
            self.seconds,
            self.follow_error_degrees,
            self.arms,
        )
    }
}

pub struct EpisodeStore {
    project_store: Arc<ProjectStore>,
    completed: Arc<Mutex<BTreeMap<String, FullEpisode>>>,
    /// What the last catalog read could not do.
    catalog_issues: Mutex<Vec<String>>,
    seek_sequence: AtomicU64,
    latest_seek: Mutex<BTreeMap<String, u64>>,
    seek_gate: tokio::sync::Mutex<()>,
}

impl EpisodeStore {
    pub fn new(project_store: Arc<ProjectStore>) -> Self {
        Self {
            project_store,
            completed: Arc::new(Mutex::new(BTreeMap::new())),
            catalog_issues: Mutex::new(Vec::new()),
            seek_sequence: AtomicU64::new(0),
            latest_seek: Mutex::new(BTreeMap::new()),
            seek_gate: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn list(&self) -> Vec<EpisodeInfo> {
        self.list_selected(EpisodeCatalogSelection::All).await
    }

    pub async fn list_in_dataset(&self, dataset_id: &str) -> Vec<EpisodeListItem> {
        match self
            .read_catalog(EpisodeCatalogSelection::Dataset(dataset_id))
            .await
        {
            Ok(read) => {
                *self.catalog_issues.lock_unpoisoned() = read.warnings.clone();
                read.list_items()
            }
            Err(error) => {
                *self.catalog_issues.lock_unpoisoned() = vec![error.to_string()];
                Vec::new()
            }
        }
    }

    async fn list_selected(&self, selection: EpisodeCatalogSelection<'_>) -> Vec<EpisodeInfo> {
        match self.read_catalog(selection).await {
            Ok(read) => {
                *self.catalog_issues.lock_unpoisoned() = read.warnings.clone();
                read.summaries()
            }
            Err(error) => {
                *self.catalog_issues.lock_unpoisoned() = vec![error.to_string()];
                Vec::new()
            }
        }
    }
    pub fn issues(&self) -> Vec<String> {
        self.catalog_issues.lock_unpoisoned().clone()
    }
    pub async fn get(&self, id: &str) -> Result<EpisodeInfo, EpisodeError> {
        EpisodeId::try_from(id).map_err(|_| EpisodeError::InvalidId(id.to_owned()))?;
        self.try_list()
            .await?
            .into_iter()
            .find(|episode| episode.id() == id)
            .ok_or_else(|| EpisodeError::NotFound(id.to_owned()))
    }

    async fn full_episode(&self, id: &str) -> Result<FullEpisode, EpisodeError> {
        EpisodeId::try_from(id).map_err(|_| EpisodeError::InvalidId(id.to_owned()))?;
        self.load_episode(id, EpisodeCatalogSelection::Episode(id))
            .await
    }

    async fn load_episode(
        &self,
        id: &str,
        selection: EpisodeCatalogSelection<'_>,
    ) -> Result<FullEpisode, EpisodeError> {
        let (read, rows) =
            future::try_join(self.read_catalog(selection), self.read_sample_rows(id)).await?;
        let (start, completion, _) = read
            .episodes
            .into_iter()
            .next()
            .ok_or_else(|| EpisodeError::NotFound(id.to_owned()))?
            .into_parts();
        let decoded = start.decode_samples(rows).map_err(EpisodeError::Invalid)?;
        Ok(FullEpisode {
            start,
            samples: decoded,
            completion,
        })
    }

    async fn read_sample_rows(
        &self,
        id: &str,
    ) -> Result<Vec<JsonMap<String, JsonValue>>, EpisodeError> {
        let sample_query = SpiralSql::episode_samples(self.project_store.namespace(), id);
        let mut connection = self
            .project_store
            .connect()
            .await
            .map_err(EpisodeError::Invalid)?;
        let result = connection
            .query_rows(&sample_query, MAX_EPISODE_ROWS)
            .await
            .map_err(EpisodeError::Invalid)?;
        if result.truncated() {
            return Err(EpisodeError::Invalid(format!(
                "episode {id:?} exceeds the supported row limit"
            )));
        }
        Ok(result.into_rows())
    }

    pub async fn inspection(&self, id: &str) -> Result<EpisodeInspection, EpisodeError> {
        EpisodeId::try_from(id).map_err(|_| EpisodeError::InvalidId(id.to_owned()))?;
        let (episode, videos) = future::try_join(
            self.load_episode(id, EpisodeCatalogSelection::Inspection(id)),
            self.video_metadata_for_existing(id),
        )
        .await?;
        let summary = episode
            .start
            .summary(&episode.completion, episode.samples.len() as u64);
        Ok(EpisodeInspection {
            episode,
            summary,
            videos,
        })
    }

    async fn next_id(&self) -> Result<String, EpisodeError> {
        let highest = self
            .try_list()
            .await?
            .iter()
            .filter_map(|episode| {
                EpisodeId::try_from(episode.id())
                    .ok()
                    .and_then(|id| id.sequence_number())
            })
            .max()
            .unwrap_or(0);
        Ok(format!("episode_{:06}", highest + 1))
    }

    pub async fn begin_in_dataset(
        &self,
        request: EpisodeRecordingRequest,
    ) -> Result<EpisodeWriter, EpisodeError> {
        if request.arms.is_empty() {
            return Err(EpisodeError::Invalid(
                "a take needs at least one connected arm to sample".to_owned(),
            ));
        }
        let start = EpisodeStart::new(self.next_id().await?, request.dataset_id)
            .with_current_timing(request.sample_hz)
            .with_source(request.source, request.note)
            .with_arms(request.arms, request.condition);
        SpiralEpisodeEncoder::new(start.clone())?;
        EpisodeWriter::start(start, Arc::clone(&self.completed))
    }

    pub async fn delete(&self, id: &str) -> Result<(), EpisodeError> {
        self.get(id).await?;
        let id = id.to_owned();
        let namespace = self.project_store.namespace().to_owned();
        let mut connection = self
            .project_store
            .connect_exclusive()
            .await
            .map_err(EpisodeError::Invalid)?;
        connection
            .clear_episode_rows(&namespace, &id)
            .await
            .map_err(EpisodeError::Invalid)
    }

    pub async fn reapply_calibration(
        &self,
        robot: &str,
        calibration: &Calibration,
    ) -> Result<ReappliedEpisodePositions, EpisodeError> {
        let mut result = ReappliedEpisodePositions::default();
        for summary in self.try_list().await? {
            let Some(arm_index) = summary.arms().iter().position(|arm| arm.id() == robot) else {
                continue;
            };
            let joints = summary.arms()[arm_index].joints();
            let missing = joints
                .iter()
                .filter(|joint| !calibration.motors().contains_key(*joint))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                result.episodes_skipped += 1;
                result.warnings.push(format!(
                    "take {:?} uses joints absent from the current calibration: {}",
                    summary.id(),
                    missing.join(", ")
                ));
                continue;
            }
            let mut episode = self.full_episode(summary.id()).await?;
            let mut changed = false;
            for sample in &mut episode.samples {
                let normalized = joints.iter().enumerate().map(|(joint_index, joint)| {
                    calibration.motors()[joint].normalize_raw(sample.raw()[arm_index][joint_index]).map(Some).map_err(|error| EpisodeError::Invalid(format!("could not reapply calibration to take {:?} joint {joint:?}: {error}", summary.id())))
                }).collect::<Result<Vec<_>, _>>()?;
                if sample.normalized()[arm_index] != normalized {
                    sample
                        .replace_normalized_arm(arm_index, normalized)
                        .map_err(|_| {
                            EpisodeError::Invalid("calibration arm index disappeared".to_owned())
                        })?;
                    changed = true;
                }
            }
            if changed {
                self.replace_samples(&episode).await?;
                result.frames_updated = result.frames_updated.saturating_add(summary.frames());
                result.updated_episode_ids.push(summary.id().to_owned());
            } else {
                result.episodes_unchanged += 1;
            }
        }
        Ok(result)
    }

    pub async fn trajectory_at(
        &self,
        id: &str,
        request: RobotPlaybackRequest<'_>,
    ) -> Result<ReplayTrajectory, EpisodeError> {
        let arm = request.arm;
        let seconds = request.seconds;
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(EpisodeError::Invalid(
                "replay start time must be finite and non-negative".to_owned(),
            ));
        }
        let episode = self.full_episode(id).await?;
        let arm_index = episode
            .start
            .arms
            .iter()
            .position(|entry| entry.id() == arm)
            .ok_or_else(|| {
                EpisodeError::Invalid(format!("take {id:?} did not record arm {arm:?}"))
            })?;
        let joints = episode.start.arms()[arm_index].joints();
        let timestamped = episode
            .samples
            .iter()
            .filter_map(|sample| {
                let values = &sample.normalized()[arm_index];
                if values.iter().any(Option::is_none) {
                    return None;
                }
                Some((
                    sample.seconds(),
                    joints
                        .iter()
                        .enumerate()
                        .map(|(index, joint)| {
                            (joint.clone(), values[index].expect("complete pose"))
                        })
                        .collect::<BTreeMap<_, _>>(),
                ))
            })
            .collect::<Vec<_>>();
        if timestamped.is_empty() {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} has no calibrated positions for {arm:?}"
            )));
        }
        let start_frame = timestamped
            .iter()
            .enumerate()
            .min_by(|(left_index, (left, _)), (right_index, (right, _))| {
                (seconds - left)
                    .abs()
                    .total_cmp(&(seconds - right).abs())
                    .then_with(|| left_index.cmp(right_index))
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        let resolved_start_seconds = timestamped[start_frame].0;
        Ok(ReplayTrajectory {
            frames: timestamped.into_iter().map(|(_, pose)| pose).collect(),
            start_frame,
            resolved_start_seconds,
        })
    }

    pub async fn sample_at(&self, id: &str, seconds: f64) -> Result<TimelineSample, EpisodeError> {
        if !seconds.is_finite() {
            return Err(EpisodeError::Invalid(
                "timeline time must be finite".to_owned(),
            ));
        }
        let episode = self.full_episode(id).await?;
        let (frame, sample) = episode
            .samples
            .iter()
            .enumerate()
            .min_by(|(left_index, left), (right_index, right)| {
                (seconds - left.seconds())
                    .abs()
                    .total_cmp(&(seconds - right.seconds()).abs())
                    .then_with(|| left_index.cmp(right_index))
            })
            .ok_or_else(|| EpisodeError::Invalid(format!("take {id:?} has no samples")))?;
        Ok(episode.start.timeline_sample(sample, frame as u64))
    }

    pub fn reserve_seek(&self, robot: &str) -> u64 {
        let ticket = self.seek_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.latest_seek
            .lock_unpoisoned()
            .insert(robot.to_owned(), ticket);
        ticket
    }
    pub fn seek_is_current(&self, robot: &str, ticket: u64) -> bool {
        self.latest_seek
            .lock_unpoisoned()
            .get(robot)
            .is_some_and(|latest| *latest == ticket)
    }
    pub async fn lock_seek(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.seek_gate.lock().await
    }

    async fn video_metadata_for_existing(
        &self,
        id: &str,
    ) -> Result<Vec<VideoAssetMetadata>, EpisodeError> {
        let relation = Table::VideoAssets.qualified_name(self.project_store.namespace());
        let id = SpiralSql::string_literal(id);
        let mut connection = self
            .project_store
            .connect()
            .await
            .map_err(EpisodeError::Invalid)?;
        let result = connection.query_rows(&format!("SELECT {}, {}, {}, {}, {}, {}, {}, {} FROM {relation} WHERE {} = {id} ORDER BY {}, {}", db::STREAM_ID, db::CAMERA_ID, db::WIDTH, db::HEIGHT, db::FPS, db::FRAMES, db::SEGMENT_INDEX, db::TIMELINE_OFFSET_NS, db::EPISODE_ID, db::STREAM_ID, db::SEGMENT_INDEX), 10_000).await.map_err(EpisodeError::Invalid)?;
        result
            .into_rows()
            .into_iter()
            .map(|row| row.video_metadata().map_err(EpisodeError::Invalid))
            .collect()
    }

    async fn try_list(&self) -> Result<Vec<EpisodeInfo>, EpisodeError> {
        self.read_catalog(EpisodeCatalogSelection::All)
            .await
            .map(EpisodeCatalogRead::summaries)
    }

    /// Read every episode without modifying persisted data.
    async fn read_catalog(
        &self,
        selection: EpisodeCatalogSelection<'_>,
    ) -> Result<EpisodeCatalogRead, EpisodeError> {
        EpisodeCatalogRead::load(&self.project_store, selection).await
    }

    async fn replace_samples(&self, episode: &FullEpisode) -> Result<(), EpisodeError> {
        let episode = episode.clone();
        let namespace = self.project_store.namespace().to_owned();
        let mut connection = self
            .project_store
            .connect_exclusive()
            .await
            .map_err(EpisodeError::Invalid)?;
        let sample_relation = Table::JointSamples.qualified_name(&namespace);
        let episode_relation = Table::Episodes.qualified_name(&namespace);
        connection
            .execute_update_sql(&SpiralSql::delete_episode(
                &sample_relation,
                episode.start.id(),
            ))
            .await
            .map_err(EpisodeError::Invalid)?;
        connection
            .execute_update_sql(&SpiralSql::delete_episode(
                &episode_relation,
                episode.start.id(),
            ))
            .await
            .map_err(EpisodeError::Invalid)?;
        let mut encoder = SpiralEpisodeEncoder::new(episode.start.clone())?;
        let mut rows = Vec::new();
        for sample in &episode.samples {
            rows.extend(encoder.encode(sample)?);
            if rows.len() >= 512 {
                connection
                    .ingest_batch(&sample_relation, rows.batch()?)
                    .await
                    .map_err(EpisodeError::Invalid)?;
                rows.clear();
            }
        }
        if !rows.is_empty() {
            connection
                .ingest_batch(&sample_relation, rows.batch()?)
                .await
                .map_err(EpisodeError::Invalid)?;
        }
        connection
            .ingest_batch(
                &episode_relation,
                episode
                    .completion
                    .episode_batch(&episode.start, encoder.frames())
                    .map_err(|error| EpisodeError::Invalid(error.to_string()))?,
            )
            .await
            .map_err(EpisodeError::Invalid)
    }

    pub fn completed_episode(&self, id: &str) -> Result<FullEpisode, EpisodeError> {
        self.completed
            .lock_unpoisoned()
            .get(id)
            .cloned()
            .ok_or_else(|| EpisodeError::NotFound(id.to_owned()))
    }

    pub fn forget_completed(&self, id: &str) {
        self.completed.lock_unpoisoned().remove(id);
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
    session: Box<dyn EpisodeWriteSession>,
}

impl EpisodeWriter {
    fn from_session(session: impl EpisodeWriteSession + 'static) -> Self {
        Self {
            session: Box::new(session),
        }
    }
    fn start(
        start: EpisodeStart,
        completed: Arc<Mutex<BTreeMap<String, FullEpisode>>>,
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
    completed: Arc<Mutex<BTreeMap<String, FullEpisode>>>,
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
        self.completed
            .lock_unpoisoned()
            .insert(full_episode.start.id().to_owned(), full_episode.clone());
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
    completed: &'a Mutex<BTreeMap<String, FullEpisode>>,
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
                    self.completed
                        .lock_unpoisoned()
                        .insert(self.start.id().to_owned(), full_episode);
                    let _ = complete.send(Ok(()));
                }
            }
        }
        Ok(samples)
    }
}

#[derive(Default)]
struct SampleBuilder {
    seconds: Option<f64>,
    normalized: Vec<Option<Vec<Option<f64>>>>,
    raw: Vec<Option<Vec<i64>>>,
    velocity: Vec<Option<Vec<Option<f64>>>>,
    effort: Vec<Option<Vec<Option<f64>>>>,
    action: Vec<Option<Vec<Option<f64>>>>,
    follow_error: Option<f64>,
}

impl EpisodeStart {
    fn decode_samples(&self, rows: Vec<JsonMap<String, JsonValue>>) -> Result<Vec<Sample>, String> {
        let start = self;
        let mut frames = BTreeMap::<u64, SampleBuilder>::new();
        for row in rows {
            let index = row.u64(db::SAMPLE_INDEX)?;
            let stream = row.string(db::STREAM_ID)?;
            let arm = start
                .arms
                .iter()
                .position(|arm| arm.id() == stream)
                .ok_or_else(|| {
                    format!(
                        "episode {:?} has sample for unknown stream {stream:?}",
                        start.id()
                    )
                })?;
            let frame = frames.entry(index).or_insert_with(|| SampleBuilder {
                normalized: vec![None; start.arms().len()],
                raw: vec![None; start.arms().len()],
                velocity: vec![None; start.arms().len()],
                effort: vec![None; start.arms().len()],
                action: vec![None; start.arms().len()],
                ..SampleBuilder::default()
            });
            let seconds = row.u64(db::EPISODE_TIME_NS)? as f64 / 1_000_000_000.0;
            if frame
                .seconds
                .is_some_and(|existing| existing.to_bits() != seconds.to_bits())
            {
                return Err(format!(
                    "episode {:?} sample {index} has inconsistent clocks",
                    start.id()
                ));
            }
            frame.seconds = Some(seconds);
            frame.normalized[arm] = Some(row.optional_f64s(db::STATE_POSITION)?);
            frame.raw[arm] = Some(row.i64s(db::RAW_POSITION)?);
            frame.velocity[arm] = row.optional_optional_f64s(db::VELOCITY)?;
            frame.effort[arm] = row.optional_optional_f64s(db::EFFORT)?;
            frame.action[arm] = row.optional_optional_f64s(db::ACTION_POSITION)?;
            if let Some(error) = row
                .get(db::FOLLOW_ERROR_DEGREES)
                .and_then(JsonValue::as_f64)
            {
                frame.follow_error = Some(
                    frame
                        .follow_error
                        .map_or(error, |current| current.max(error)),
                );
            }
        }
        frames
            .into_iter()
            .map(|(index, frame)| {
                let location = SampleLocation {
                    episode_id: start.id(),
                    sample_index: index,
                };
                let normalized = location.require(frame.normalized, "state")?;
                let raw = location.require(frame.raw, "raw state")?;
                let velocity = location.all_or_none(frame.velocity, "velocity")?;
                let effort = location.all_or_none(frame.effort, "effort")?;
                Ok(Sample::new(
                    frame.seconds.ok_or_else(|| {
                        format!("episode {:?} sample {index} has no clock", start.id())
                    })?,
                    normalized,
                    raw,
                )
                .with_velocity(velocity)
                .with_effort(effort)
                .with_action(frame.action)
                .with_follow_error_degrees(frame.follow_error))
            })
            .collect()
    }

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

    fn timeline_sample(&self, sample: &Sample, frame: u64) -> TimelineSample {
        let start = self;
        let arms = start
            .arms
            .iter()
            .enumerate()
            .map(|(arm_index, arm)| {
                let values = |values: Option<&[Vec<Option<f64>>]>| {
                    values.map(|values| {
                        arm.joints()
                            .iter()
                            .enumerate()
                            .map(|(joint_index, joint)| {
                                (joint.clone(), values[arm_index][joint_index])
                            })
                            .collect()
                    })
                };
                TimelineArmSample {
                    arm: arm.id().to_owned(),
                    normalized: arm
                        .joints()
                        .iter()
                        .enumerate()
                        .map(|(joint_index, joint)| {
                            (joint.clone(), sample.normalized()[arm_index][joint_index])
                        })
                        .collect(),
                    raw: arm
                        .joints()
                        .iter()
                        .enumerate()
                        .map(|(joint_index, joint)| {
                            (joint.clone(), sample.raw()[arm_index][joint_index])
                        })
                        .collect(),
                    velocity: values(sample.velocity()),
                    effort: values(sample.effort()),
                    action: sample
                        .action()
                        .get(arm_index)
                        .and_then(Option::as_ref)
                        .map(|values| {
                            arm.joints()
                                .iter()
                                .enumerate()
                                .map(|(joint_index, joint)| (joint.clone(), values[joint_index]))
                                .collect()
                        }),
                }
            })
            .collect();
        TimelineSample {
            frame,
            seconds: sample.seconds(),
            follow_error_degrees: sample.follow_error_degrees(),
            arms,
        }
    }
}

#[derive(Clone, Copy)]
struct SampleLocation<'a> {
    episode_id: &'a str,
    sample_index: u64,
}

impl SampleLocation<'_> {
    fn require<T>(self, values: Vec<Option<T>>, label: &str) -> Result<Vec<T>, String> {
        values
            .into_iter()
            .enumerate()
            .map(|(arm, value)| {
                value.ok_or_else(|| {
                    format!(
                        "episode {:?} sample {} arm {arm} has no {label}",
                        self.episode_id, self.sample_index
                    )
                })
            })
            .collect()
    }

    fn all_or_none<T>(self, values: Vec<Option<T>>, label: &str) -> Result<Option<Vec<T>>, String> {
        if values.iter().all(Option::is_none) {
            return Ok(None);
        }
        self.require(values, label).map(Some)
    }
}

/// One read of the persisted episode relations.
///
/// Episodes and their stream declarations are read together because an
/// episode's arms live in the stream relation: neither half is a complete
/// record on its own.
#[derive(Clone, Copy)]
enum EpisodeCatalogSelection<'a> {
    All,
    Dataset(&'a str),
    Episode(&'a str),
    Inspection(&'a str),
}

impl EpisodeCatalogSelection<'_> {
    fn episodes_sql(self, namespace: &str) -> String {
        let relation = Table::Episodes.qualified_name(namespace);
        match self {
            Self::All => format!("SELECT * FROM {relation}"),
            Self::Dataset(dataset_id) => format!(
                "SELECT * FROM {relation} WHERE {} = {}",
                db::DATASET_ID,
                SpiralSql::string_literal(dataset_id)
            ),
            Self::Episode(episode_id) | Self::Inspection(episode_id) => format!(
                "SELECT * FROM {relation} WHERE {} = {}",
                db::EPISODE_ID,
                SpiralSql::string_literal(episode_id)
            ),
        }
    }

    fn streams_sql(self, namespace: &str) -> String {
        let streams = Table::EpisodeStreams.qualified_name(namespace);
        let projection = format!(
            "s.{0} AS {0}, s.{1} AS {1}, s.{2} AS {2}, s.{3} AS {3}, s.{4} AS {4}, s.{5} AS {5}",
            db::EPISODE_ID,
            db::STREAM_ID,
            db::ORDINAL,
            db::MODALITY,
            db::ROBOT_TYPE,
            db::JOINT_NAMES,
        );
        match self {
            Self::All => format!(
                "SELECT {projection} FROM {streams} AS s WHERE s.{} = 'joint'",
                db::MODALITY
            ),
            Self::Dataset(_) => unreachable!("dataset listings do not read episode streams"),
            Self::Episode(episode_id) => format!(
                "SELECT * FROM {streams} WHERE {} = {}",
                db::EPISODE_ID,
                SpiralSql::string_literal(episode_id)
            ),
            Self::Inspection(episode_id) => format!(
                "SELECT {projection} FROM {streams} AS s WHERE s.{} = {} AND s.{} = 'joint'",
                db::EPISODE_ID,
                SpiralSql::string_literal(episode_id),
                db::MODALITY,
            ),
        }
    }

    fn decode_stream(
        self,
        batch: &arrow_array::RecordBatch,
        row: usize,
    ) -> Result<EpisodeStreamRow, String> {
        match self {
            Self::Episode(_) => EpisodeStreamRow::decode(batch, row),
            Self::All | Self::Dataset(_) | Self::Inspection(_) => {
                EpisodeStreamRow::decode_catalog(batch, row)
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct EpisodeCatalogRead {
    episodes: Vec<StoredEpisodeRow>,
    warnings: Vec<String>,
}

impl EpisodeCatalogRead {
    async fn load(
        project_store: &ProjectStore,
        selection: EpisodeCatalogSelection<'_>,
    ) -> Result<Self, EpisodeError> {
        let namespace = project_store.namespace();
        let streams = async {
            if matches!(selection, EpisodeCatalogSelection::Dataset(_)) {
                return Ok(EpisodeStreamCatalog::default());
            }
            let mut connection = project_store
                .connect()
                .await
                .map_err(EpisodeError::Invalid)?;
            Self::load_streams(&mut connection, namespace, selection).await
        };
        let episode_batches = async {
            let mut connection = project_store
                .connect()
                .await
                .map_err(EpisodeError::Invalid)?;
            connection
                .read_batches(&selection.episodes_sql(namespace))
                .await
                .map_err(EpisodeError::Invalid)
        };
        let (streams, episode_batches) = future::try_join(streams, episode_batches).await?;
        Self::decode(episode_batches, streams)
    }

    fn decode(
        episode_batches: Vec<arrow_array::RecordBatch>,
        streams: EpisodeStreamCatalog,
    ) -> Result<Self, EpisodeError> {
        let mut rows = Vec::new();
        for batch in episode_batches {
            for row in 0..batch.num_rows() {
                rows.push(
                    StoredEpisodeRow::decode(&batch, row, &streams)
                        .map_err(EpisodeError::Invalid)?,
                );
            }
        }
        rows.sort_by(|left, right| {
            left.start()
                .started_ms()
                .cmp(&right.start().started_ms())
                .then_with(|| left.start().id().cmp(right.start().id()))
        });

        Ok(Self {
            episodes: rows,
            warnings: Vec::new(),
        })
    }

    async fn load_streams(
        connection: &mut ProjectConnection,
        namespace: &str,
        selection: EpisodeCatalogSelection<'_>,
    ) -> Result<EpisodeStreamCatalog, EpisodeError> {
        let mut rows = Vec::new();
        for batch in connection
            .read_batches(&selection.streams_sql(namespace))
            .await
            .map_err(EpisodeError::Invalid)?
        {
            for row in 0..batch.num_rows() {
                rows.push(
                    selection
                        .decode_stream(&batch, row)
                        .map_err(EpisodeError::Invalid)?,
                );
            }
        }
        Ok(EpisodeStreamCatalog::new(rows))
    }

    fn summaries(self) -> Vec<EpisodeInfo> {
        self.episodes
            .iter()
            .map(|row| row.start().summary(row.completion(), row.frames()))
            .collect()
    }

    fn list_items(self) -> Vec<EpisodeListItem> {
        self.episodes
            .iter()
            .map(|row| EpisodeListItem::from(&row.start().summary(row.completion(), row.frames())))
            .collect()
    }
}

trait EpisodeJsonRow {
    fn string(&self, name: &str) -> Result<&str, String>;
    fn u64(&self, name: &str) -> Result<u64, String>;
    fn i64s(&self, name: &str) -> Result<Vec<i64>, String>;
    fn optional_f64s(&self, name: &str) -> Result<Vec<Option<f64>>, String>;
    fn optional_optional_f64s(&self, name: &str) -> Result<Option<Vec<Option<f64>>>, String>;
    fn video_metadata(&self) -> Result<VideoAssetMetadata, String>;
}

impl EpisodeJsonRow for JsonMap<String, JsonValue> {
    fn string(&self, name: &str) -> Result<&str, String> {
        self.get(name)
            .and_then(JsonValue::as_str)
            .ok_or_else(|| format!("row has no string {name}"))
    }

    fn u64(&self, name: &str) -> Result<u64, String> {
        self.get(name)
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| format!("row has no unsigned {name}"))
    }

    fn i64s(&self, name: &str) -> Result<Vec<i64>, String> {
        self.get(name)
            .and_then(JsonValue::as_array)
            .ok_or_else(|| format!("row has no array {name}"))?
            .iter()
            .map(|value| {
                value
                    .as_i64()
                    .ok_or_else(|| format!("{name} contains a non-integer"))
            })
            .collect()
    }

    fn optional_f64s(&self, name: &str) -> Result<Vec<Option<f64>>, String> {
        self.get(name)
            .and_then(JsonValue::as_array)
            .ok_or_else(|| format!("row has no array {name}"))?
            .iter()
            .map(|value| {
                if value.is_null() {
                    Ok(None)
                } else {
                    value
                        .as_f64()
                        .map(Some)
                        .ok_or_else(|| format!("{name} contains a non-number"))
                }
            })
            .collect()
    }

    fn optional_optional_f64s(&self, name: &str) -> Result<Option<Vec<Option<f64>>>, String> {
        match self.get(name) {
            None | Some(JsonValue::Null) => Ok(None),
            Some(_) => self.optional_f64s(name).map(Some),
        }
    }

    fn video_metadata(&self) -> Result<VideoAssetMetadata, String> {
        let fps = self.u64(db::FPS)? as u32;
        let frames = self.u64(db::FRAMES)?;
        let timing = VideoTiming::new(fps, frames);
        Ok(VideoAssetMetadata::new(
            self.string(db::STREAM_ID)?.to_owned(),
            self.string(db::CAMERA_ID)?.to_owned(),
            timing.duration_seconds(),
        )
        .with_size(VideoSize::new(
            self.u64(db::WIDTH)? as u32,
            self.u64(db::HEIGHT)? as u32,
        ))
        .with_timing(timing)
        .with_segment(VideoSegment::new(
            self.u64(db::SEGMENT_INDEX)? as u32,
            self.get(db::TIMELINE_OFFSET_NS)
                .and_then(JsonValue::as_u64)
                .map(|value| value as f64 / 1_000_000_000.0),
        )))
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
mod tests {
    use super::*;

    #[test]
    fn inspection_reads_only_the_selected_joint_stream_layout() {
        let sql =
            EpisodeCatalogSelection::Inspection("episode_000052").streams_sql("spiral_robotics");

        assert!(sql.contains("episode_id = 'episode_000052'"));
        assert!(sql.contains("modality = 'joint'"));
        assert!(sql.contains("joint_names"));
        assert!(!sql.contains("camera_id"));
        assert!(!sql.contains("temperature_celsius"));
    }

    #[test]
    fn complete_episode_reads_retain_condition_and_video_columns() {
        let sql = EpisodeCatalogSelection::Episode("episode_000052").streams_sql("spiral_robotics");

        assert!(sql.starts_with("SELECT *"));
        assert!(sql.contains("episode_id = 'episode_000052'"));
    }
}

/// Complete episode replacement operations for one project.
pub(crate) struct EpisodePersistence<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> EpisodePersistence<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    pub(crate) async fn commit(
        &self,
        episode: &FullEpisode,
        videos: &[VideoAsset],
    ) -> Result<(), String> {
        let mut connection = self.project_store.connect().await?;
        EpisodeReplacement::new(self.project_store.namespace(), episode, videos)
            .save(&mut connection)
            .await
    }
}

struct EpisodeReplacement<'a> {
    namespace: &'a str,
    episode: &'a FullEpisode,
    videos: &'a [VideoAsset],
}

impl<'a> EpisodeReplacement<'a> {
    const fn new(namespace: &'a str, episode: &'a FullEpisode, videos: &'a [VideoAsset]) -> Self {
        Self {
            namespace,
            episode,
            videos,
        }
    }

    async fn save(self, connection: &mut ProjectConnection) -> Result<(), String> {
        let Self {
            namespace,
            episode,
            videos,
        } = self;
        let had_video = connection
            .episode_has_video_assets(namespace, episode.start().id())
            .await?;
        let video_rows_changed = had_video || !videos.is_empty();
        let dataset_exists = if video_rows_changed {
            connection.video_dataset_exists(namespace).await?
        } else {
            false
        };
        let previous_video_episodes = if video_rows_changed {
            connection.stored_video_episode_ids(namespace).await?
        } else {
            BTreeSet::new()
        };
        let has_video_rows = !videos.is_empty()
            || previous_video_episodes
                .iter()
                .any(|episode_id| episode_id != episode.start().id());
        if !dataset_exists && !previous_video_episodes.is_empty() && has_video_rows {
            connection
                .retarget_stored_video_media_relations(namespace)
                .await?;
        }
        // Replacement is idempotent. Clear any earlier complete row and partial
        // children before writing, so a retry after a process or network failure
        // can finish without duplicating already-landed rows.
        connection
            .clear_episode_rows_only(namespace, episode.start().id())
            .await?;
        let datasets = Table::Datasets.qualified_name(namespace);
        if !connection
            .table_has_dataset(&datasets, episode.start().dataset_id())
            .await?
        {
            connection
                .ingest_batch(
                    &datasets,
                    episode
                        .start()
                        .dataset_batch()
                        .map_err(|error| error.to_string())?,
                )
                .await?;
        }
        connection
            .ingest_batch(
                &Table::EpisodeStreams.qualified_name(namespace),
                episode
                    .start()
                    .streams_batch(episode.completion())
                    .map_err(|error| error.to_string())?,
            )
            .await?;
        if !videos.is_empty() {
            let streams = videos
                .iter()
                .map(|video| {
                    (
                        video.stream_id().to_owned(),
                        VideoStreamRow::new(
                            episode.start().id().to_owned(),
                            video.stream_id().to_owned(),
                            VideoStreamProperties::new(
                                video.camera_id().to_owned(),
                                video.size(),
                                video.timing().fps(),
                            ),
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>()
                .into_values()
                .collect::<Vec<_>>();
            connection
                .ingest_batch(
                    &Table::EpisodeStreams.qualified_name(namespace),
                    streams.batch().map_err(|error| error.to_string())?,
                )
                .await?;
        }
        let mut encoder = SpiralEpisodeEncoder::new(episode.start().clone())
            .map_err(|error| error.to_string())?;
        let mut rows = Vec::new();
        for sample in episode.samples() {
            rows.extend(encoder.encode(sample).map_err(|error| error.to_string())?);
            if rows.len() >= JOINT_BATCH_ROWS {
                let batch = rows.batch().map_err(|error| error.to_string())?;
                connection
                    .ingest_batch(&Table::JointSamples.qualified_name(namespace), batch)
                    .await?;
                rows.clear();
            }
        }
        if !rows.is_empty() {
            let batch = rows.batch().map_err(|error| error.to_string())?;
            connection
                .ingest_batch(&Table::JointSamples.qualified_name(namespace), batch)
                .await?;
        }
        if !videos.is_empty() {
            let media_relation = SpiralSql::video_dataset_output(namespace);
            let batch = VideoAsset::storage_publication_batch(
                episode.start().id(),
                videos,
                &media_relation,
            )?;
            connection
                .ingest_batch(&Table::VideoAssets.qualified_name(namespace), batch)
                .await?;
        }
        if video_rows_changed {
            if has_video_rows {
                connection
                    .rebuild_video_dataset_internal(namespace, dataset_exists)
                    .await?;
            } else {
                connection.drop_video_dataset(namespace).await?;
            }
        }
        // Inserted last: readers never see an episode whose streams or media only
        // partially arrived.
        connection
            .ingest_batch(
                &Table::Episodes.qualified_name(namespace),
                episode
                    .completion()
                    .episode_batch(episode.start(), encoder.frames())
                    .map_err(|error| error.to_string())?,
            )
            .await?;
        Ok(())
    }
}

/// Rebuild the namespace's one managed MP4 index from all durable `video_assets` rows.
///
/// The output is deliberately refreshed even when the Dataset already exists.
/// Project import replaces those source rows in place, so merely observing an
/// existing output does not prove that its video index matches the imported
/// payloads.
impl ProjectConnection {
    pub(crate) async fn rebuild_video_dataset_internal(
        &mut self,
        namespace: &str,
        dataset_exists: bool,
    ) -> Result<(), String> {
        let connection = self;
        // One unfiltered view and Dataset cover the whole Studio namespace. The
        // output preserves episode and stream IDs, so consumers select individual
        // media without multiplying catalog resources by episode count.
        let sources = SpiralSql::video_sources_relation(namespace);
        let assets = Table::VideoAssets.qualified_name(namespace);
        let dataset = SpiralSql::video_dataset_relation(namespace);
        connection
            .execute_update_sql(&SpiralSql::create_video_sources(&sources, &assets))
            .await
            .map_err(|error| format!("creating Spiral video source view: {error}"))?;
        if !dataset_exists {
            connection
                .execute_update_sql(&SpiralSql::create_video_dataset(&dataset, &sources))
                .await
                .map_err(|error| format!("creating Spiral MP4 dataset: {error}"))?;
        }
        connection
            .execute_update_sql(&SpiralSql::refresh_dataset(&dataset))
            .await
            .map_err(|error| format!("refreshing Spiral MP4 dataset: {error}"))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn rebuild_video_dataset(
        &mut self,
        namespace: &str,
        dataset_exists: bool,
    ) -> Result<(), String> {
        self.rebuild_video_dataset_internal(namespace, dataset_exists)
            .await
    }

    pub(crate) async fn video_dataset_exists(&mut self, namespace: &str) -> Result<bool, String> {
        let connection = self;
        let dataset = SpiralSql::video_dataset_relation(namespace);
        match connection
            .query_rows(&format!("DESCRIBE {dataset}"), 1)
            .await
        {
            Ok(_) => Ok(true),
            Err(error)
                if error.contains("unknown resource")
                    || error.contains("does not exist")
                    || error.contains("unknown relation") =>
            {
                Ok(false)
            }
            Err(error) => Err(format!("checking Spiral MP4 dataset: {error}")),
        }
    }

    pub(crate) async fn drop_video_dataset(&mut self, namespace: &str) -> Result<(), String> {
        let connection = self;
        let dataset = SpiralSql::video_dataset_relation(namespace);
        QualifiedRelation::try_from(dataset.as_str())
            .map_err(|error| format!("refusing to drop an invalid Spiral MP4 Dataset: {error}"))?;
        connection
            .execute_update_sql(&SpiralSql::drop_video_dataset_sql(&dataset))
            .await
            .map_err(|error| format!("dropping Spiral MP4 dataset with CASCADE: {error}"))?;
        connection
            .execute_update_sql(&SpiralSql::drop_video_sources(
                &SpiralSql::video_sources_relation(namespace),
            ))
            .await
            .map_err(|error| format!("dropping Spiral video source view: {error}"))
    }

    pub(crate) async fn clear_episode_rows(
        &mut self,
        namespace: &str,
        episode_id: &str,
    ) -> Result<(), String> {
        let connection = self;
        let previous_video_episodes = connection.stored_video_episode_ids(namespace).await?;
        let had_video = previous_video_episodes.contains(episode_id);
        let dataset_exists = if had_video {
            connection.video_dataset_exists(namespace).await?
        } else {
            false
        };
        if had_video && !dataset_exists && previous_video_episodes.len() > 1 {
            connection
                .retarget_stored_video_media_relations(namespace)
                .await?;
        }
        connection
            .clear_episode_rows_only(namespace, episode_id)
            .await?;
        if !had_video {
            return Ok(());
        }
        if previous_video_episodes.len() == 1 {
            connection.drop_video_dataset(namespace).await?;
        } else {
            connection
                .rebuild_video_dataset_internal(namespace, dataset_exists)
                .await?;
        }
        Ok(())
    }

    async fn clear_episode_rows_only(
        &mut self,
        namespace: &str,
        episode_id: &str,
    ) -> Result<(), String> {
        let connection = self;
        for table in super::EpisodeRelations::DELETION_RELATIONS {
            let target = table.qualified_name(namespace);
            connection
                .execute_update_sql(&SpiralSql::delete_episode(&target, episode_id))
                .await
                .map_err(|error| format!("clearing prior Spiral rows in {target}: {error}"))?;
        }
        Ok(())
    }

    pub(crate) async fn stored_video_episode_ids(
        &mut self,
        namespace: &str,
    ) -> Result<BTreeSet<String>, String> {
        let connection = self;
        const MAX_VIDEO_EPISODES: usize = 100_000;

        let assets = Table::VideoAssets.qualified_name(namespace);
        let result = connection
            .query_rows(
                &format!(
                    "SELECT DISTINCT {episode_id} FROM {assets}",
                    episode_id = db::EPISODE_ID
                ),
                MAX_VIDEO_EPISODES,
            )
            .await?;
        if result.truncated() {
            return Err(format!(
                "project has more than {MAX_VIDEO_EPISODES} video episodes; refusing incomplete video-resource reconciliation"
            ));
        }
        result
            .into_rows()
            .into_iter()
            .map(|row| {
                row.get(db::EPISODE_ID)
                    .and_then(JsonValue::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| "stored video asset has no valid episode ID".to_owned())
            })
            .collect()
    }

    async fn episode_has_video_assets(
        &mut self,
        namespace: &str,
        episode_id: &str,
    ) -> Result<bool, String> {
        let connection = self;
        let assets = Table::VideoAssets.qualified_name(namespace);
        let result = connection.query_rows(
        &format!(
            "SELECT {episode_column} FROM {assets} WHERE {episode_column} = {episode_id} LIMIT 1",
            episode_column = db::EPISODE_ID,
            episode_id = SpiralSql::string_literal(episode_id),
        ),
        1,
    ).await?;
        Ok(!result.rows().is_empty())
    }

    pub(crate) async fn retarget_stored_video_media_relations(
        &mut self,
        namespace: &str,
    ) -> Result<(), String> {
        let connection = self;
        let assets = Table::VideoAssets.qualified_name(namespace);
        let output = SpiralSql::video_dataset_output(namespace);
        connection
            .execute_update_sql(&format!(
                "UPDATE {assets} SET {media_relation} = {output}",
                media_relation = db::MEDIA_RELATION,
                output = SpiralSql::string_literal(&output),
            ))
            .await
            .map_err(|error| format!("retargeting stored video media relations: {error}"))
    }

    async fn table_has_dataset(&mut self, target: &str, dataset_id: &str) -> Result<bool, String> {
        let connection = self;
        let result = connection
            .query_rows(&SpiralSql::dataset_exists(target, dataset_id), 1)
            .await?;
        Ok(!result.rows().is_empty())
    }
}
