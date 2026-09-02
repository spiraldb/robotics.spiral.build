//! Columnar episode rows at the Spiral Robotics → Spiral boundary.
//!
//! This module deliberately stops at Arrow record batches. The shared Spiral
//! project store owns connections while the hardware loop only needs a
//! deterministic conversion from native samples into stable relational keys.

use std::sync::Arc;

use crate::db::{SCHEMA_VERSION, column as db};
use arrow_array::builder::{Float64Builder, Int64Builder, ListBuilder};
use arrow_array::{
    ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt32Array,
    UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use spiral_robotics_common::recording::{EpisodeArm, RecordingGap};

use super::episodes::{
    ArmCondition, ArmConditionEffortExtremes, ArmConditionSummary, ArmConditionThermalExtremes,
    EpisodeCompletion, EpisodeError, EpisodeStart, JointMotorLimits, JointVoltageRange, Sample,
};
use super::video::VideoSize;
use crate::db::{
    ArrowColumns, ListColumnBuilder as _, OptionalFloatListColumn, StringListColumn,
    UnsignedListColumn,
};

const NANOSECONDS_PER_SECOND: f64 = 1_000_000_000.0;

/// Per-joint condition channels, in the order their columns are written.
const CONDITION_COLUMNS: [&str; 10] = [
    db::START_TEMPERATURE_CELSIUS,
    db::START_VOLTAGE_VOLTS,
    db::LIMIT_MAX_TEMPERATURE_CELSIUS,
    db::LIMIT_MIN_VOLTAGE_VOLTS,
    db::LIMIT_MAX_VOLTAGE_VOLTS,
    db::LIMIT_MAX_LOAD,
    db::PEAK_TEMPERATURE_CELSIUS,
    db::MIN_VOLTAGE_VOLTS,
    db::PEAK_LOAD,
    db::PEAK_CURRENT_AMPS,
];

pub(super) struct EpisodeSchema;

impl EpisodeSchema {
    /// Schema for the managed `datasets` relation.
    pub(super) fn datasets() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(db::DATASET_ID, DataType::Utf8, false),
            Field::new(db::NAME, DataType::Utf8, false),
            Field::new(db::CREATED_MS, DataType::Int64, false),
            Field::new(db::DESCRIPTION, DataType::Utf8, true),
        ]))
    }

    pub(super) fn episodes() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(db::EPISODE_ID, DataType::Utf8, false),
            Field::new(db::DATASET_ID, DataType::Utf8, false),
            Field::new(db::SCHEMA_VERSION, DataType::UInt32, false),
            Field::new(db::STARTED_MS, DataType::Int64, false),
            Field::new(db::SAMPLE_HZ, DataType::Float64, false),
            Field::new(db::SOURCE, DataType::Utf8, false),
            Field::new(db::NOTE, DataType::Utf8, true),
            Field::new(db::FRAMES, DataType::UInt64, false),
            Field::new(db::DURATION_NS, DataType::UInt64, false),
            Field::new(db::MAX_FOLLOW_ERROR_DEGREES, DataType::Float64, true),
            Field::new(db::DISCARDED, DataType::Boolean, false),
            Field::new(
                db::RECORDING_GAP_AFTER_NS,
                UnsignedListColumn::data_type(),
                false,
            ),
            Field::new(
                db::RECORDING_GAP_DURATION_NS,
                UnsignedListColumn::data_type(),
                false,
            ),
        ]))
    }

    pub(super) fn episode_streams() -> SchemaRef {
        let condition = CONDITION_COLUMNS
            .into_iter()
            .map(|name| Field::new(name, OptionalFloatListColumn::data_type(), true));
        Arc::new(Schema::new(
            [
                Field::new(db::EPISODE_ID, DataType::Utf8, false),
                Field::new(db::STREAM_ID, DataType::Utf8, false),
                Field::new(db::ORDINAL, DataType::UInt32, false),
                Field::new(db::MODALITY, DataType::Utf8, false),
                Field::new(db::ROBOT_TYPE, DataType::Utf8, true),
                Field::new(db::JOINT_NAMES, StringListColumn::data_type(), true),
                Field::new(db::CAMERA_ID, DataType::Utf8, true),
                Field::new(db::WIDTH, DataType::UInt32, true),
                Field::new(db::HEIGHT, DataType::UInt32, true),
                Field::new(db::FPS, DataType::UInt32, true),
            ]
            .into_iter()
            .chain(condition)
            .collect::<Vec<_>>(),
        ))
    }

    pub(super) fn joint_samples() -> SchemaRef {
        let nullable_f64_list =
            || DataType::List(Arc::new(Field::new("item", DataType::Float64, true)));
        Arc::new(Schema::new(vec![
            Field::new(db::EPISODE_ID, DataType::Utf8, false),
            Field::new(db::STREAM_ID, DataType::Utf8, false),
            Field::new(db::SAMPLE_INDEX, DataType::UInt64, false),
            Field::new(db::EPISODE_TIME_NS, DataType::UInt64, false),
            Field::new(db::STATE_VALID, DataType::Boolean, false),
            Field::new(db::ACTION_VALID, DataType::Boolean, false),
            Field::new(db::STATE_POSITION, nullable_f64_list(), false),
            Field::new(db::ACTION_POSITION, nullable_f64_list(), true),
            Field::new(
                db::RAW_POSITION,
                DataType::List(Arc::new(Field::new("item", DataType::Int64, false))),
                false,
            ),
            Field::new(db::VELOCITY, nullable_f64_list(), true),
            Field::new(db::EFFORT, nullable_f64_list(), true),
            Field::new(db::FOLLOW_ERROR_DEGREES, DataType::Float64, true),
        ]))
    }
}

