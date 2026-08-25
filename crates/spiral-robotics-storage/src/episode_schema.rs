//! Columnar episode rows at the Spiral Robotics → Spiral boundary.
//!
//! This module deliberately stops at Arrow record batches. The shared Spiral
//! project store owns connections while the hardware loop only needs a
//! deterministic conversion from native samples into stable relational keys.

use std::sync::Arc;

use crate::schema::{SCHEMA_VERSION, column as db};
use arrow_array::builder::{Float64Builder, Int64Builder, ListBuilder, StringBuilder};
use arrow_array::{
    ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt32Array,
    UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use spiral_robotics_api::recording::EpisodeArm;

use crate::episodes::{EpisodeCompletion, EpisodeError, EpisodeStart, Sample};
use crate::video::VideoSize;

pub(crate) struct EpisodeSchema;

impl EpisodeSchema {
    /// Schema for [`crate::schema::Table::Datasets`].
    pub(crate) fn datasets() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(db::DATASET_ID, DataType::Utf8, false),
            Field::new(db::NAME, DataType::Utf8, false),
            Field::new(db::CREATED_MS, DataType::Int64, false),
            Field::new(db::DESCRIPTION, DataType::Utf8, true),
        ]))
    }

    fn episodes() -> SchemaRef {
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
            Field::new(db::METADATA_JSON, DataType::Utf8, false),
        ]))
    }

    fn episode_streams() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(db::EPISODE_ID, DataType::Utf8, false),
            Field::new(db::STREAM_ID, DataType::Utf8, false),
            Field::new(db::MODALITY, DataType::Utf8, false),
            Field::new(db::ROBOT_TYPE, DataType::Utf8, true),
            Field::new(
                db::JOINT_NAMES,
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
                true,
            ),
            Field::new(db::CAMERA_ID, DataType::Utf8, true),
            Field::new(db::WIDTH, DataType::UInt32, true),
            Field::new(db::HEIGHT, DataType::UInt32, true),
            Field::new(db::FPS, DataType::UInt32, true),
        ]))
    }

    fn joint_samples() -> SchemaRef {
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
    pub(crate) fn dataset_batch(&self) -> Result<RecordBatch, EpisodeError> {
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

/// One row destined for [`crate::schema::Table::JointSamples`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JointSampleRow {
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
pub(crate) struct VideoStreamRow {
    episode_id: String,
    stream_id: String,
    properties: VideoStreamProperties,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VideoStreamProperties {
    camera_id: String,
    size: VideoSize,
    fps: u32,
}

impl VideoStreamProperties {
    pub(crate) fn new(camera_id: String, size: VideoSize, fps: u32) -> Self {
        Self {
            camera_id,
            size,
            fps,
        }
    }
}

impl VideoStreamRow {
    pub(crate) fn new(
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
pub(crate) struct SpiralEpisodeEncoder {
    start: EpisodeStart,
    next_sample_index: u64,
}

impl SpiralEpisodeEncoder {
    pub(crate) fn new(start: EpisodeStart) -> Result<Self, EpisodeError> {
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

    pub(crate) fn frames(&self) -> u64 {
        self.next_sample_index
    }

    /// Convert one control frame without performing I/O.
    pub(crate) fn encode(&mut self, sample: &Sample) -> Result<Vec<JointSampleRow>, EpisodeError> {
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

/// Build the single row that makes an episode visible to readers.
impl EpisodeCompletion {
    pub(crate) fn episode_batch(
        &self,
        start: &EpisodeStart,
        frames: u64,
    ) -> Result<RecordBatch, EpisodeError> {
        let duration_ns =
            EpisodeTimestamp::nanoseconds(self.duration_seconds(), "episode duration")?;
        let metadata = serde_json::to_string(&(start, self)).map_err(|error| {
            EpisodeError::invalid(format!("encoding episode metadata: {error}"))
        })?;
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(vec![start.id()])),
            Arc::new(StringArray::from(vec![start.dataset_id()])),
            Arc::new(UInt32Array::from(vec![SCHEMA_VERSION])),
            Arc::new(Int64Array::from(vec![start.started_ms()])),
            Arc::new(Float64Array::from(vec![start.sample_hz()])),
            Arc::new(StringArray::from(vec![start.source()])),
            Arc::new(StringArray::from(vec![start.note()])),
            Arc::new(UInt64Array::from(vec![frames])),
            Arc::new(UInt64Array::from(vec![duration_ns])),
            Arc::new(Float64Array::from(vec![self.max_follow_error_degrees()])),
            Arc::new(BooleanArray::from(vec![self.discarded()])),
            Arc::new(StringArray::from(vec![metadata])),
        ];
        RecordBatch::try_new(EpisodeSchema::episodes(), columns).map_err(|error| {
            EpisodeError::invalid(format!("building Spiral episode batch: {error}"))
        })
    }
}

/// Describe every joint stream before its samples are appended.
impl EpisodeStart {
    pub(crate) fn streams_batch(&self) -> Result<RecordBatch, EpisodeError> {
        let start = self;
        let mut joint_names = ListBuilder::new(StringBuilder::new())
            .with_field(Arc::new(Field::new("item", DataType::Utf8, false)));
        for arm in start.arms() {
            for joint in arm.joints() {
                joint_names.values().append_value(joint);
            }
            joint_names.append(true);
        }
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                start.arms().iter().map(|_| start.id()),
            )),
            Arc::new(StringArray::from_iter_values(
                start.arms().iter().map(EpisodeArm::id),
            )),
            Arc::new(StringArray::from_iter_values(
                start.arms().iter().map(|_| "joint"),
            )),
            Arc::new(StringArray::from_iter(
                start.arms().iter().map(EpisodeArm::robot_type),
            )),
            Arc::new(joint_names.finish()),
            Arc::new(StringArray::from(vec![None::<&str>; start.arms().len()])),
            Arc::new(UInt32Array::from(vec![None; start.arms().len()])),
            Arc::new(UInt32Array::from(vec![None; start.arms().len()])),
            Arc::new(UInt32Array::from(vec![None; start.arms().len()])),
        ];
        RecordBatch::try_new(EpisodeSchema::episode_streams(), columns).map_err(|error| {
            EpisodeError::invalid(format!("building Spiral stream batch: {error}"))
        })
    }
}

/// Describe camera streams using the same keyed stream table as robot joints.
pub(crate) trait VideoStreamRows {
    fn batch(&self) -> Result<RecordBatch, EpisodeError>;
}

impl VideoStreamRows for [VideoStreamRow] {
    fn batch(&self) -> Result<RecordBatch, EpisodeError> {
        let rows = self;
        let mut joint_names = ListBuilder::new(StringBuilder::new())
            .with_field(Arc::new(Field::new("item", DataType::Utf8, false)));
        for _ in rows {
            joint_names.append(false);
        }
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.episode_id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.stream_id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|_| "video"))),
            Arc::new(StringArray::from(vec![None::<&str>; rows.len()])),
            Arc::new(joint_names.finish()),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.properties.camera_id.as_str()),
            )),
            Arc::new(UInt32Array::from_iter_values(
                rows.iter().map(|row| row.properties.size.width()),
            )),
            Arc::new(UInt32Array::from_iter_values(
                rows.iter().map(|row| row.properties.size.height()),
            )),
            Arc::new(UInt32Array::from_iter_values(
                rows.iter().map(|row| row.properties.fps),
            )),
        ];
        RecordBatch::try_new(EpisodeSchema::episode_streams(), columns).map_err(|error| {
            EpisodeError::invalid(format!("building Spiral video stream batch: {error}"))
        })
    }
}

/// Convert rows into a batch suitable for ADBC `bind_stream` ingestion.
pub(crate) trait JointSampleRows {
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
    use crate::schema::Table;

    use arrow_array::{Array as _, Float64Array, ListArray, StringArray, UInt64Array};
    use spiral_robotics_api::recording::EpisodeArm;

    use super::*;

    #[test]
    fn persisted_arrow_schemas_match_the_shared_contract() {
        for (table, schema) in [
            (Table::Datasets, EpisodeSchema::datasets()),
            (Table::Episodes, EpisodeSchema::episodes()),
            (Table::EpisodeStreams, EpisodeSchema::episode_streams()),
            (Table::JointSamples, EpisodeSchema::joint_samples()),
        ] {
            let actual = schema
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>();
            assert_eq!(actual, table.columns(), "{table}");
        }
    }

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
        let batch = start().streams_batch().unwrap();
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
}
