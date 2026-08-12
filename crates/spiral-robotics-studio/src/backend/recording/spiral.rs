//! Columnar episode rows at the Spiral Robotics → Spiral boundary.
//!
//! This module deliberately stops at Arrow record batches. Connection,
//! authentication, retries, and publication belong to the eventual Spiral
//! writer worker; the hardware loop only needs a deterministic conversion from
//! its native samples into stable relational keys.

use std::sync::Arc;

use arrow_array::builder::{Float64Builder, Int64Builder, ListBuilder, StringBuilder};
use arrow_array::{
    ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt32Array,
    UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use spiral_robotics_core::spiral::{SCHEMA_VERSION, column as db};

use crate::backend::recording::episodes::{EpisodeCompletion, EpisodeError, EpisodeStart, Sample};

/// One row destined for [`spiral_robotics_core::spiral::Table::JointSamples`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JointSampleRow {
    pub episode_id: String,
    pub stream_id: String,
    pub sample_index: u64,
    pub episode_time_ns: u64,
    pub state_valid: bool,
    pub action_valid: bool,
    pub state_position: Vec<Option<f64>>,
    pub action_position: Option<Vec<Option<f64>>>,
    pub raw_position: Vec<i64>,
    pub velocity: Option<Vec<Option<f64>>>,
    pub effort: Option<Vec<Option<f64>>>,
    pub follow_error_degrees: Option<f64>,
}

/// Camera stream metadata stored beside joint stream descriptions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VideoStreamRow {
    pub episode_id: String,
    pub stream_id: String,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
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
        if start.arms.is_empty() {
            return Err(invalid("a Spiral episode needs at least one arm stream"));
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
        validate_sample(&self.start, sample)?;
        let episode_time_ns = seconds_to_ns(sample.seconds, "sample time")?;
        let sample_index = self.next_sample_index;

        let rows = self
            .start
            .arms
            .iter()
            .enumerate()
            .map(|(arm_index, arm)| JointSampleRow {
                episode_id: self.start.id.clone(),
                stream_id: arm.id.clone(),
                sample_index,
                episode_time_ns,
                state_valid: sample.normalized[arm_index]
                    .iter()
                    .all(|value| value.is_some_and(f64::is_finite)),
                action_valid: sample
                    .action
                    .get(arm_index)
                    .and_then(Option::as_ref)
                    .is_some_and(|values| {
                        values.iter().all(|value| value.is_some_and(f64::is_finite))
                    }),
                state_position: sample.normalized[arm_index].clone(),
                action_position: sample.action.get(arm_index).cloned().flatten(),
                raw_position: sample.raw[arm_index].clone(),
                velocity: sample
                    .velocity
                    .as_ref()
                    .map(|values| values[arm_index].clone()),
                effort: sample
                    .effort
                    .as_ref()
                    .map(|values| values[arm_index].clone()),
                follow_error_degrees: sample.follow_error_degrees,
            })
            .collect();

        self.next_sample_index = self
            .next_sample_index
            .checked_add(1)
            .ok_or_else(|| invalid("episode sample index overflowed u64"))?;
        Ok(rows)
    }
}

/// Schema for the completion row inserted last into
/// [`spiral_robotics_core::spiral::Table::Episodes`].
pub(crate) fn episodes_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(db::EPISODE_ID, DataType::Utf8, false),
        Field::new(db::SCHEMA_VERSION, DataType::UInt32, false),
        Field::new(db::STARTED_MS, DataType::Int64, false),
        Field::new(db::SAMPLE_HZ, DataType::Float64, false),
        Field::new(db::SOURCE, DataType::Utf8, false),
        Field::new(db::NOTE, DataType::Utf8, true),
        Field::new(db::FRAMES, DataType::UInt64, false),
        Field::new(db::DURATION_NS, DataType::UInt64, false),
        Field::new(db::MAX_FOLLOW_ERROR_DEGREES, DataType::Float64, true),
        Field::new(db::DISCARDED, DataType::Boolean, false),
    ]))
}