/// Build the owning dataset row. Dataset IDs double as their initial display
/// name until richer dataset metadata is edited in a future workflow.
impl EpisodeStart {
    pub(super) fn dataset_batch(&self) -> Result<RecordBatch, EpisodeError> {
        let start = self;
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(vec![start.dataset_id()])),
            Arc::new(StringArray::from(vec![start.dataset_id()])),
            Arc::new(Int64Array::from(vec![start.started_ms()])),
            Arc::new(StringArray::from(vec![None::<&str>])),
        ];
        RecordBatch::try_new(EpisodeSchema::datasets(), columns).map_err(|error| {
            EpisodeError::invalid(format!("building Spiral dataset batch: {error}"))
        })
    }
}

/// One row destined for the managed `joint_samples` relation.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct JointSampleRow {
    key: JointSampleKey,
    validity: SampleValidity,
    state: JointStateValues,
    action_position: Option<Vec<Option<f64>>>,
    follow_error_degrees: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
struct JointSampleKey {
    episode_id: String,
    stream_id: String,
    sample_index: u64,
    episode_time_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SampleValidity {
    state: bool,
    action: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct JointStateValues {
    position: Vec<Option<f64>>,
    raw_position: Vec<i64>,
    velocity: Option<Vec<Option<f64>>>,
    effort: Option<Vec<Option<f64>>>,
}

/// Camera stream metadata stored beside joint stream descriptions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VideoStreamRow {
    episode_id: String,
    stream_id: String,
    properties: VideoStreamProperties,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VideoStreamProperties {
    camera_id: String,
    size: VideoSize,
    fps: u32,
}

impl VideoStreamProperties {
    pub(super) fn new(camera_id: String, size: VideoSize, fps: u32) -> Self {
        Self {
            camera_id,
            size,
            fps,
        }
    }

    fn camera_id(&self) -> &str {
        &self.camera_id
    }
}

impl VideoStreamRow {
    pub(super) fn new(
        episode_id: String,
        stream_id: String,
        properties: VideoStreamProperties,
    ) -> Self {
        Self {
            episode_id,
            stream_id,
            properties,
        }
    }
}

/// Stateful conversion from control-loop frames to relational stream rows.
///
/// Every control frame receives one `sample_index`; every arm in that frame
/// receives a row with the same index. The resulting stable key is
/// `(episode_id, stream_id, sample_index)`.
pub(super) struct SpiralEpisodeEncoder {
    start: EpisodeStart,
    next_sample_index: u64,
}

impl SpiralEpisodeEncoder {
    pub(super) fn new(start: EpisodeStart) -> Result<Self, EpisodeError> {
        if start.arms().is_empty() {
            return Err(EpisodeError::invalid(
                "a Spiral episode needs at least one arm stream",
            ));
        }
        Ok(Self {
            start,
            next_sample_index: 0,
        })
    }

    pub(super) fn frames(&self) -> u64 {
        self.next_sample_index
    }

    /// Convert one control frame without performing I/O.
    pub(super) fn encode(&mut self, sample: &Sample) -> Result<Vec<JointSampleRow>, EpisodeError> {
        sample.validate_for(&self.start)?;
        let episode_time_ns = EpisodeTimestamp::nanoseconds(sample.seconds(), "sample time")?;
        let sample_index = self.next_sample_index;

        let rows = self
            .start
            .arms()
            .iter()
            .enumerate()
            .map(|(arm_index, arm)| JointSampleRow {
                key: JointSampleKey {
                    episode_id: self.start.id().to_owned(),
                    stream_id: arm.id().to_owned(),
                    sample_index,
                    episode_time_ns,
                },
                validity: SampleValidity {
                    state: sample.normalized()[arm_index]
                        .iter()
                        .all(|value| value.is_some_and(f64::is_finite)),
                    action: sample
                        .action()
                        .get(arm_index)
                        .and_then(Option::as_ref)
                        .is_some_and(|values| {
                            values.iter().all(|value| value.is_some_and(f64::is_finite))
                        }),
                },
                state: JointStateValues {
                    position: sample.normalized()[arm_index].clone(),
                    raw_position: sample.raw()[arm_index].clone(),
                    velocity: sample.velocity().map(|values| values[arm_index].clone()),
                    effort: sample.effort().map(|values| values[arm_index].clone()),
                },
                action_position: sample.action().get(arm_index).cloned().flatten(),
                follow_error_degrees: sample.follow_error_degrees(),
            })
            .collect();

        self.next_sample_index = self
            .next_sample_index
            .checked_add(1)
            .ok_or_else(|| EpisodeError::invalid("episode sample index overflowed u64"))?;
        Ok(rows)
    }
}

/// One row of the managed `episodes` relation, decoded from typed columns.
///
/// The relation is the whole record: arms come from the episode's joint stream
/// rows, and every other field is a column of the episode row itself. Nothing
/// is read out of a serialized wire type.
#[derive(Clone, Debug)]
pub(super) struct StoredEpisodeRow {
    start: EpisodeStart,
    completion: EpisodeCompletion,
    frames: u64,
}

impl StoredEpisodeRow {
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

    pub(super) const fn start(&self) -> &EpisodeStart {
        &self.start
    }

    pub(super) const fn completion(&self) -> &EpisodeCompletion {
        &self.completion
    }

    pub(super) const fn frames(&self) -> u64 {
        self.frames
    }

    pub(super) fn into_parts(self) -> (EpisodeStart, EpisodeCompletion, u64) {
        (self.start, self.completion, self.frames)
    }

    pub(super) fn decode(
        batch: &RecordBatch,
        row: usize,
        streams: &EpisodeStreamCatalog,
    ) -> Result<Self, String> {
        let columns = ArrowColumns::new(batch, "episode");
        let id = columns.text(db::EPISODE_ID, row)?.to_owned();
        let start = EpisodeStart::new(id.clone(), columns.text(db::DATASET_ID, row)?.to_owned())
            .with_timing(
                columns.timestamp(db::STARTED_MS, row)?,
                columns.float(db::SAMPLE_HZ, row)?,
            )
            .with_source(
                columns.text(db::SOURCE, row)?.to_owned(),
                columns.optional_text(db::NOTE, row)?.map(str::to_owned),
            )
            .with_arms(streams.arms(&id), streams.start_condition(&id));
        let gaps = RecordingGapColumns::decode(&columns, row)?;
        let completion = EpisodeCompletion::new(
            columns.unsigned(db::DURATION_NS, row)? as f64 / NANOSECONDS_PER_SECOND,
            gaps,
            streams.completion_condition(&id),
        )
        .with_max_follow_error_degrees(columns.optional_float(db::MAX_FOLLOW_ERROR_DEGREES, row)?)
        .with_discarded(columns.boolean(db::DISCARDED, row)?);
        Ok(Self {
            start,
            completion,
            frames: columns.unsigned(db::FRAMES, row)?,
        })
    }

    /// Encode whole episode rows stamped with the current [`SCHEMA_VERSION`].
    pub(super) fn batch(rows: &[Self]) -> Result<RecordBatch, EpisodeError> {
        let mut after_ns = UnsignedListColumn::builder();
        let mut duration_ns = UnsignedListColumn::builder();
        let mut durations = Vec::with_capacity(rows.len());
        for row in rows {
            let gaps = RecordingGapColumns::encode(row.completion.recording_gaps())?;
            after_ns.append_optional_list(Some(gaps.after_ns()));
            duration_ns.append_optional_list(Some(gaps.duration_ns()));
            durations.push(EpisodeTimestamp::nanoseconds(
                row.completion.duration_seconds(),
                "episode duration",
            )?);
        }
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.start.id()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.start.dataset_id()),
            )),
            Arc::new(UInt32Array::from(vec![SCHEMA_VERSION; rows.len()])),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|row| row.start.started_ms()),
            )),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|row| row.start.sample_hz()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.start.source()),
            )),
            Arc::new(StringArray::from_iter(
                rows.iter().map(|row| row.start.note()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|row| row.frames),
            )),
            Arc::new(UInt64Array::from(durations)),
            Arc::new(Float64Array::from_iter(
                rows.iter()
                    .map(|row| row.completion.max_follow_error_degrees()),
            )),
            Arc::new(BooleanArray::from_iter(
                rows.iter().map(|row| Some(row.completion.discarded())),
            )),
            Arc::new(after_ns.finish()),
            Arc::new(duration_ns.finish()),
        ];
        RecordBatch::try_new(EpisodeSchema::episodes(), columns).map_err(|error| {
            EpisodeError::invalid(format!("building Spiral episode batch: {error}"))
        })
    }
}

