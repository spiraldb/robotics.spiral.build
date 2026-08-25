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
use spiral_robotics_api::recording::{EpisodeArm, EpisodeSummary, RecordingGap};
use spiral_robotics_api::robots::MotorLimits;
use spiral_robotics_core::arm::SO101_JOINTS;
use spiral_robotics_core::calibration::Calibration;
pub use spiral_robotics_core::episode::MAX_EPISODE_FRAMES;
use spiral_robotics_hardware::RecordingCompletion;
use spiral_robotics_hardware::RecordingWriter;
pub use spiral_robotics_hardware::{ArmConditionSummary, EpisodeCheckpoint, Sample};

use crate::adbc::ProjectStore;
use crate::episode_schema::{SpiralEpisodeEncoder, episode_completion_batch, joint_samples_batch};
use crate::manager::{
    execute_update_sql, ingest_batch, replace_episode_blocking, run_query, table_exists,
};
use crate::sql;
use crate::video::{VideoAssetMetadata, VideoSegment, VideoSize, VideoTiming};

const SAMPLE_QUEUE_DEPTH: usize = 2_048;
const MAX_EPISODE_ROWS: usize = MAX_EPISODE_FRAMES as usize * 16;
const NO_SAMPLE_TIMESTAMP_BITS: u64 = u64::MAX;

type NormalizedPose = BTreeMap<String, f64>;

#[derive(Debug)]
pub struct ReplayTrajectory {
    pub frames: Vec<NormalizedPose>,
    pub start_frame: usize,
    pub resolved_start_seconds: f64,
}

