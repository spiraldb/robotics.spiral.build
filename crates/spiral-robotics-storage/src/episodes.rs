//! Episode capture and inspection backed directly by SpiralDB.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::schema::{Table, column as db};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use spiral_robotics_api::identity::EpisodeId;
use spiral_robotics_api::recording::{
    EpisodeArm, EpisodeAssessment, EpisodeIdentity as EpisodeSummaryIdentity, EpisodeSummary,
    EpisodeTimeline, RecordingGap,
};
use spiral_robotics_api::robots::MotorLimits;
use spiral_robotics_hardware::Calibration;
use spiral_robotics_hardware::{
    ArmConditionSummary as HardwareArmConditionSummary, CompletedRecording, RecordingCompletion,
    RecordingGap as HardwareRecordingGap, RecordingWriter,
};
pub use spiral_robotics_hardware::{EpisodeCheckpoint, Sample};

use crate::adbc::ProjectStore;
use crate::episode_schema::{JointSampleRows, SpiralEpisodeEncoder};
use crate::manager::EpisodeReplacement;
use crate::sql::SpiralSql;
use crate::video::{VideoAssetMetadata, VideoSegment, VideoSize, VideoTiming};

const SAMPLE_QUEUE_DEPTH: usize = 2_048;
/// Hard storage limit for a single recorded episode.
pub const MAX_EPISODE_FRAMES: u64 = 1_000_000;
const MAX_EPISODE_ROWS: usize = MAX_EPISODE_FRAMES as usize * 16;
const NO_SAMPLE_TIMESTAMP_BITS: u64 = u64::MAX;

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
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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

    pub fn arm(&self) -> &str {
        &self.arm
    }

    pub fn temperature_celsius(&self) -> &[Option<f64>] {
        &self.temperature_celsius
    }

    pub fn voltage_volts(&self) -> &[Option<f64>] {
        &self.voltage_volts
    }

    pub fn limits(&self) -> &[MotorLimits] {
        &self.limits
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EpisodeStart {
    #[serde(flatten)]
    identity: EpisodeIdentity,
    #[serde(flatten)]
    recording: EpisodeRecordingMetadata,
    arms: Vec<EpisodeArm>,
    condition: Vec<ArmCondition>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EpisodeIdentity {
    id: String,
    dataset_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EpisodeRecordingMetadata {
    started_ms: i64,
    sample_hz: f64,
    source: String,
    note: Option<String>,
}

impl EpisodeStart {
    pub(crate) fn new(id: String, dataset_id: String) -> Self {
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

    pub(crate) fn with_timing(mut self, started_ms: i64, sample_hz: f64) -> Self {
        self.recording.started_ms = started_ms;
        self.recording.sample_hz = sample_hz;
        self
    }

    pub(crate) fn with_current_timing(self, sample_hz: f64) -> Self {
        let started_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        self.with_timing(started_ms, sample_hz)
    }

    pub(crate) fn with_source(mut self, source: String, note: Option<String>) -> Self {
        self.recording.source = source;
        self.recording.note = note;
        self
    }

    pub(crate) fn with_arms(mut self, arms: Vec<EpisodeArm>, condition: Vec<ArmCondition>) -> Self {
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
    pub const fn started_ms(&self) -> i64 {
        self.recording.started_ms
    }
    pub const fn sample_hz(&self) -> f64 {
        self.recording.sample_hz
    }
    pub fn source(&self) -> &str {
        &self.recording.source
    }
    pub fn note(&self) -> Option<&str> {
        self.recording.note.as_deref()
    }
    pub fn arms(&self) -> &[EpisodeArm] {
        &self.arms
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

pub struct ReplayRequest<'a> {
    arm: &'a str,
    seconds: f64,
}

impl<'a> ReplayRequest<'a> {
    pub fn new(arm: &'a str, seconds: f64) -> Self {
        Self { arm, seconds }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct EpisodeCompletion {
    duration_seconds: f64,
    recording_gaps: Vec<RecordingGap>,
    max_follow_error_degrees: Option<f64>,
    discarded: bool,
    condition: Vec<ArmConditionSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ArmConditionSummary {
    arm: String,
    peak_temperature_celsius: Vec<Option<f64>>,
    min_voltage_volts: Vec<Option<f64>>,
    peak_load: Vec<Option<f64>>,
    peak_current_amps: Vec<Option<f64>>,
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

impl EpisodeCompletion {
    pub(crate) fn new(
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

    pub(crate) const fn with_max_follow_error_degrees(
        mut self,
        max_follow_error_degrees: Option<f64>,
    ) -> Self {
        self.max_follow_error_degrees = max_follow_error_degrees;
        self
    }

    pub(crate) const fn with_discarded(mut self, discarded: bool) -> Self {
        self.discarded = discarded;
        self
    }

    pub(crate) const fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }

    pub(crate) fn recording_gaps(&self) -> &[RecordingGap] {
        &self.recording_gaps
    }

    pub(crate) const fn max_follow_error_degrees(&self) -> Option<f64> {
        self.max_follow_error_degrees
    }

    pub(crate) const fn discarded(&self) -> bool {
        self.discarded
    }
}

#[derive(Clone, Debug)]
pub struct CompletedEpisode {
    start: EpisodeStart,
    samples: Vec<Sample>,
    completion: EpisodeCompletion,
}

impl CompletedEpisode {
    pub const fn start(&self) -> &EpisodeStart {
        &self.start
    }
    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    pub(crate) const fn completion(&self) -> &EpisodeCompletion {
        &self.completion
    }
}

#[derive(Clone, Debug)]
pub struct EpisodeInspection {
    episode: CompletedEpisode,
    summary: EpisodeSummary,
    videos: Vec<VideoAssetMetadata>,
}

impl EpisodeInspection {
    pub fn into_parts(self) -> (CompletedEpisode, EpisodeSummary, Vec<VideoAssetMetadata>) {
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

    pub fn with_skipped(mut self, episodes_skipped: u64, warnings: Vec<String>) -> Self {
        self.episodes_skipped = episodes_skipped;
        self.warnings = warnings;
        self
    }

    pub const fn frames_updated(&self) -> u64 {
        self.frames_updated
    }
    pub const fn episodes_unchanged(&self) -> u64 {
        self.episodes_unchanged
    }
    pub const fn episodes_skipped(&self) -> u64 {
        self.episodes_skipped
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub fn into_parts(self) -> (Vec<String>, u64, u64, u64, Vec<String>) {
        (
            self.updated_episode_ids,
            self.frames_updated,
            self.episodes_unchanged,
            self.episodes_skipped,
            self.warnings,
        )
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
    pub fn new(
        arm: String,
        normalized: BTreeMap<String, Option<f64>>,
        raw: BTreeMap<String, i64>,
    ) -> Self {
        Self {
            arm,
            normalized,
            raw,
            ..Self::default()
        }
    }
    pub fn with_velocity(mut self, velocity: Option<BTreeMap<String, Option<f64>>>) -> Self {
        self.velocity = velocity;
        self
    }
    pub fn with_effort(mut self, effort: Option<BTreeMap<String, Option<f64>>>) -> Self {
        self.effort = effort;
        self
    }
    pub fn with_action(mut self, action: Option<BTreeMap<String, Option<f64>>>) -> Self {
        self.action = action;
        self
    }
    pub fn arm(&self) -> &str {
        &self.arm
    }
    pub fn normalized(&self) -> &BTreeMap<String, Option<f64>> {
        &self.normalized
    }
    pub fn raw(&self) -> &BTreeMap<String, i64> {
        &self.raw
    }
    pub fn velocity(&self) -> Option<&BTreeMap<String, Option<f64>>> {
        self.velocity.as_ref()
    }
    pub fn effort(&self) -> Option<&BTreeMap<String, Option<f64>>> {
        self.effort.as_ref()
    }
    pub fn action(&self) -> Option<&BTreeMap<String, Option<f64>>> {
        self.action.as_ref()
    }
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
    pub const fn new(frame: u64, seconds: f64) -> Self {
        Self {
            frame,
            seconds,
            follow_error_degrees: None,
            arms: Vec::new(),
        }
    }
    pub fn with_follow_error_degrees(mut self, value: Option<f64>) -> Self {
        self.follow_error_degrees = value;
        self
    }
    pub fn with_arms(mut self, arms: Vec<TimelineArmSample>) -> Self {
        self.arms = arms;
        self
    }
    pub const fn frame(&self) -> u64 {
        self.frame
    }
    pub const fn seconds(&self) -> f64 {
        self.seconds
    }
    pub const fn follow_error_degrees(&self) -> Option<f64> {
        self.follow_error_degrees
    }
    pub fn arms(&self) -> &[TimelineArmSample] {
        &self.arms
    }
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
    last_issue: Mutex<Option<String>>,
    seek_sequence: AtomicU64,
    latest_seek: Mutex<BTreeMap<String, u64>>,
    seek_gate: tokio::sync::Mutex<()>,
}

impl EpisodeStore {
    pub fn new(project_store: Arc<ProjectStore>) -> Self {
        Self {
            project_store,
            last_issue: Mutex::new(None),
            seek_sequence: AtomicU64::new(0),
            latest_seek: Mutex::new(BTreeMap::new()),
            seek_gate: tokio::sync::Mutex::new(()),
        }
    }

    pub fn list(&self) -> Vec<EpisodeSummary> {
        match self.try_list() {
            Ok(episodes) => {
                *self.last_issue.lock_unpoisoned() = None;
                episodes
            }
            Err(error) => {
                *self.last_issue.lock_unpoisoned() = Some(error.to_string());
                Vec::new()
            }
        }
    }
    pub fn issues(&self) -> Vec<String> {
        self.last_issue.lock_unpoisoned().iter().cloned().collect()
    }
    pub fn get(&self, id: &str) -> Result<EpisodeSummary, EpisodeError> {
        EpisodeId::try_from(id).map_err(|_| EpisodeError::InvalidId(id.to_owned()))?;
        self.try_list()?
            .into_iter()
            .find(|episode| episode.id() == id)
            .ok_or_else(|| EpisodeError::NotFound(id.to_owned()))
    }

    pub fn completed(&self, id: &str) -> Result<CompletedEpisode, EpisodeError> {
        EpisodeId::try_from(id).map_err(|_| EpisodeError::InvalidId(id.to_owned()))?;
        let episodes = Table::Episodes.qualified_name(self.project_store.namespace());
        let samples = Table::JointSamples.qualified_name(self.project_store.namespace());
        let episode_value = SpiralSql::string_literal(id);
        let sample_query = SpiralSql::episode_samples(self.project_store.namespace(), id);
        self.project_store
            .run_blocking(move |connection| {
                if !connection.table_exists(&episodes)? {
                    return Err(EpisodeError::NotFound(id.to_owned()).to_string());
                }
                let metadata = connection.run_query(
                    &format!(
                        "SELECT {} FROM {episodes} WHERE {} = {episode_value} LIMIT 1",
                        db::METADATA_JSON,
                        db::EPISODE_ID
                    ),
                    1,
                )?;
                let row = metadata
                    .rows()
                    .first()
                    .ok_or_else(|| EpisodeError::NotFound(id.to_owned()).to_string())?;
                let metadata = row
                    .get(db::METADATA_JSON)
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| format!("episode {id:?} has no metadata"))?;
                let (start, completion): (EpisodeStart, EpisodeCompletion) =
                    serde_json::from_str(metadata)
                        .map_err(|error| format!("episode {id:?} metadata is invalid: {error}"))?;
                let rows = if connection.table_exists(&samples)? {
                    let result = connection.run_query(&sample_query, MAX_EPISODE_ROWS)?;
                    if result.truncated() {
                        return Err(format!("episode {id:?} exceeds the supported row limit"));
                    }
                    result.into_rows()
                } else {
                    Vec::new()
                };
                let decoded = start.decode_samples(rows)?;
                Ok(CompletedEpisode {
                    start,
                    samples: decoded,
                    completion,
                })
            })
            .map_err(EpisodeError::Invalid)
    }

    pub fn inspection(&self, id: &str) -> Result<EpisodeInspection, EpisodeError> {
        let episode = self.completed(id)?;
        let summary = episode
            .start
            .summary(&episode.completion, episode.samples.len() as u64);
        let videos = self.video_metadata_for_existing(id)?;
        Ok(EpisodeInspection {
            episode,
            summary,
            videos,
        })
    }

    pub fn next_id(&self) -> Result<String, EpisodeError> {
        let highest = self
            .try_list()?
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

    pub fn begin_in_dataset(
        &self,
        request: EpisodeRecordingRequest,
    ) -> Result<EpisodeWriter, EpisodeError> {
        if request.arms.is_empty() {
            return Err(EpisodeError::Invalid(
                "a take needs at least one connected arm to sample".to_owned(),
            ));
        }
        let start = EpisodeStart::new(self.next_id()?, request.dataset_id)
            .with_current_timing(request.sample_hz)
            .with_source(request.source, request.note)
            .with_arms(request.arms, request.condition);
        SpiralEpisodeEncoder::new(start.clone())?;
        EpisodeWriter::start(start, self.project_store.clone())
    }

    pub fn delete(&self, id: &str) -> Result<(), EpisodeError> {
        self.get(id)?;
        let id = id.to_owned();
        let namespace = self.project_store.namespace().to_owned();
        self.project_store
            .run_blocking(move |connection| connection.clear_episode_rows(&namespace, &id))
            .map_err(EpisodeError::Invalid)
    }

    pub fn reapply_calibration(
        &self,
        robot: &str,
        calibration: &Calibration,
    ) -> Result<ReappliedEpisodePositions, EpisodeError> {
        let mut result = ReappliedEpisodePositions::default();
        for summary in self.try_list()? {
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
            let mut episode = self.completed(summary.id())?;
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
                self.replace_samples(&episode)?;
                result.frames_updated = result.frames_updated.saturating_add(summary.frames());
                result.updated_episode_ids.push(summary.id().to_owned());
            } else {
                result.episodes_unchanged += 1;
            }
        }
        Ok(result)
    }

    pub fn trajectory_at(
        &self,
        id: &str,
        request: ReplayRequest<'_>,
    ) -> Result<ReplayTrajectory, EpisodeError> {
        let arm = request.arm;
        let seconds = request.seconds;
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(EpisodeError::Invalid(
                "replay start time must be finite and non-negative".to_owned(),
            ));
        }
        let episode = self.completed(id)?;
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

    pub fn sample_at(&self, id: &str, seconds: f64) -> Result<TimelineSample, EpisodeError> {
        if !seconds.is_finite() {
            return Err(EpisodeError::Invalid(
                "timeline time must be finite".to_owned(),
            ));
        }
        let episode = self.completed(id)?;
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

    fn video_metadata_for_existing(
        &self,
        id: &str,
    ) -> Result<Vec<VideoAssetMetadata>, EpisodeError> {
        let relation = Table::VideoAssets.qualified_name(self.project_store.namespace());
        let id = SpiralSql::string_literal(id);
        self.project_store.run_blocking(move |connection| {
            if !connection.table_exists(&relation)? { return Ok(Vec::new()); }
            let result = connection.run_query(&format!("SELECT {}, {}, {}, {}, {}, {}, {}, {} FROM {relation} WHERE {} = {id} ORDER BY {}, {}", db::STREAM_ID, db::CAMERA_ID, db::WIDTH, db::HEIGHT, db::FPS, db::FRAMES, db::SEGMENT_INDEX, db::TIMELINE_OFFSET_NS, db::EPISODE_ID, db::STREAM_ID, db::SEGMENT_INDEX), 10_000)?;
            result
                .into_rows()
                .into_iter()
                .map(|row| row.video_metadata())
                .collect()
        }).map_err(EpisodeError::Invalid)
    }

    fn try_list(&self) -> Result<Vec<EpisodeSummary>, EpisodeError> {
        let relation = Table::Episodes.qualified_name(self.project_store.namespace());
        self.project_store
            .run_blocking(move |connection| {
                if !connection.table_exists(&relation)? {
                    return Ok(Vec::new());
                }
                let result = connection.run_query(
                    &format!(
                        "SELECT {}, {} FROM {relation} ORDER BY {}",
                        db::METADATA_JSON,
                        db::FRAMES,
                        db::STARTED_MS
                    ),
                    100_000,
                )?;
                result
                    .into_rows()
                    .into_iter()
                    .map(|row| row.episode_summary())
                    .collect()
            })
            .map_err(EpisodeError::Invalid)
    }

    fn replace_samples(&self, episode: &CompletedEpisode) -> Result<(), EpisodeError> {
        let episode = episode.clone();
        let namespace = self.project_store.namespace().to_owned();
        self.project_store
            .run_blocking(move |connection| {
                let sample_relation = Table::JointSamples.qualified_name(&namespace);
                let episode_relation = Table::Episodes.qualified_name(&namespace);
                connection.execute_update_sql(&SpiralSql::delete_episode(
                    &sample_relation,
                    episode.start.id(),
                ))?;
                connection.execute_update_sql(&SpiralSql::delete_episode(
                    &episode_relation,
                    episode.start.id(),
                ))?;
                let mut encoder = SpiralEpisodeEncoder::new(episode.start.clone())
                    .map_err(|error| error.to_string())?;
                let mut rows = Vec::new();
                for sample in &episode.samples {
                    rows.extend(encoder.encode(sample).map_err(|error| error.to_string())?);
                    if rows.len() >= 512 {
                        connection.ingest_batch(
                            &sample_relation,
                            rows.batch().map_err(|error| error.to_string())?,
                        )?;
                        rows.clear();
                    }
                }
                if !rows.is_empty() {
                    connection.ingest_batch(
                        &sample_relation,
                        rows.batch().map_err(|error| error.to_string())?,
                    )?;
                }
                connection.ingest_batch(
                    &episode_relation,
                    episode
                        .completion
                        .episode_batch(&episode.start, encoder.frames())
                        .map_err(|error| error.to_string())?,
                )
            })
            .map_err(EpisodeError::Invalid)
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

pub(crate) trait EpisodeWriteSession: Send {
    fn start(&self) -> &EpisodeStart;
    fn push(&self, sample: Sample) -> bool;
    fn checkpoint(&self, completion: EpisodeCompletion) -> Result<EpisodeCheckpoint, String>;
    fn failure(&self) -> Option<String>;
    fn finish(
        self: Box<Self>,
        completion: EpisodeCompletion,
    ) -> Result<EpisodeSummary, EpisodeError>;
}

pub struct EpisodeWriter {
    session: Box<dyn EpisodeWriteSession>,
}

impl EpisodeWriter {
    pub(crate) fn from_session(session: impl EpisodeWriteSession + 'static) -> Self {
        Self {
            session: Box::new(session),
        }
    }
    fn start(start: EpisodeStart, project_store: Arc<ProjectStore>) -> Result<Self, EpisodeError> {
        let (sender, receiver) = mpsc::sync_channel(SAMPLE_QUEUE_DEPTH);
        let shared = Arc::new(WriterShared {
            written: AtomicU64::new(0),
            failure: Mutex::new(None),
        });
        let worker_shared = shared.clone();
        let validation_start = start.clone();
        let worker_repository = project_store.clone();
        let thread = std::thread::Builder::new()
            .name("spiral-episode-transaction".to_owned())
            .spawn(move || {
                EpisodeDrain {
                    start: &validation_start,
                    project_store: &worker_repository,
                    shared: &worker_shared,
                }
                .run(receiver)
            })
            .map_err(|source| {
                EpisodeError::Invalid(format!(
                    "could not start the SpiralDB episode transaction: {source}"
                ))
            })?;
        Ok(Self::from_session(SpiralEpisodeWriter {
            start,
            sender: Some(sender),
            thread: Some(thread),
            project_store,
            shared,
            last_queued_seconds: AtomicU64::new(NO_SAMPLE_TIMESTAMP_BITS),
        }))
    }
    pub fn id(&self) -> &str {
        self.session.start().id()
    }
    pub fn dataset_id(&self) -> &str {
        self.session.start().dataset_id()
    }
    pub fn started_ms(&self) -> i64 {
        self.session.start().started_ms()
    }
    pub fn push(&self, sample: Sample) -> bool {
        self.session.push(sample)
    }
    pub(crate) fn checkpoint(
        &self,
        completion: EpisodeCompletion,
    ) -> Result<EpisodeCheckpoint, String> {
        self.session.checkpoint(completion)
    }
    pub fn failure(&self) -> Option<String> {
        self.session.failure()
    }
    pub(crate) fn finish(
        self,
        completion: EpisodeCompletion,
    ) -> Result<EpisodeSummary, EpisodeError> {
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
                completion.recording_gaps().to_api(),
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
                completion.recording_gaps().to_api(),
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

trait RecordingGapProjection {
    fn to_api(&self) -> Vec<RecordingGap>;
}

impl RecordingGapProjection for [HardwareRecordingGap] {
    fn to_api(&self) -> Vec<RecordingGap> {
        self.iter()
            .map(|gap| RecordingGap::new(gap.after_seconds(), gap.duration_seconds()))
            .collect()
    }
}

struct SpiralEpisodeWriter {
    start: EpisodeStart,
    sender: Option<mpsc::SyncSender<WriterMessage>>,
    thread: Option<JoinHandle<Result<Vec<Sample>, String>>>,
    project_store: Arc<ProjectStore>,
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
            .ok_or_else(|| "episode transaction is closed".to_owned())?
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
    ) -> Result<EpisodeSummary, EpisodeError> {
        self.sender.take();
        let samples = self
            .thread
            .take()
            .expect("episode worker exists")
            .join()
            .map_err(|_| EpisodeError::Invalid("episode transaction worker panicked".to_owned()))?
            .map_err(EpisodeError::Invalid)?;
        if let Some(error) = self.failure() {
            return Err(EpisodeError::Invalid(error));
        }
        let completed = CompletedEpisode {
            start: self.start.clone(),
            samples,
            completion,
        };
        let namespace = self.project_store.namespace().to_owned();
        self.project_store
            .run_blocking(|connection| {
                EpisodeReplacement::new(&namespace, &completed, &[]).save(connection)
            })
            .map_err(EpisodeError::Invalid)?;
        Ok(completed
            .start
            .summary(&completed.completion, completed.samples.len() as u64))
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
    project_store: &'a ProjectStore,
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
                    let completed = CompletedEpisode {
                        start: self.start.clone(),
                        samples: samples.clone(),
                        completion,
                    };
                    let namespace = self.project_store.namespace().to_owned();
                    let result = self.project_store.run_blocking(|connection| {
                        EpisodeReplacement::new(&namespace, &completed, &[]).save(connection)
                    });
                    if let Err(error) = &result {
                        *self.shared.failure.lock_unpoisoned() = Some(error.clone());
                    }
                    let _ = complete.send(result);
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

    fn summary(&self, completion: &EpisodeCompletion, frames: u64) -> EpisodeSummary {
        let start = self;
        let identity =
            EpisodeSummaryIdentity::new(start.id(), start.dataset_id(), start.arms().to_vec())
                .with_source(start.source());
        let timeline =
            EpisodeTimeline::new(frames, completion.duration_seconds(), start.sample_hz())
                .with_recording_gaps(completion.recording_gaps().to_vec())
                .with_started_ms(start.started_ms());
        let assessment = EpisodeAssessment::new(
            completion.discarded(),
            completion.max_follow_error_degrees(),
            None,
        );
        EpisodeSummary::new(identity, timeline, assessment)
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

trait EpisodeJsonRow {
    fn string(&self, name: &str) -> Result<&str, String>;
    fn u64(&self, name: &str) -> Result<u64, String>;
    fn i64s(&self, name: &str) -> Result<Vec<i64>, String>;
    fn optional_f64s(&self, name: &str) -> Result<Vec<Option<f64>>, String>;
    fn optional_optional_f64s(&self, name: &str) -> Result<Option<Vec<Option<f64>>>, String>;
    fn episode_summary(&self) -> Result<EpisodeSummary, String>;
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

    fn episode_summary(&self) -> Result<EpisodeSummary, String> {
        let metadata = self
            .get(db::METADATA_JSON)
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "episode row has no metadata_json".to_owned())?;
        let (start, completion): (EpisodeStart, EpisodeCompletion) =
            serde_json::from_str(metadata).map_err(|error| error.to_string())?;
        Ok(start.summary(&completion, self.u64(db::FRAMES)?))
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