/// Build the single row that makes an episode visible to readers.
impl EpisodeCompletion {
    pub(super) fn episode_batch(
        &self,
        start: &EpisodeStart,
        frames: u64,
    ) -> Result<RecordBatch, EpisodeError> {
        StoredEpisodeRow::batch(&[StoredEpisodeRow::new(start.clone(), self.clone(), frames)])
    }
}

/// The two parallel gap lists an episode row carries.
struct RecordingGapColumns {
    after_ns: Vec<u64>,
    duration_ns: Vec<u64>,
}

impl RecordingGapColumns {
    fn after_ns(&self) -> &[u64] {
        &self.after_ns
    }

    fn duration_ns(&self) -> &[u64] {
        &self.duration_ns
    }

    fn encode(gaps: &[RecordingGap]) -> Result<Self, EpisodeError> {
        Ok(Self {
            after_ns: gaps
                .iter()
                .map(|gap| {
                    EpisodeTimestamp::nanoseconds(gap.after_seconds(), "recording gap start")
                })
                .collect::<Result<Vec<_>, _>>()?,
            duration_ns: gaps
                .iter()
                .map(|gap| {
                    EpisodeTimestamp::nanoseconds(gap.duration_seconds(), "recording gap duration")
                })
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    fn decode(columns: &ArrowColumns<'_>, row: usize) -> Result<Vec<RecordingGap>, String> {
        let after = columns.unsigned_list(db::RECORDING_GAP_AFTER_NS, row)?;
        let duration = columns.unsigned_list(db::RECORDING_GAP_DURATION_NS, row)?;
        if after.len() != duration.len() {
            return Err(format!(
                "stored episode has {} recording gap starts but {} durations",
                after.len(),
                duration.len()
            ));
        }
        Ok(after
            .into_iter()
            .zip(duration)
            .map(|(after, duration)| {
                RecordingGap::new(
                    after as f64 / NANOSECONDS_PER_SECOND,
                    duration as f64 / NANOSECONDS_PER_SECOND,
                )
            })
            .collect())
    }
}

/// Which kind of data one episode stream carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StreamModality {
    Joint,
    Video,
}

impl StreamModality {
    const fn persisted(self) -> &'static str {
        match self {
            Self::Joint => "joint",
            Self::Video => "video",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "joint" => Ok(Self::Joint),
            "video" => Ok(Self::Video),
            other => Err(format!("stored stream modality {other:?} is not known")),
        }
    }
}

/// Identity of one stream inside one episode.
///
/// `ordinal` is the stream's position within its own modality. Joint streams
/// are reconstructed in that order because it is the order their values appear
/// in every recorded frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EpisodeStreamKey {
    episode_id: String,
    stream_id: String,
    ordinal: u32,
}

impl EpisodeStreamKey {
    pub(super) const fn new(episode_id: String, stream_id: String, ordinal: u32) -> Self {
        Self {
            episode_id,
            stream_id,
            ordinal,
        }
    }
}

/// One row of the managed `episode_streams` relation in either modality.
#[derive(Clone, Debug)]
pub(super) struct EpisodeStreamRow {
    key: EpisodeStreamKey,
    modality: StreamModality,
    robot_type: Option<String>,
    joints: Option<Vec<String>>,
    start: Option<ArmCondition>,
    peak: Option<ArmConditionSummary>,
    video: Option<VideoStreamProperties>,
}

impl EpisodeStreamRow {
    pub(super) fn joint(key: EpisodeStreamKey, arm: &EpisodeArm) -> Self {
        Self {
            key,
            modality: StreamModality::Joint,
            robot_type: arm.robot_type().map(str::to_owned),
            joints: Some(arm.joints().to_vec()),
            start: None,
            peak: None,
            video: None,
        }
    }