/// Build the single row that makes an episode visible to readers.
pub(crate) fn episode_completion_batch(
    start: &EpisodeStart,
    completion: &EpisodeCompletion,
    frames: u64,
) -> Result<RecordBatch, EpisodeError> {
    let duration_ns = seconds_to_ns(completion.duration_seconds, "episode duration")?;
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![start.id.as_str()])),
        Arc::new(UInt32Array::from(vec![SCHEMA_VERSION])),
        Arc::new(Int64Array::from(vec![start.started_ms])),
        Arc::new(Float64Array::from(vec![start.sample_hz])),
        Arc::new(StringArray::from(vec![start.source.as_str()])),
        Arc::new(StringArray::from(vec![start.note.as_deref()])),
        Arc::new(UInt64Array::from(vec![frames])),
        Arc::new(UInt64Array::from(vec![duration_ns])),
        Arc::new(Float64Array::from(vec![
            completion.max_follow_error_degrees,
        ])),
        Arc::new(BooleanArray::from(vec![completion.discarded])),
    ];
    RecordBatch::try_new(episodes_schema(), columns)
        .map_err(|error| invalid(format!("building Spiral episode batch: {error}")))
}

/// Schema for [`spiral_robotics_core::spiral::Table::EpisodeStreams`].
pub(crate) fn episode_streams_schema() -> SchemaRef {
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

/// Describe every joint stream before its samples are appended.
pub(crate) fn episode_streams_batch(start: &EpisodeStart) -> Result<RecordBatch, EpisodeError> {
    let mut joint_names = ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
        "item",
        DataType::Utf8,
        false,
    )));
    for arm in &start.arms {
        for joint in &arm.joints {
            joint_names.values().append_value(joint);
        }
        joint_names.append(true);
    }
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from_iter_values(
            start.arms.iter().map(|_| start.id.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            start.arms.iter().map(|arm| arm.id.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            start.arms.iter().map(|_| "joint"),
        )),
        Arc::new(StringArray::from_iter(
            start.arms.iter().map(|arm| arm.robot_type.as_deref()),
        )),
        Arc::new(joint_names.finish()),
        Arc::new(StringArray::from(vec![None::<&str>; start.arms.len()])),
        Arc::new(UInt32Array::from(vec![None; start.arms.len()])),
        Arc::new(UInt32Array::from(vec![None; start.arms.len()])),
        Arc::new(UInt32Array::from(vec![None; start.arms.len()])),
    ];
    RecordBatch::try_new(episode_streams_schema(), columns)
        .map_err(|error| invalid(format!("building Spiral stream batch: {error}")))
}

/// Describe camera streams using the same keyed stream table as robot joints.
pub(crate) fn video_streams_batch(rows: &[VideoStreamRow]) -> Result<RecordBatch, EpisodeError> {
    let mut joint_names = ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
        "item",
        DataType::Utf8,
        false,
    )));
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
            rows.iter().map(|row| row.camera_id.as_str()),
        )),
        Arc::new(UInt32Array::from_iter_values(
            rows.iter().map(|row| row.width),
        )),
        Arc::new(UInt32Array::from_iter_values(
            rows.iter().map(|row| row.height),
        )),
        Arc::new(UInt32Array::from_iter_values(
            rows.iter().map(|row| row.fps),
        )),
    ];
    RecordBatch::try_new(episode_streams_schema(), columns)
        .map_err(|error| invalid(format!("building Spiral video stream batch: {error}")))
}