#[derive(Debug)]
pub enum EpisodeError {
    InvalidId(String),
    NotFound(String),
    Invalid(String),
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
    pub arm: String,
    pub temperature_celsius: Vec<Option<f64>>,
    pub voltage_volts: Vec<Option<f64>>,
    pub limits: Vec<MotorLimits>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EpisodeStart {
    pub id: String,
    pub dataset_id: String,
    pub started_ms: i64,
    pub sample_hz: f64,
    pub source: String,
    pub note: Option<String>,
    pub arms: Vec<EpisodeArm>,
    pub condition: Vec<ArmCondition>,
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
pub struct EpisodeCompletion {
    pub duration_seconds: f64,
    pub recording_gaps: Vec<RecordingGap>,
    pub max_follow_error_degrees: Option<f64>,
    pub discarded: bool,
    pub condition: Vec<ArmConditionSummary>,
}

#[derive(Clone, Debug)]
pub struct CompletedEpisode {
    pub start: EpisodeStart,
    pub samples: Vec<Sample>,
    pub completion: EpisodeCompletion,
}

#[derive(Clone, Debug)]
pub struct EpisodeInspection {
    pub episode: CompletedEpisode,
    pub summary: EpisodeSummary,
    pub videos: Vec<VideoAssetMetadata>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReappliedEpisodePositions {
    pub updated_episode_ids: Vec<String>,
    pub frames_updated: u64,
    pub episodes_unchanged: u64,
    pub episodes_skipped: u64,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TimelineMetadata {
    pub samples: u64,
    pub first_seconds: Option<f64>,
    pub last_seconds: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TimelineArmSample {
    pub arm: String,
    pub normalized: BTreeMap<String, Option<f64>>,
    pub raw: BTreeMap<String, i64>,
    pub velocity: Option<BTreeMap<String, Option<f64>>>,
    pub effort: Option<BTreeMap<String, Option<f64>>>,
    pub action: Option<BTreeMap<String, Option<f64>>>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TimelineSample {
    pub frame: u64,
    pub seconds: f64,
    pub follow_error_degrees: Option<f64>,
    pub arms: Vec<TimelineArmSample>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TimelineTraces {
    pub episode: String,
    pub samples: u64,
    pub buckets: usize,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    pub lanes: Vec<TimelineTraceLane>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TimelineTraceLane {
    pub joint: String,
    pub series: Vec<TimelineTraceSeries>,
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TimelineTraceSeries {
    pub arm: String,
    pub points: Vec<TimelineTracePoint>,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TimelineTracePoint {
    pub seconds: f64,
    pub value: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default)]
struct TraceBucket {
    minimum: Option<(f64, f64)>,
    maximum: Option<(f64, f64)>,
    first_missing: Option<f64>,
}

impl TraceBucket {
    fn observe(&mut self, seconds: f64, value: Option<f64>) {
        let Some(value) = value.filter(|value| value.is_finite()) else {
            self.first_missing.get_or_insert(seconds);
            return;
        };
        if self.minimum.is_none_or(|(_, minimum)| value < minimum) {
            self.minimum = Some((seconds, value));
        }
        if self.maximum.is_none_or(|(_, maximum)| value > maximum) {
            self.maximum = Some((seconds, value));
        }
    }
    fn into_points(self) -> Vec<TimelineTracePoint> {
        let mut points = Vec::with_capacity(3);
        if let Some((seconds, value)) = self.minimum {
            points.push(TimelineTracePoint {
                seconds,
                value: Some(value),
            });
        }
        if let Some((seconds, value)) = self.maximum
            && self.minimum != self.maximum
        {
            points.push(TimelineTracePoint {
                seconds,
                value: Some(value),
            });
        }
        if let Some(seconds) = self.first_missing {
            points.push(TimelineTracePoint {
                seconds,
                value: None,
            });
        }
        points.sort_by(|left, right| left.seconds.total_cmp(&right.seconds));
        points
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
                *lock(&self.last_issue) = None;
                episodes
            }
            Err(error) => {
                *lock(&self.last_issue) = Some(error.to_string());
                Vec::new()
            }
        }
    }
    pub fn issues(&self) -> Vec<String> {
        lock(&self.last_issue).iter().cloned().collect()
    }
    pub fn get(&self, id: &str) -> Result<EpisodeSummary, EpisodeError> {
        validate_episode_id(id)?;
        self.try_list()?
            .into_iter()
            .find(|episode| episode.id == id)
            .ok_or_else(|| EpisodeError::NotFound(id.to_owned()))
    }

    pub fn completed(&self, id: &str) -> Result<CompletedEpisode, EpisodeError> {
        validate_episode_id(id)?;
        let episodes = Table::Episodes.qualified_name(self.project_store.namespace());
        let samples = Table::JointSamples.qualified_name(self.project_store.namespace());
        let episode_value = sql::string_literal(id);
        let sample_query = sql::episode_samples(self.project_store.namespace(), id);
        self.project_store
            .run_blocking(move |connection| {
                if !table_exists(connection, &episodes)? {
                    return Err(EpisodeError::NotFound(id.to_owned()).to_string());
                }
                let metadata = run_query(
                    connection,
                    &format!(
                        "SELECT {} FROM {episodes} WHERE {} = {episode_value} LIMIT 1",
                        db::METADATA_JSON,
                        db::EPISODE_ID
                    ),
                    1,
                )?;
                let row = metadata
                    .rows
                    .first()
                    .ok_or_else(|| EpisodeError::NotFound(id.to_owned()).to_string())?;
                let metadata = row
                    .get(db::METADATA_JSON)
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| format!("episode {id:?} has no metadata"))?;
                let (start, completion): (EpisodeStart, EpisodeCompletion) =
                    serde_json::from_str(metadata)
                        .map_err(|error| format!("episode {id:?} metadata is invalid: {error}"))?;
                let rows = if table_exists(connection, &samples)? {
                    let result = run_query(connection, &sample_query, MAX_EPISODE_ROWS)?;
                    if result.truncated {
                        return Err(format!("episode {id:?} exceeds the supported row limit"));
                    }
                    result.rows
                } else {
                    Vec::new()
                };
                let decoded = decode_samples(&start, rows)?;
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
        let summary = summary(
            &episode.start,
            &episode.completion,
            episode.samples.len() as u64,
        );
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
            .filter_map(|episode| episode_number(&episode.id))
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
        let start = EpisodeStart {
            id: self.next_id()?,
            dataset_id: request.dataset_id,
            started_ms: unix_millis(),
            sample_hz: request.sample_hz,
            source: request.source,
            note: request.note,
            arms: request.arms,
            condition: request.condition,
        };
        SpiralEpisodeEncoder::new(start.clone())?;
        EpisodeWriter::start(start, self.project_store.clone())
    }

    pub fn judge(&self, id: &str, discarded: bool) -> Result<EpisodeSummary, EpisodeError> {
        let mut episode = self.completed(id)?;
        episode.completion.discarded = discarded;
        self.replace_completion(&episode)?;
        self.get(id)
    }

    pub fn delete(&self, id: &str) -> Result<(), EpisodeError> {
        self.get(id)?;
        let id = id.to_owned();
        let namespace = self.project_store.namespace().to_owned();
        self.project_store
            .run_blocking(move |connection| {
                crate::manager::clear_episode_rows(connection, &namespace, &id)
            })
            .map_err(EpisodeError::Invalid)
    }

    pub fn reapply_calibration(
        &self,
        robot: &str,
        calibration: &Calibration,
    ) -> Result<ReappliedEpisodePositions, EpisodeError> {
        let mut result = ReappliedEpisodePositions::default();
        for summary in self.try_list()? {
            let Some(arm_index) = summary.arms.iter().position(|arm| arm.id == robot) else {
                continue;
            };
            let joints = &summary.arms[arm_index].joints;
            let missing = joints
                .iter()
                .filter(|joint| !calibration.motors().contains_key(*joint))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                result.episodes_skipped += 1;
                result.warnings.push(format!(
                    "take {:?} uses joints absent from the current calibration: {}",
                    summary.id,
                    missing.join(", ")
                ));
                continue;
            }
            let mut episode = self.completed(&summary.id)?;
            let mut changed = false;
            for sample in &mut episode.samples {
                let normalized = joints.iter().enumerate().map(|(joint_index, joint)| {
                    calibration.motors()[joint].normalize_raw(sample.raw[arm_index][joint_index]).map(Some).map_err(|error| EpisodeError::Invalid(format!("could not reapply calibration to take {:?} joint {joint:?}: {error}", summary.id)))
                }).collect::<Result<Vec<_>, _>>()?;
                if sample.normalized[arm_index] != normalized {
                    sample.normalized[arm_index] = normalized;
                    changed = true;
                }
            }
            if changed {
                self.replace_samples(&episode)?;
                result.frames_updated = result.frames_updated.saturating_add(summary.frames);
                result.updated_episode_ids.push(summary.id);
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
            .position(|entry| entry.id == arm)
            .ok_or_else(|| {
                EpisodeError::Invalid(format!("take {id:?} did not record arm {arm:?}"))
            })?;
        let joints = &episode.start.arms[arm_index].joints;
        let timestamped = episode
            .samples
            .iter()
            .filter_map(|sample| {
                let values = &sample.normalized[arm_index];
                if values.iter().any(Option::is_none) {
                    return None;
                }
                Some((
                    sample.seconds,
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

    pub fn timeline_metadata(&self, id: &str) -> Result<TimelineMetadata, EpisodeError> {
        let episode = self.completed(id)?;
        Ok(TimelineMetadata {
            samples: episode.samples.len() as u64,
            first_seconds: episode.samples.first().map(|sample| sample.seconds),
            last_seconds: episode.samples.last().map(|sample| sample.seconds),
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
                (seconds - left.seconds)
                    .abs()
                    .total_cmp(&(seconds - right.seconds).abs())
                    .then_with(|| left_index.cmp(right_index))
            })
            .ok_or_else(|| EpisodeError::Invalid(format!("take {id:?} has no samples")))?;
        Ok(render_timeline_sample(&episode.start, sample, frame as u64))
    }

    pub fn reserve_seek(&self, robot: &str) -> u64 {
        let ticket = self.seek_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        lock(&self.latest_seek).insert(robot.to_owned(), ticket);
        ticket
    }
    pub fn seek_is_current(&self, robot: &str, ticket: u64) -> bool {
        lock(&self.latest_seek)
            .get(robot)
            .is_some_and(|latest| *latest == ticket)
    }
    pub async fn lock_seek(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.seek_gate.lock().await
    }

    pub fn trace(&self, id: &str, buckets: usize) -> Result<Vec<Option<f64>>, EpisodeError> {
        let episode = self.completed(id)?;
        if episode.samples.is_empty() {
            return Ok(Vec::new());
        }
        let width = episode
            .samples
            .len()
            .div_ceil(buckets.clamp(1, 2_048))
            .max(1);
        Ok(episode
            .samples
            .chunks(width)
            .map(|chunk| {
                chunk
                    .iter()
                    .filter_map(|sample| sample.follow_error_degrees)
                    .filter(|value| value.is_finite())
                    .reduce(f64::max)
            })
            .collect())
    }

    pub fn timeline_traces(
        &self,
        id: &str,
        buckets: usize,
    ) -> Result<TimelineTraces, EpisodeError> {
        let episode = self.completed(id)?;
        let sample_count = episode.samples.len();
        let bucket_count = buckets.clamp(1, 512).min(sample_count);
        let start_seconds = episode.samples.first().map(|sample| sample.seconds);
        let end_seconds = episode.samples.last().map(|sample| sample.seconds);
        let trace_range = TraceRange {
            start_seconds: start_seconds.unwrap_or_default(),
            end_seconds: end_seconds.or(start_seconds).unwrap_or_default(),
            buckets: bucket_count,
        };
        let mut lanes = Vec::new();
        for joint in SO101_JOINTS {
            let mut series = Vec::new();
            for (arm_index, arm) in episode.start.arms.iter().enumerate() {
                let Some(joint_index) = arm.joints.iter().position(|name| name == joint.name())
                else {
                    continue;
                };
                let mut reduced = vec![TraceBucket::default(); bucket_count];
                if bucket_count > 0 {
                    for sample in &episode.samples {
                        let bucket = trace_range.bucket(sample.seconds);
                        reduced[bucket]
                            .observe(sample.seconds, sample.normalized[arm_index][joint_index]);
                    }
                }
                series.push(TimelineTraceSeries {
                    arm: arm.id.clone(),
                    points: reduced
                        .into_iter()
                        .flat_map(TraceBucket::into_points)
                        .collect(),
                });
            }
            lanes.push(TimelineTraceLane {
                joint: joint.name().to_owned(),
                series,
            });
        }
        Ok(TimelineTraces {
            episode: id.to_owned(),
            samples: sample_count as u64,
            buckets: bucket_count,
            start_seconds,
            end_seconds,
            lanes,
        })
    }

    pub fn video_metadata(&self, id: &str) -> Result<Vec<VideoAssetMetadata>, EpisodeError> {
        self.get(id)?;
        self.video_metadata_for_existing(id)
    }

    fn video_metadata_for_existing(
        &self,
        id: &str,
    ) -> Result<Vec<VideoAssetMetadata>, EpisodeError> {
        let relation = Table::VideoAssets.qualified_name(self.project_store.namespace());
        let id = sql::string_literal(id);
        self.project_store.run_blocking(move |connection| {
            if !table_exists(connection, &relation)? { return Ok(Vec::new()); }
            let result = run_query(connection, &format!("SELECT {}, {}, {}, {}, {}, {}, {}, {} FROM {relation} WHERE {} = {id} ORDER BY {}, {}", db::STREAM_ID, db::CAMERA_ID, db::WIDTH, db::HEIGHT, db::FPS, db::FRAMES, db::SEGMENT_INDEX, db::TIMELINE_OFFSET_NS, db::EPISODE_ID, db::STREAM_ID, db::SEGMENT_INDEX), 10_000)?;
            result.rows.into_iter().map(|row| video_metadata_row(&row)).collect()
        }).map_err(EpisodeError::Invalid)
    }

    fn try_list(&self) -> Result<Vec<EpisodeSummary>, EpisodeError> {
        let relation = Table::Episodes.qualified_name(self.project_store.namespace());
        self.project_store
            .run_blocking(move |connection| {
                if !table_exists(connection, &relation)? {
                    return Ok(Vec::new());
                }
                let result = run_query(
                    connection,
                    &format!(
                        "SELECT {}, {} FROM {relation} ORDER BY {}",
                        db::METADATA_JSON,
                        db::FRAMES,
                        db::STARTED_MS
                    ),
                    100_000,
                )?;
                result
                    .rows
                    .into_iter()
                    .map(|row| summary_row(&row))
                    .collect()
            })
            .map_err(EpisodeError::Invalid)
    }

    fn replace_completion(&self, episode: &CompletedEpisode) -> Result<(), EpisodeError> {
        let episode = episode.clone();
        let namespace = self.project_store.namespace().to_owned();
        self.project_store
            .run_blocking(move |connection| {
                let relation = Table::Episodes.qualified_name(&namespace);
                execute_update_sql(
                    connection,
                    &sql::delete_episode(&relation, &episode.start.id),
                )?;
                ingest_batch(
                    connection,
                    &relation,
                    episode_completion_batch(
                        &episode.start,
                        &episode.completion,
                        episode.samples.len() as u64,
                    )
                    .map_err(|error| error.to_string())?,
                )
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
                execute_update_sql(
                    connection,
                    &sql::delete_episode(&sample_relation, &episode.start.id),
                )?;
                execute_update_sql(
                    connection,
                    &sql::delete_episode(&episode_relation, &episode.start.id),
                )?;
                let mut encoder = SpiralEpisodeEncoder::new(episode.start.clone())
                    .map_err(|error| error.to_string())?;
                let mut rows = Vec::new();
                for sample in &episode.samples {
                    rows.extend(encoder.encode(sample).map_err(|error| error.to_string())?);
                    if rows.len() >= 512 {
                        ingest_batch(
                            connection,
                            &sample_relation,
                            joint_samples_batch(&rows).map_err(|error| error.to_string())?,
                        )?;
                        rows.clear();
                    }
                }
                if !rows.is_empty() {
                    ingest_batch(
                        connection,
                        &sample_relation,
                        joint_samples_batch(&rows).map_err(|error| error.to_string())?,
                    )?;
                }
                ingest_batch(
                    connection,
                    &episode_relation,
                    episode_completion_batch(&episode.start, &episode.completion, encoder.frames())
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

pub trait EpisodeWriteSession: Send {
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
    pub fn from_session(session: impl EpisodeWriteSession + 'static) -> Self {
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
        &self.session.start().id
    }
    pub fn dataset_id(&self) -> &str {
        &self.session.start().dataset_id
    }
    pub fn started_ms(&self) -> i64 {
        self.session.start().started_ms
    }
    pub fn push(&self, sample: Sample) -> bool {
        self.session.push(sample)
    }
    pub fn checkpoint(&self, completion: EpisodeCompletion) -> Result<EpisodeCheckpoint, String> {
        self.session.checkpoint(completion)
    }
    pub fn failure(&self) -> Option<String> {
        self.session.failure()
    }
    pub fn finish(self, completion: EpisodeCompletion) -> Result<EpisodeSummary, EpisodeError> {
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
            EpisodeCompletion {
                duration_seconds: completion.duration_seconds(),
                recording_gaps: completion.recording_gaps().to_vec(),
                max_follow_error_degrees: completion.max_follow_error_degrees(),
                discarded: false,
                condition: completion.condition().to_vec(),
            },
        )
    }

    fn failure(&self) -> Option<String> {
        EpisodeWriter::failure(self)
    }

    fn finish(self: Box<Self>, completion: RecordingCompletion) -> Result<EpisodeSummary, String> {
        EpisodeWriter::finish(
            *self,
            EpisodeCompletion {
                duration_seconds: completion.duration_seconds(),
                recording_gaps: completion.recording_gaps().to_vec(),
                max_follow_error_degrees: completion.max_follow_error_degrees(),
                discarded: completion.discarded(),
                condition: completion.condition().to_vec(),
            },
        )
        .map_err(|error| error.to_string())
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
        if !sample.seconds.is_finite() || sample.seconds < 0.0 {
            return self.fail("sample time must be finite and non-negative");
        }
        let next = sample.seconds.to_bits();
        let mut previous = self.last_queued_seconds.load(Ordering::Relaxed);
        loop {
            if previous != NO_SAMPLE_TIMESTAMP_BITS && sample.seconds < f64::from_bits(previous) {
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
        lock(&self.shared.failure).clone()
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
                replace_episode_blocking(connection, &namespace, &completed, &[])
            })
            .map_err(EpisodeError::Invalid)?;
        Ok(summary(
            &completed.start,
            &completed.completion,
            completed.samples.len() as u64,
        ))
    }
}

impl SpiralEpisodeWriter {
    fn fail(&self, message: &str) -> bool {
        let mut failure = lock(&self.shared.failure);
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
                        *lock(&self.shared.failure) = Some(error.clone());
                        return Err(error);
                    }
                    encoder.encode(&sample).map_err(|error| {
                        let error = error.to_string();
                        *lock(&self.shared.failure) = Some(error.clone());
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
                        replace_episode_blocking(connection, &namespace, &completed, &[])
                    });
                    if let Err(error) = &result {
                        *lock(&self.shared.failure) = Some(error.clone());
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

fn decode_samples(
    start: &EpisodeStart,
    rows: Vec<JsonMap<String, JsonValue>>,
) -> Result<Vec<Sample>, String> {
    let mut frames = BTreeMap::<u64, SampleBuilder>::new();
    for row in rows {
        let index = json_u64(&row, db::SAMPLE_INDEX)?;
        let stream = json_str(&row, db::STREAM_ID)?;
        let arm = start
            .arms
            .iter()
            .position(|arm| arm.id == stream)
            .ok_or_else(|| {
                format!(
                    "episode {:?} has sample for unknown stream {stream:?}",
                    start.id
                )
            })?;
        let frame = frames.entry(index).or_insert_with(|| SampleBuilder {
            normalized: vec![None; start.arms.len()],
            raw: vec![None; start.arms.len()],
            velocity: vec![None; start.arms.len()],
            effort: vec![None; start.arms.len()],
            action: vec![None; start.arms.len()],
            ..SampleBuilder::default()
        });
        let seconds = json_u64(&row, db::EPISODE_TIME_NS)? as f64 / 1_000_000_000.0;
        if frame
            .seconds
            .is_some_and(|existing| existing.to_bits() != seconds.to_bits())
        {
            return Err(format!(
                "episode {:?} sample {index} has inconsistent clocks",
                start.id
            ));
        }
        frame.seconds = Some(seconds);
        frame.normalized[arm] = Some(json_optional_f64s(&row, db::STATE_POSITION)?);
        frame.raw[arm] = Some(json_i64s(&row, db::RAW_POSITION)?);
        frame.velocity[arm] = json_optional_optional_f64s(&row, db::VELOCITY)?;
        frame.effort[arm] = json_optional_optional_f64s(&row, db::EFFORT)?;
        frame.action[arm] = json_optional_optional_f64s(&row, db::ACTION_POSITION)?;
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
                episode_id: &start.id,
                sample_index: index,
            };
            let normalized = location.require(frame.normalized, "state")?;
            let raw = location.require(frame.raw, "raw state")?;
            let velocity = location.all_or_none(frame.velocity, "velocity")?;
            let effort = location.all_or_none(frame.effort, "effort")?;
            Ok(Sample {
                seconds: frame
                    .seconds
                    .ok_or_else(|| format!("episode {:?} sample {index} has no clock", start.id))?,
                normalized,
                raw,
                velocity,
                effort,
                action: frame.action,
                follow_error_degrees: frame.follow_error,
            })
        })
        .collect()
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

fn summary_row(row: &JsonMap<String, JsonValue>) -> Result<EpisodeSummary, String> {
    let metadata = row
        .get(db::METADATA_JSON)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| "episode row has no metadata_json".to_owned())?;
    let (start, completion): (EpisodeStart, EpisodeCompletion) =
        serde_json::from_str(metadata).map_err(|error| error.to_string())?;
    Ok(summary(&start, &completion, json_u64(row, db::FRAMES)?))
}

fn summary(start: &EpisodeStart, completion: &EpisodeCompletion, frames: u64) -> EpisodeSummary {
    EpisodeSummary {
        id: start.id.clone(),
        dataset_id: start.dataset_id.clone(),
        arms: start.arms.clone(),
        frames,
        duration_seconds: completion.duration_seconds,
        recording_gaps: completion.recording_gaps.clone(),
        sample_hz: start.sample_hz,
        started_ms: start.started_ms,
        source: start.source.clone(),
        max_follow_error_degrees: completion.max_follow_error_degrees,
        discarded: completion.discarded,
        problem: None,
    }
}

fn render_timeline_sample(start: &EpisodeStart, sample: &Sample, frame: u64) -> TimelineSample {
    let arms = start
        .arms
        .iter()
        .enumerate()
        .map(|(arm_index, arm)| {
            let values = |values: Option<&Vec<Vec<Option<f64>>>>| {
                values.map(|values| {
                    arm.joints
                        .iter()
                        .enumerate()
                        .map(|(joint_index, joint)| (joint.clone(), values[arm_index][joint_index]))
                        .collect()
                })
            };
            TimelineArmSample {
                arm: arm.id.clone(),
                normalized: arm
                    .joints
                    .iter()
                    .enumerate()
                    .map(|(joint_index, joint)| {
                        (joint.clone(), sample.normalized[arm_index][joint_index])
                    })
                    .collect(),
                raw: arm
                    .joints
                    .iter()
                    .enumerate()
                    .map(|(joint_index, joint)| (joint.clone(), sample.raw[arm_index][joint_index]))
                    .collect(),
                velocity: values(sample.velocity.as_ref()),
                effort: values(sample.effort.as_ref()),
                action: sample
                    .action
                    .get(arm_index)
                    .and_then(Option::as_ref)
                    .map(|values| {
                        arm.joints
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
        seconds: sample.seconds,
        follow_error_degrees: sample.follow_error_degrees,
        arms,
    }
}

fn video_metadata_row(row: &JsonMap<String, JsonValue>) -> Result<VideoAssetMetadata, String> {
    let fps = json_u64(row, db::FPS)? as u32;
    let frames = json_u64(row, db::FRAMES)?;
    let timing = VideoTiming::new(fps, frames);
    Ok(VideoAssetMetadata::new(
        json_str(row, db::STREAM_ID)?.to_owned(),
        json_str(row, db::CAMERA_ID)?.to_owned(),
        timing.duration_seconds(),
    )
    .with_size(VideoSize::new(
        json_u64(row, db::WIDTH)? as u32,
        json_u64(row, db::HEIGHT)? as u32,
    ))
    .with_timing(timing)
    .with_segment(VideoSegment::new(
        json_u64(row, db::SEGMENT_INDEX)? as u32,
        row.get(db::TIMELINE_OFFSET_NS)
            .and_then(JsonValue::as_u64)
            .map(|value| value as f64 / 1_000_000_000.0),
    )))
}

fn json_str<'a>(row: &'a JsonMap<String, JsonValue>, name: &str) -> Result<&'a str, String> {
    row.get(name)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| format!("row has no string {name}"))
}
fn json_u64(row: &JsonMap<String, JsonValue>, name: &str) -> Result<u64, String> {
    row.get(name)
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| format!("row has no unsigned {name}"))
}
fn json_i64s(row: &JsonMap<String, JsonValue>, name: &str) -> Result<Vec<i64>, String> {
    row.get(name)
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
fn json_optional_f64s(
    row: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<Vec<Option<f64>>, String> {
    row.get(name)
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
fn json_optional_optional_f64s(
    row: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<Option<Vec<Option<f64>>>, String> {
    match row.get(name) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(_) => json_optional_f64s(row, name).map(Some),
    }
}

#[derive(Clone, Copy)]
struct TraceRange {
    start_seconds: f64,
    end_seconds: f64,
    buckets: usize,
}

impl TraceRange {
    fn bucket(self, seconds: f64) -> usize {
        if self.buckets <= 1 || self.end_seconds <= self.start_seconds {
            0
        } else {
            ((((seconds - self.start_seconds) / (self.end_seconds - self.start_seconds))
                .clamp(0.0, 1.0)
                * self.buckets as f64)
                .floor() as usize)
                .min(self.buckets - 1)
        }
    }
}
fn episode_number(id: &str) -> Option<u64> {
    id.strip_prefix("episode_")?.parse().ok()
}
fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
pub fn validate_episode_id(id: &str) -> Result<(), EpisodeError> {
    (!id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_')))
    .then_some(())
    .ok_or_else(|| EpisodeError::InvalidId(id.to_owned()))
}
pub fn build_arms(arms: &[(String, Vec<String>)]) -> Result<Vec<EpisodeArm>, EpisodeError> {
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
            Ok(EpisodeArm {
                id: id.clone(),
                robot_type: None,
                joints: joints.clone(),
            })
        })
        .collect()
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