    pub(super) fn video(key: EpisodeStreamKey, properties: VideoStreamProperties) -> Self {
        Self {
            key,
            modality: StreamModality::Video,
            robot_type: None,
            joints: None,
            start: None,
            peak: None,
            video: Some(properties),
        }
    }

    /// Attach the motor readings recorded for this arm. An observation that
    /// measured nothing is dropped so the columns stay null rather than
    /// claiming an all-empty reading.
    pub(super) fn with_condition(
        mut self,
        start: Option<&ArmCondition>,
        peak: Option<&ArmConditionSummary>,
    ) -> Self {
        self.start = start.filter(|value| !value.is_empty()).cloned();
        self.peak = peak.filter(|value| !value.is_empty()).cloned();
        self
    }

    fn arm(&self) -> EpisodeArm {
        EpisodeArm::new(
            self.key.stream_id.clone(),
            self.robot_type.clone(),
            self.joints.clone().unwrap_or_default(),
        )
    }

    fn limit_column(&self, read: fn(JointMotorLimits) -> Option<f64>) -> Option<Vec<Option<f64>>> {
        self.start
            .as_ref()
            .map(|start| start.limits().iter().copied().map(read).collect())
    }

    fn condition_columns(&self) -> [Option<Vec<Option<f64>>>; 10] {
        let start = |read: fn(&ArmCondition) -> &[Option<f64>]| {
            self.start.as_ref().map(|value| read(value).to_vec())
        };
        let peak = |read: fn(&ArmConditionSummary) -> &[Option<f64>]| {
            self.peak.as_ref().map(|value| read(value).to_vec())
        };
        [
            start(ArmCondition::temperature_celsius),
            start(ArmCondition::voltage_volts),
            self.limit_column(JointMotorLimits::max_temperature_celsius),
            self.limit_column(JointMotorLimits::min_voltage_volts),
            self.limit_column(JointMotorLimits::max_voltage_volts),
            self.limit_column(JointMotorLimits::max_load),
            peak(ArmConditionSummary::peak_temperature_celsius),
            peak(ArmConditionSummary::min_voltage_volts),
            peak(ArmConditionSummary::peak_load),
            peak(ArmConditionSummary::peak_current_amps),
        ]
    }

    pub(super) fn decode(batch: &RecordBatch, row: usize) -> Result<Self, String> {
        let columns = ArrowColumns::new(batch, "episode stream");
        let modality = StreamModality::parse(columns.text(db::MODALITY, row)?)?;
        let channel = |name: &str| columns.optional_float_list(name, row);
        let start = StreamConditionColumns::start(
            columns.text(db::STREAM_ID, row)?,
            [
                channel(CONDITION_COLUMNS[0])?,
                channel(CONDITION_COLUMNS[1])?,
                channel(CONDITION_COLUMNS[2])?,
                channel(CONDITION_COLUMNS[3])?,
                channel(CONDITION_COLUMNS[4])?,
                channel(CONDITION_COLUMNS[5])?,
            ],
        );
        let peak = StreamConditionColumns::peak(
            columns.text(db::STREAM_ID, row)?,
            [
                channel(CONDITION_COLUMNS[6])?,
                channel(CONDITION_COLUMNS[7])?,
                channel(CONDITION_COLUMNS[8])?,
                channel(CONDITION_COLUMNS[9])?,
            ],
        );
        let video = match modality {
            StreamModality::Joint => None,
            StreamModality::Video => Some(VideoStreamProperties::new(
                columns
                    .optional_text(db::CAMERA_ID, row)?
                    .unwrap_or_default()
                    .to_owned(),
                VideoSize::new(
                    columns.optional_unsigned(db::WIDTH, row)?.unwrap_or(0) as u32,
                    columns.optional_unsigned(db::HEIGHT, row)?.unwrap_or(0) as u32,
                ),
                columns.optional_unsigned(db::FPS, row)?.unwrap_or(0) as u32,
            )),
        };
        Ok(Self {
            key: EpisodeStreamKey::new(
                columns.text(db::EPISODE_ID, row)?.to_owned(),
                columns.text(db::STREAM_ID, row)?.to_owned(),
                columns.ordinal(db::ORDINAL, row)?,
            ),
            modality,
            robot_type: columns
                .optional_text(db::ROBOT_TYPE, row)?
                .map(str::to_owned),
            joints: matches!(modality, StreamModality::Joint)
                .then(|| columns.text_list(db::JOINT_NAMES, row))
                .transpose()?,
            start,
            peak,
            video,
        })
    }

    /// Decode the columns needed to assemble an episode-list row. Conditions
    /// and video properties are deliberately absent from the catalog query.
    pub(super) fn decode_catalog(batch: &RecordBatch, row: usize) -> Result<Self, String> {
        let columns = ArrowColumns::new(batch, "episode stream catalog");
        let modality = StreamModality::parse(columns.text(db::MODALITY, row)?)?;
        Ok(Self {
            key: EpisodeStreamKey::new(
                columns.text(db::EPISODE_ID, row)?.to_owned(),
                columns.text(db::STREAM_ID, row)?.to_owned(),
                columns.ordinal(db::ORDINAL, row)?,
            ),
            modality,
            robot_type: columns
                .optional_text(db::ROBOT_TYPE, row)?
                .map(str::to_owned),
            joints: matches!(modality, StreamModality::Joint)
                .then(|| columns.text_list(db::JOINT_NAMES, row))
                .transpose()?,
            start: None,
            peak: None,
            video: None,
        })
    }
}

/// Assemble the per-joint condition columns of one stream back into the arm
/// observations they were written from.
struct StreamConditionColumns;