/// Schema for [`spiral_robotics_core::spiral::Table::JointSamples`].
pub(crate) fn joint_samples_schema() -> SchemaRef {
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

/// Convert rows into a batch suitable for ADBC `bind_stream` ingestion.
pub(crate) fn joint_samples_batch(rows: &[JointSampleRow]) -> Result<RecordBatch, EpisodeError> {
    let mut state_position = ListBuilder::new(Float64Builder::new());
    let mut action_position = ListBuilder::new(Float64Builder::new());
    let mut raw_position = ListBuilder::new(Int64Builder::new()).with_field(Arc::new(Field::new(
        "item",
        DataType::Int64,
        false,
    )));
    let mut velocity = ListBuilder::new(Float64Builder::new());
    let mut effort = ListBuilder::new(Float64Builder::new());

    for row in rows {
        append_nullable_values(&mut state_position, Some(&row.state_position));
        append_nullable_values(&mut action_position, row.action_position.as_deref());
        for value in &row.raw_position {
            raw_position.values().append_value(*value);
        }
        raw_position.append(true);
        append_nullable_values(&mut velocity, row.velocity.as_deref());
        append_nullable_values(&mut effort, row.effort.as_deref());
    }

    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|row| row.episode_id.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|row| row.stream_id.as_str()),
        )),
        Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.sample_index),
        )),
        Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.episode_time_ns),
        )),
        Arc::new(BooleanArray::from(
            rows.iter().map(|row| row.state_valid).collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from(
            rows.iter().map(|row| row.action_valid).collect::<Vec<_>>(),
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
    RecordBatch::try_new(joint_samples_schema(), columns)
        .map_err(|error| invalid(format!("building Spiral joint sample batch: {error}")))
}

fn append_nullable_values(
    builder: &mut ListBuilder<Float64Builder>,
    values: Option<&[Option<f64>]>,
) {
    let Some(values) = values else {
        builder.append(false);
        return;
    };
    for value in values {
        match value {
            Some(value) => builder.values().append_value(*value),
            None => builder.values().append_null(),
        }
    }
    builder.append(true);
}

fn validate_sample(start: &EpisodeStart, sample: &Sample) -> Result<(), EpisodeError> {
    if !sample.seconds.is_finite() || sample.seconds < 0.0 {
        return Err(invalid("sample time must be finite and non-negative"));
    }
    if sample
        .follow_error_degrees
        .is_some_and(|value| !value.is_finite())
    {
        return Err(invalid("follow error must be finite when present"));
    }
    validate_outer_shape(
        "normalized position",
        start.arms.len(),
        sample.normalized.len(),
    )?;
    validate_outer_shape("raw position", start.arms.len(), sample.raw.len())?;
    if let Some(velocity) = &sample.velocity {
        validate_outer_shape("velocity", start.arms.len(), velocity.len())?;
    }
    if let Some(effort) = &sample.effort {
        validate_outer_shape("effort", start.arms.len(), effort.len())?;
    }
    if !sample.action.is_empty() {
        validate_outer_shape("action", start.arms.len(), sample.action.len())?;
    }

    for (arm_index, arm) in start.arms.iter().enumerate() {
        validate_joint_shape(
            "normalized position",
            &arm.id,
            arm.joints.len(),
            sample.normalized[arm_index].len(),
        )?;
        validate_joint_shape(
            "raw position",
            &arm.id,
            arm.joints.len(),
            sample.raw[arm_index].len(),
        )?;
        validate_finite_values("normalized position", &sample.normalized[arm_index])?;
        if let Some(action) = sample.action.get(arm_index).and_then(Option::as_ref) {
            validate_joint_shape("action", &arm.id, arm.joints.len(), action.len())?;
            validate_finite_values("action", action)?;
        }
        if let Some(velocity) = &sample.velocity {
            validate_joint_shape(
                "velocity",
                &arm.id,
                arm.joints.len(),
                velocity[arm_index].len(),
            )?;
            validate_finite_values("velocity", &velocity[arm_index])?;
        }
        if let Some(effort) = &sample.effort {
            validate_joint_shape("effort", &arm.id, arm.joints.len(), effort[arm_index].len())?;
            validate_finite_values("effort", &effort[arm_index])?;
        }
    }
    Ok(())
}

fn validate_outer_shape(label: &str, expected: usize, actual: usize) -> Result<(), EpisodeError> {
    if expected == actual {
        return Ok(());
    }
    Err(invalid(format!(
        "{label} has {actual} arm streams, expected {expected}"
    )))
}

fn validate_joint_shape(
    label: &str,
    arm: &str,
    expected: usize,
    actual: usize,
) -> Result<(), EpisodeError> {
    if expected == actual {
        return Ok(());
    }
    Err(invalid(format!(
        "{label} for arm {arm:?} has {actual} joints, expected {expected}"
    )))
}

fn validate_finite_values(label: &str, values: &[Option<f64>]) -> Result<(), EpisodeError> {
    if values.iter().flatten().all(|value| value.is_finite()) {
        return Ok(());
    }
    Err(invalid(format!("{label} contains a non-finite value")))
}

fn seconds_to_ns(seconds: f64, label: &str) -> Result<u64, EpisodeError> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(invalid(format!("{label} must be finite and non-negative")));
    }
    let nanos = seconds * 1_000_000_000.0;
    if nanos > u64::MAX as f64 {
        return Err(invalid(format!("{label} is too large")));
    }
    Ok(nanos.round() as u64)
}