impl StreamConditionColumns {
    fn start(arm: &str, channels: [Option<Vec<Option<f64>>>; 6]) -> Option<ArmCondition> {
        let [
            temperature,
            voltage,
            max_temperature,
            min_voltage,
            max_voltage,
            max_load,
        ] = channels;
        let limits = [&max_temperature, &min_voltage, &max_voltage, &max_load]
            .into_iter()
            .flatten()
            .map(Vec::len)
            .max()
            .unwrap_or(0);
        let limits = (0..limits)
            .map(|index| {
                let read = |channel: &Option<Vec<Option<f64>>>| {
                    channel
                        .as_ref()
                        .and_then(|values| values.get(index).copied().flatten())
                };
                JointMotorLimits::new(
                    read(&max_temperature),
                    JointVoltageRange::new(read(&min_voltage), read(&max_voltage)),
                    read(&max_load),
                )
            })
            .collect::<Vec<_>>();
        let condition = ArmCondition::new(arm.to_owned())
            .with_measurements(temperature.unwrap_or_default(), voltage.unwrap_or_default())
            .with_limits(limits);
        (!condition.is_empty()).then_some(condition)
    }

    fn peak(arm: &str, channels: [Option<Vec<Option<f64>>>; 4]) -> Option<ArmConditionSummary> {
        let [temperature, voltage, load, current] = channels;
        let summary = ArmConditionSummary::new(arm.to_owned()).with_extremes(
            ArmConditionThermalExtremes::new(
                temperature.unwrap_or_default(),
                voltage.unwrap_or_default(),
            ),
            ArmConditionEffortExtremes::new(load.unwrap_or_default(), current.unwrap_or_default()),
        );
        (!summary.is_empty()).then_some(summary)
    }
}

/// Describe every stream of an episode in one keyed relation.
pub(super) trait EpisodeStreamRows {
    fn batch(&self) -> Result<RecordBatch, EpisodeError>;
}

impl EpisodeStreamRows for [EpisodeStreamRow] {
    fn batch(&self) -> Result<RecordBatch, EpisodeError> {
        let rows = self;
        let mut joint_names = StringListColumn::builder();
        for row in rows {
            joint_names.append_optional_list(row.joints.as_deref());
        }
        let mut condition = std::array::from_fn::<_, 10, _>(|_| OptionalFloatListColumn::builder());
        for row in rows {
            for (builder, values) in condition.iter_mut().zip(row.condition_columns()) {
                builder.append_optional_list(values.as_deref());
            }
        }
        let video = |read: fn(&VideoStreamProperties) -> u32| {
            UInt32Array::from_iter(rows.iter().map(|row| row.video.as_ref().map(&read)))
        };
        let columns: Vec<ArrayRef> = [
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.key.episode_id.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.key.stream_id.as_str()),
            )),
            Arc::new(UInt32Array::from_iter_values(
                rows.iter().map(|row| row.key.ordinal),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.modality.persisted()),
            )),
            Arc::new(StringArray::from_iter(
                rows.iter().map(|row| row.robot_type.as_deref()),
            )),
            Arc::new(joint_names.finish()),
            Arc::new(StringArray::from_iter(rows.iter().map(|row| {
                row.video.as_ref().map(VideoStreamProperties::camera_id)
            }))),
            Arc::new(video(|properties| properties.size.width())),
            Arc::new(video(|properties| properties.size.height())),
            Arc::new(video(|properties| properties.fps)),
        ]
        .into_iter()
        .chain(
            condition
                .into_iter()
                .map(|mut builder| Arc::new(builder.finish()) as ArrayRef),
        )
        .collect();
        RecordBatch::try_new(EpisodeSchema::episode_streams(), columns).map_err(|error| {
            EpisodeError::invalid(format!("building Spiral stream batch: {error}"))
        })
    }
}

/// Every stream row a read observed, indexed back to its episode.
#[derive(Clone, Debug, Default)]
pub(super) struct EpisodeStreamCatalog {
    rows: Vec<EpisodeStreamRow>,
}

impl EpisodeStreamCatalog {
    pub(super) const fn new(rows: Vec<EpisodeStreamRow>) -> Self {
        Self { rows }
    }

    /// One episode's joint streams, in the order their values appear in a frame.
    fn joint_streams(&self, episode_id: &str) -> Vec<&EpisodeStreamRow> {
        let mut streams = self
            .rows
            .iter()
            .filter(|row| row.key.episode_id == episode_id && row.modality == StreamModality::Joint)
            .collect::<Vec<_>>();
        streams.sort_by(|left, right| {
            left.key
                .ordinal
                .cmp(&right.key.ordinal)
                .then_with(|| left.key.stream_id.cmp(&right.key.stream_id))
        });
        streams
    }

    pub(super) fn arms(&self, episode_id: &str) -> Vec<EpisodeArm> {
        self.joint_streams(episode_id)
            .into_iter()
            .map(EpisodeStreamRow::arm)
            .collect()
    }

    pub(super) fn start_condition(&self, episode_id: &str) -> Vec<ArmCondition> {
        self.joint_streams(episode_id)
            .into_iter()
            .filter_map(|row| row.start.clone())
            .collect()
    }

    pub(super) fn completion_condition(&self, episode_id: &str) -> Vec<ArmConditionSummary> {
        self.joint_streams(episode_id)
            .into_iter()
            .filter_map(|row| row.peak.clone())
            .collect()
    }
}

/// Describe every joint stream before its samples are appended.
impl EpisodeStart {
    pub(super) fn stream_rows(&self, completion: &EpisodeCompletion) -> Vec<EpisodeStreamRow> {
        self.arms()
            .iter()
            .enumerate()
            .map(|(index, arm)| {
                EpisodeStreamRow::joint(
                    EpisodeStreamKey::new(
                        self.id().to_owned(),
                        arm.id().to_owned(),
                        u32::try_from(index).unwrap_or(u32::MAX),
                    ),
                    arm,
                )
                .with_condition(
                    self.condition()
                        .iter()
                        .find(|entry| entry.arm() == arm.id()),
                    completion
                        .condition()
                        .iter()
                        .find(|entry| entry.arm() == arm.id()),
                )
            })
            .collect()
    }

    pub(super) fn streams_batch(
        &self,
        completion: &EpisodeCompletion,
    ) -> Result<RecordBatch, EpisodeError> {
        self.stream_rows(completion).batch()
    }
}

/// Describe camera streams using the same keyed stream table as robot joints.
pub(super) trait VideoStreamRows {
    fn batch(&self) -> Result<RecordBatch, EpisodeError>;
}

impl VideoStreamRows for [VideoStreamRow] {
    fn batch(&self) -> Result<RecordBatch, EpisodeError> {
        self.iter()
            .enumerate()
            .map(|(index, row)| {
                EpisodeStreamRow::video(
                    EpisodeStreamKey::new(
                        row.episode_id.clone(),
                        row.stream_id.clone(),
                        u32::try_from(index).unwrap_or(u32::MAX),
                    ),
                    row.properties.clone(),
                )
            })
            .collect::<Vec<_>>()
            .batch()
    }
}

/// Convert rows into a batch suitable for native Arrow-stream ingestion.
pub(super) trait JointSampleRows {
    fn batch(&self) -> Result<RecordBatch, EpisodeError>;
}

impl JointSampleRows for [JointSampleRow] {
    fn batch(&self) -> Result<RecordBatch, EpisodeError> {
        let rows = self;
        let mut state_position = ListBuilder::new(Float64Builder::new());
        let mut action_position = ListBuilder::new(Float64Builder::new());
        let mut raw_position = ListBuilder::new(Int64Builder::new())
            .with_field(Arc::new(Field::new("item", DataType::Int64, false)));
        let mut velocity = ListBuilder::new(Float64Builder::new());
        let mut effort = ListBuilder::new(Float64Builder::new());

        for row in rows {
            state_position.append_nullable_values(Some(&row.state.position));
            action_position.append_nullable_values(row.action_position.as_deref());
            for value in &row.state.raw_position {
                raw_position.values().append_value(*value);
            }
            raw_position.append(true);
            velocity.append_nullable_values(row.state.velocity.as_deref());
            effort.append_nullable_values(row.state.effort.as_deref());
        }

        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.key.episode_id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.key.stream_id.as_str()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|row| row.key.sample_index),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|row| row.key.episode_time_ns),
            )),
            Arc::new(BooleanArray::from(
                rows.iter()
                    .map(|row| row.validity.state)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                rows.iter()
                    .map(|row| row.validity.action)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(state_position.finish()),
            Arc::new(action_position.finish()),
            Arc::new(raw_position.finish()),
            Arc::new(velocity.finish()),
            Arc::new(effort.finish()),
            Arc::new(Float64Array::from_iter(
                rows.iter().map(|row| row.follow_error_degrees),
            )),
        ];
        RecordBatch::try_new(EpisodeSchema::joint_samples(), columns).map_err(|error| {
            EpisodeError::invalid(format!("building Spiral joint sample batch: {error}"))
        })
    }
}

trait NullableFloatListBuilder {
    fn append_nullable_values(&mut self, values: Option<&[Option<f64>]>);
}

impl NullableFloatListBuilder for ListBuilder<Float64Builder> {
    fn append_nullable_values(&mut self, values: Option<&[Option<f64>]>) {
        let Some(values) = values else {
            self.append(false);
            return;
        };
        for value in values {
            match value {
                Some(value) => self.values().append_value(*value),
                None => self.values().append_null(),
            }
        }
        self.append(true);
    }
}

trait EpisodeSample {
    fn validate_for(&self, start: &EpisodeStart) -> Result<(), EpisodeError>;
}