fn invalid(message: impl Into<String>) -> EpisodeError {
    EpisodeError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use spiral_robotics_core::spiral::Table;

    use arrow_array::{Array as _, Float64Array, ListArray, StringArray, UInt64Array};
    use spiral_robotics_core::api::EpisodeArm;

    use super::*;

    #[test]
    fn persisted_arrow_schemas_match_the_shared_contract() {
        for (table, schema) in [
            (Table::Episodes, episodes_schema()),
            (Table::EpisodeStreams, episode_streams_schema()),
            (Table::JointSamples, joint_samples_schema()),
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
        EpisodeStart {
            id: "episode_000042".to_owned(),
            started_ms: 1_700_000_000_000,
            sample_hz: 60.0,
            source: "teleoperation".to_owned(),
            note: Some("pick cube".to_owned()),
            arms: vec![
                EpisodeArm {
                    id: "leader".to_owned(),
                    robot_type: Some("so101_leader".to_owned()),
                    joints: vec!["shoulder".to_owned(), "elbow".to_owned()],
                },
                EpisodeArm {
                    id: "follower".to_owned(),
                    robot_type: Some("so101_follower".to_owned()),
                    joints: vec!["shoulder".to_owned(), "elbow".to_owned()],
                },
            ],
            condition: Vec::new(),
        }
    }

    fn sample(seconds: f64) -> Sample {
        Sample {
            seconds,
            normalized: vec![vec![Some(0.1), None], vec![Some(0.2), Some(0.3)]],
            raw: vec![vec![10, 20], vec![30, 40]],
            follow_error_degrees: Some(1.25),
            velocity: Some(vec![vec![Some(2.0), Some(3.0)], vec![Some(4.0), Some(5.0)]]),
            effort: None,
            action: vec![None, Some(vec![Some(0.25), Some(0.5)])],
        }
    }

    #[test]
    fn control_frames_become_stably_keyed_arm_rows() {
        let mut encoder = SpiralEpisodeEncoder::new(start()).unwrap();
        let first = encoder.encode(&sample(0.25)).unwrap();
        let second = encoder.encode(&sample(0.5)).unwrap();

        assert_eq!(first.len(), 2);
        assert_eq!(first[0].episode_id, "episode_000042");
        assert_eq!(first[0].stream_id, "leader");
        assert_eq!(first[1].stream_id, "follower");
        assert_eq!(first[0].sample_index, 0);
        assert_eq!(first[0].episode_time_ns, 250_000_000);
        assert_eq!(second[0].sample_index, 1);
        assert_eq!(first[0].action_position, None);
        assert_eq!(first[1].action_position, Some(vec![Some(0.25), Some(0.5)]));
        assert_eq!(encoder.frames(), 2);
    }

    #[test]
    fn malformed_frames_do_not_advance_the_stable_sample_index() {
        let mut encoder = SpiralEpisodeEncoder::new(start()).unwrap();
        let mut malformed = sample(0.0);
        malformed.raw.pop();

        let error = encoder.encode(&malformed).unwrap_err().to_string();
        assert!(error.contains("raw position has 1 arm streams, expected 2"));
        assert_eq!(encoder.frames(), 0);
        assert_eq!(encoder.encode(&sample(0.0)).unwrap()[0].sample_index, 0);
    }

    #[test]
    fn arrow_batches_preserve_keys_lists_and_nullability() {
        let mut encoder = SpiralEpisodeEncoder::new(start()).unwrap();
        let rows = encoder.encode(&sample(0.25)).unwrap();
        let batch = joint_samples_batch(&rows).unwrap();

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
        let completion = EpisodeCompletion {
            duration_seconds: 2.5,
            recording_gaps: Vec::new(),
            max_follow_error_degrees: Some(1.25),
            discarded: false,
            condition: Vec::new(),
        };
        let batch = episode_completion_batch(&start, &completion, 150).unwrap();

        assert_eq!(batch.num_rows(), 1);
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
    fn stream_rows_keep_joint_order() {
        let batch = episode_streams_batch(&start()).unwrap();
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
        let batch = video_streams_batch(&[VideoStreamRow {
            episode_id: "episode_000042".to_owned(),
            stream_id: "follower.camera.wrist".to_owned(),
            camera_id: "camera-1".to_owned(),
            width: 640,
            height: 480,
            fps: 30,
        }])
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