impl EpisodeSample for Sample {
    fn validate_for(&self, start: &EpisodeStart) -> Result<(), EpisodeError> {
        let sample = self;
        if !self.seconds().is_finite() || self.seconds() < 0.0 {
            return Err(EpisodeError::invalid(
                "sample time must be finite and non-negative",
            ));
        }
        if sample
            .follow_error_degrees()
            .is_some_and(|value| !value.is_finite())
        {
            return Err(EpisodeError::invalid(
                "follow error must be finite when present",
            ));
        }
        Shape::new(start.arms().len(), sample.normalized().len())
            .validate_streams("normalized position")?;
        Shape::new(start.arms().len(), sample.raw().len()).validate_streams("raw position")?;
        if let Some(velocity) = &sample.velocity() {
            Shape::new(start.arms().len(), velocity.len()).validate_streams("velocity")?;
        }
        if let Some(effort) = &sample.effort() {
            Shape::new(start.arms().len(), effort.len()).validate_streams("effort")?;
        }
        if !sample.action().is_empty() {
            Shape::new(start.arms().len(), sample.action().len()).validate_streams("action")?;
        }

        for (arm_index, arm) in start.arms().iter().enumerate() {
            Shape::new(arm.joints().len(), sample.normalized()[arm_index].len())
                .validate_joints("normalized position", arm.id())?;
            Shape::new(arm.joints().len(), sample.raw()[arm_index].len())
                .validate_joints("raw position", arm.id())?;
            sample.normalized()[arm_index].validate_finite("normalized position")?;
            if let Some(action) = sample.action().get(arm_index).and_then(Option::as_ref) {
                Shape::new(arm.joints().len(), action.len()).validate_joints("action", arm.id())?;
                action.validate_finite("action")?;
            }
            if let Some(velocity) = &sample.velocity() {
                Shape::new(arm.joints().len(), velocity[arm_index].len())
                    .validate_joints("velocity", arm.id())?;
                velocity[arm_index].validate_finite("velocity")?;
            }
            if let Some(effort) = &sample.effort() {
                Shape::new(arm.joints().len(), effort[arm_index].len())
                    .validate_joints("effort", arm.id())?;
                effort[arm_index].validate_finite("effort")?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Shape {
    expected: usize,
    actual: usize,
}

impl Shape {
    const fn new(expected: usize, actual: usize) -> Self {
        Self { expected, actual }
    }

    fn validate_streams(self, label: &str) -> Result<(), EpisodeError> {
        if self.expected == self.actual {
            return Ok(());
        }
        Err(EpisodeError::invalid(format!(
            "{label} has {} arm streams, expected {}",
            self.actual, self.expected
        )))
    }

    fn validate_joints(self, label: &str, arm: &str) -> Result<(), EpisodeError> {
        if self.expected == self.actual {
            return Ok(());
        }
        Err(EpisodeError::invalid(format!(
            "{label} for arm {arm:?} has {} joints, expected {}",
            self.actual, self.expected
        )))
    }
}

trait FiniteOptionalValues {
    fn validate_finite(&self, label: &str) -> Result<(), EpisodeError>;
}

impl FiniteOptionalValues for [Option<f64>] {
    fn validate_finite(&self, label: &str) -> Result<(), EpisodeError> {
        if self.iter().flatten().all(|value| value.is_finite()) {
            return Ok(());
        }
        Err(EpisodeError::invalid(format!(
            "{label} contains a non-finite value"
        )))
    }
}

struct EpisodeTimestamp;

impl EpisodeTimestamp {
    fn nanoseconds(seconds: f64, label: &str) -> Result<u64, EpisodeError> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(EpisodeError::invalid(format!(
                "{label} must be finite and non-negative"
            )));
        }
        let nanos = seconds * 1_000_000_000.0;
        if nanos > u64::MAX as f64 {
            return Err(EpisodeError::invalid(format!("{label} is too large")));
        }
        Ok(nanos.round() as u64)
    }
}

#[cfg(test)]
mod tests {

    use arrow_array::{Array as _, Float64Array, ListArray, StringArray, UInt64Array};
    use spiral_robotics_common::recording::EpisodeArm;

    use super::*;

    fn start() -> EpisodeStart {
        EpisodeStart::new("episode_000042".to_owned(), "default".to_owned())
            .with_timing(1_700_000_000_000, 60.0)
            .with_source("teleoperation".to_owned(), Some("pick cube".to_owned()))
            .with_arms(
                vec![
                    EpisodeArm::new(
                        "leader",
                        Some("so101_leader".to_owned()),
                        vec!["shoulder".to_owned(), "elbow".to_owned()],
                    ),
                    EpisodeArm::new(
                        "follower",
                        Some("so101_follower".to_owned()),
                        vec!["shoulder".to_owned(), "elbow".to_owned()],
                    ),
                ],
                Vec::new(),
            )
    }

    fn sample(seconds: f64) -> Sample {
        Sample::new(
            seconds,
            vec![vec![Some(0.1), None], vec![Some(0.2), Some(0.3)]],
            vec![vec![10, 20], vec![30, 40]],
        )
        .with_follow_error_degrees(Some(1.25))
        .with_velocity(Some(vec![
            vec![Some(2.0), Some(3.0)],
            vec![Some(4.0), Some(5.0)],
        ]))
        .with_action(vec![None, Some(vec![Some(0.25), Some(0.5)])])
    }

    #[test]
    fn control_frames_become_stably_keyed_arm_rows() {
        let mut encoder = SpiralEpisodeEncoder::new(start()).unwrap();
        let first = encoder.encode(&sample(0.25)).unwrap();
        let second = encoder.encode(&sample(0.5)).unwrap();

        assert_eq!(first.len(), 2);
        assert_eq!(first[0].key.episode_id, "episode_000042");
        assert_eq!(first[0].key.stream_id, "leader");
        assert_eq!(first[1].key.stream_id, "follower");
        assert_eq!(first[0].key.sample_index, 0);
        assert_eq!(first[0].key.episode_time_ns, 250_000_000);
        assert_eq!(second[0].key.sample_index, 1);
        assert_eq!(first[0].action_position, None);
        assert_eq!(first[1].action_position, Some(vec![Some(0.25), Some(0.5)]));
        assert_eq!(encoder.frames(), 2);
    }

    #[test]
    fn malformed_frames_do_not_advance_the_stable_sample_index() {
        let mut encoder = SpiralEpisodeEncoder::new(start()).unwrap();
        let complete = sample(0.0);
        let malformed = Sample::new(
            complete.seconds(),
            complete.normalized().to_vec(),
            vec![complete.raw()[0].clone()],
        );

        let error = encoder.encode(&malformed).unwrap_err().to_string();
        assert!(error.contains("raw position has 1 arm streams, expected 2"));
        assert_eq!(encoder.frames(), 0);
        assert_eq!(encoder.encode(&sample(0.0)).unwrap()[0].key.sample_index, 0);
    }

    #[test]
    fn arrow_batches_preserve_keys_lists_and_nullability() {
        let mut encoder = SpiralEpisodeEncoder::new(start()).unwrap();
        let rows = encoder.encode(&sample(0.25)).unwrap();
        let batch = rows.batch().unwrap();

        assert_eq!(batch.num_rows(), 2);
        let streams = batch
            .column_by_name("stream_id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(streams.value(0), "leader");
        let indexes = batch
            .column_by_name("sample_index")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(indexes.values(), &[0, 0]);

        let states = batch
            .column_by_name("state_position")
            .unwrap()
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        let leader = states.value(0);
        let leader = leader.as_any().downcast_ref::<Float64Array>().unwrap();
        assert_eq!(leader.value(0), 0.1);
        assert!(leader.is_null(1));
        assert!(batch.column_by_name("effort").unwrap().is_null(0));
        assert!(batch.column_by_name("action_position").unwrap().is_null(0));
        assert!(!batch.column_by_name("action_position").unwrap().is_null(1));
    }

    #[test]
    fn completion_row_is_the_episode_visibility_marker() {
        let start = start();
        let completion = EpisodeCompletion::new(2.5, Vec::new(), Vec::new())
            .with_max_follow_error_degrees(Some(1.25));
        let batch = completion.episode_batch(&start, 150).unwrap();

        assert_eq!(batch.num_rows(), 1);
        let dataset = batch
            .column_by_name("dataset_id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(dataset.value(0), "default");
        let frames = batch
            .column_by_name("frames")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(frames.value(0), 150);
        let duration = batch
            .column_by_name("duration_ns")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(duration.value(0), 2_500_000_000);
    }

    #[test]
    fn dataset_row_precedes_its_episode_reference() {
        let batch = start().dataset_batch().unwrap();
        assert_eq!(batch.num_rows(), 1);
        let id = batch
            .column_by_name("dataset_id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(id.value(0), "default");
    }

    #[test]
    fn stream_rows_keep_joint_order() {
        let batch = start()
            .streams_batch(&EpisodeCompletion::new(0.0, Vec::new(), Vec::new()))
            .unwrap();
        let joint_names = batch
            .column_by_name("joint_names")
            .unwrap()
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        let leader = joint_names.value(0);
        let leader = leader.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(leader.value(0), "shoulder");
        assert_eq!(leader.value(1), "elbow");
    }

    #[test]
    fn catalog_streams_decode_without_condition_or_video_columns() {
        let full_batch = start()
            .streams_batch(&EpisodeCompletion::new(0.0, Vec::new(), Vec::new()))
            .unwrap();
        let catalog_batch = full_batch.project(&[0, 1, 2, 3, 4, 5]).unwrap();

        let stream = EpisodeStreamRow::decode_catalog(&catalog_batch, 0).unwrap();

        assert_eq!(stream.arm().id(), "leader");
        assert_eq!(stream.arm().joints(), ["shoulder", "elbow"]);
        assert!(stream.start.is_none());
        assert!(stream.peak.is_none());
        assert!(stream.video.is_none());
    }

    #[test]
    fn camera_rows_share_the_stream_catalog_without_fake_joints() {
        let batch = [VideoStreamRow::new(
            "episode_000042".to_owned(),
            "follower.camera.wrist".to_owned(),
            VideoStreamProperties::new("camera-1".to_owned(), VideoSize::new(640, 480), 30),
        )]
        .batch()
        .unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert!(batch.column_by_name("joint_names").unwrap().is_null(0));
        let modality = batch
            .column_by_name("modality")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(modality.value(0), "video");
    }

    fn condition() -> Vec<ArmCondition> {
        vec![
            ArmCondition::new("follower".to_owned())
                .with_measurements(vec![Some(31.5), None], vec![Some(11.9), Some(12.0)])
                .with_limits(vec![
                    JointMotorLimits::new(
                        Some(70.0),
                        JointVoltageRange::new(Some(9.0), Some(14.0)),
                        Some(0.8),
                    ),
                    JointMotorLimits::default(),
                ]),
        ]
    }

    fn condition_summary() -> Vec<ArmConditionSummary> {
        vec![
            ArmConditionSummary::new("follower".to_owned()).with_extremes(
                ArmConditionThermalExtremes::new(
                    vec![Some(36.0), Some(35.0)],
                    vec![Some(11.5), Some(11.6)],
                ),
                ArmConditionEffortExtremes::new(vec![Some(0.4), None], vec![Some(1.1), Some(1.2)]),
            ),
        ]
    }

    fn populated() -> StoredEpisodeRow {
        StoredEpisodeRow::new(
            start().with_arms(start().arms().to_vec(), condition()),
            EpisodeCompletion::new(2.5, vec![RecordingGap::new(1.0, 0.25)], condition_summary())
                .with_max_follow_error_degrees(Some(1.25))
                .with_discarded(true),
            150,
        )
    }

    /// Read one episode back out of the two relations that hold it.
    fn reread(row: &StoredEpisodeRow) -> StoredEpisodeRow {
        let streams = row.start().stream_rows(row.completion()).batch().unwrap();
        let streams = EpisodeStreamCatalog::new(
            (0..streams.num_rows())
                .map(|index| EpisodeStreamRow::decode(&streams, index).unwrap())
                .collect(),
        );
        let episodes = StoredEpisodeRow::batch(std::slice::from_ref(row)).unwrap();
        StoredEpisodeRow::decode(&episodes, 0, &streams).unwrap()
    }

    /// Everything the metadata blob used to carry now survives as columns.
    #[test]
    fn a_populated_episode_round_trips_through_typed_columns() {
        let original = populated();
        let restored = reread(&original);

        assert_eq!(restored.start().id(), "episode_000042");
        assert_eq!(restored.start().dataset_id(), "default");
        assert_eq!(restored.start().started_ms(), 1_700_000_000_000);
        assert_eq!(restored.start().sample_hz(), 60.0);
        assert_eq!(restored.start().source(), "teleoperation");
        assert_eq!(restored.start().note(), Some("pick cube"));
        assert_eq!(restored.frames(), 150);
        assert_eq!(restored.start().arms(), original.start().arms());
        assert_eq!(restored.start().condition(), condition());
        assert_eq!(restored.completion().duration_seconds(), 2.5);
        assert_eq!(
            restored.completion().recording_gaps(),
            [RecordingGap::new(1.0, 0.25)]
        );
        assert_eq!(restored.completion().max_follow_error_degrees(), Some(1.25));
        assert!(restored.completion().discarded());
        assert_eq!(restored.completion().condition(), condition_summary());
    }

    /// A take whose motors never answered writes null condition columns rather
    /// than an all-empty observation, and reads back as no observation at all.
    #[test]
    fn an_episode_without_motor_telemetry_stores_null_condition_columns() {
        let bare = StoredEpisodeRow::new(
            start(),
            EpisodeCompletion::new(0.0, Vec::new(), Vec::new()),
            0,
        );
        let batch = bare.start().stream_rows(bare.completion()).batch().unwrap();
        for name in CONDITION_COLUMNS {
            assert!(batch.column_by_name(name).unwrap().is_null(0), "{name}");
        }
        let restored = reread(&bare);
        assert_eq!(restored.start().condition(), []);
        assert_eq!(restored.completion().condition(), []);
        assert_eq!(restored.completion().recording_gaps(), []);
    }
}
