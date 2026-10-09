//! How one episode is laid out in the recording's Protobuf messages, and the
//! checks every sample passes before it is recorded.

use prost_reflect::Value;

use spiral_robotics_common::robots::MotorLimits;

use super::capture::{EpisodeCompletion, EpisodeError, EpisodeStart, EpisodeSummary, Sample};
use super::mcap_schema::{McapMessage, RecordingSchema};
use crate::SCHEMA_VERSION;

impl EpisodeSummary {
    /// The `Episode` message: identity, timing, and how the episode ended.
    pub(super) fn episode_message(
        &self,
        schema: &RecordingSchema,
    ) -> Result<McapMessage, EpisodeError> {
        let (start, completion) = (self.start(), self.completion());
        let gaps = completion.recording_gaps();
        let after_ns = gaps
            .iter()
            .map(|gap| {
                EpisodeTimestamp::nanoseconds(gap.after_seconds(), "recording gap start")
                    .map(|nanos| Some(Value::U64(nanos)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let duration_ns = gaps
            .iter()
            .map(|gap| {
                EpisodeTimestamp::nanoseconds(gap.duration_seconds(), "recording gap duration")
                    .map(|nanos| Some(Value::U64(nanos)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let duration =
            EpisodeTimestamp::nanoseconds(completion.duration_seconds(), "episode duration")?;
        let mut message = schema.message("Episode", "/studio/episode".to_owned());
        message
            .set("episode_id", Value::String(start.id().to_owned()))
            .set("dataset_id", Value::String(start.dataset_id().to_owned()))
            .set("schema_version", Value::U32(SCHEMA_VERSION))
            .set("started_ms", Value::I64(start.started_ms()))
            .set("sample_hz", Value::F64(start.sample_hz()))
            .set("source", Value::String(start.source().to_owned()))
            .set("frames", Value::U64(self.frames()))
            .set("duration_ns", Value::U64(duration))
            .set_optional(
                "max_follow_error_degrees",
                completion.max_follow_error_degrees().map(Value::F64),
            )
            .set("discarded", Value::Bool(completion.discarded()))
            .set_list("recording_gap_after_ns", after_ns)
            .set_list("recording_gap_duration_ns", duration_ns);
        Ok(message)
    }
}

impl EpisodeStart {
    /// One joint `Stream` message per arm, in arm order: the order its values
    /// episode in every sample. Motor readings that measured nothing stay absent.
    pub(super) fn joint_stream_messages(
        &self,
        completion: &EpisodeCompletion,
        schema: &RecordingSchema,
    ) -> Vec<McapMessage> {
        self.arms()
            .iter()
            .enumerate()
            .map(|(index, arm)| {
                let ordinal = u32::try_from(index).unwrap_or(u32::MAX);
                let mut message = schema
                    .message("Stream", format!("/studio/streams/{}", arm.id()))
                    .at(0, ordinal);
                message
                    .set("episode_id", Value::String(self.id().to_owned()))
                    .set("stream_id", Value::String(arm.id().to_owned()))
                    .set("ordinal", Value::U32(ordinal))
                    .set("modality", Value::String("joint".to_owned()))
                    .set_optional(
                        "robot_type",
                        arm.robot_type().map(|kind| Value::String(kind.to_owned())),
                    )
                    .set_list(
                        "joint_names",
                        arm.joints()
                            .iter()
                            .map(|joint| Some(Value::String(joint.clone()))),
                    );
                let opening = self
                    .condition()
                    .iter()
                    .find(|entry| entry.arm() == arm.id());
                if let Some(opening) = opening.filter(|opening| !opening.is_empty()) {
                    let limits = |read: fn(MotorLimits) -> Option<f64>| {
                        opening
                            .limits()
                            .iter()
                            .map(move |limits| read(*limits).map(Value::F64))
                    };
                    message
                        .set_floats("start_temperature_celsius", opening.temperature_celsius())
                        .set_floats("start_voltage_volts", opening.voltage_volts())
                        .set_list(
                            "limit_max_temperature_celsius",
                            limits(MotorLimits::max_temperature_celsius),
                        )
                        .set_list(
                            "limit_min_voltage_volts",
                            limits(MotorLimits::min_voltage_volts),
                        )
                        .set_list(
                            "limit_max_voltage_volts",
                            limits(MotorLimits::max_voltage_volts),
                        )
                        .set_list("limit_max_load", limits(MotorLimits::max_load));
                }
                let peaks = completion
                    .condition()
                    .iter()
                    .find(|entry| entry.arm() == arm.id());
                if let Some(peaks) = peaks.filter(|peaks| !peaks.is_empty()) {
                    message
                        .set_floats("peak_temperature_celsius", peaks.peak_temperature_celsius())
                        .set_floats("min_voltage_volts", peaks.min_voltage_volts())
                        .set_floats("peak_load", peaks.peak_load())
                        .set_floats("peak_current_amps", peaks.peak_current_amps());
                }
                message
            })
            .collect()
    }

    /// One `JointSample` message per arm for the sample at `index`.
    pub(super) fn joint_sample_messages(
        &self,
        sample: &Sample,
        index: u64,
        schema: &RecordingSchema,
    ) -> Result<Vec<McapMessage>, EpisodeError> {
        sample.validate_for(self)?;
        let time_ns = EpisodeTimestamp::nanoseconds(sample.seconds(), "sample time")?;
        let sequence = u32::try_from(index)
            .map_err(|_| EpisodeError::invalid("Recording sequence overflow"))?;
        let finite =
            |values: &[Option<f64>]| values.iter().all(|value| value.is_some_and(f64::is_finite));
        Ok(self
            .arms()
            .iter()
            .enumerate()
            .map(|(arm_index, arm)| {
                let action = sample.action().get(arm_index).and_then(Option::as_ref);
                let mut message = schema
                    .message("JointSample", format!("/studio/joints/{}", arm.id()))
                    .at(time_ns, sequence);
                message
                    .set("episode_id", Value::String(self.id().to_owned()))
                    .set("stream_id", Value::String(arm.id().to_owned()))
                    .set("sample_index", Value::U64(index))
                    .set("episode_time_ns", Value::U64(time_ns))
                    .set(
                        "state_valid",
                        Value::Bool(finite(&sample.normalized()[arm_index])),
                    )
                    .set(
                        "action_valid",
                        Value::Bool(action.is_some_and(|action| finite(action))),
                    )
                    .set_floats("state_position", &sample.normalized()[arm_index])
                    .set_list(
                        "raw_position",
                        sample.raw()[arm_index]
                            .iter()
                            .map(|raw| Some(Value::I64(*raw))),
                    )
                    .set_optional(
                        "follow_error_degrees",
                        sample.follow_error_degrees().map(Value::F64),
                    );
                if let Some(action) = action {
                    message.set_floats("action_position", action);
                }
                if let Some(velocity) = sample.velocity() {
                    message.set_floats("velocity", &velocity[arm_index]);
                }
                if let Some(effort) = sample.effort() {
                    message.set_floats("effort", &effort[arm_index]);
                }
                message
            })
            .collect())
    }
}

pub(super) trait EpisodeSample {
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
    use super::*;
    use spiral_robotics_common::recording::EpisodeArm;

    #[test]
    fn samples_must_match_the_declared_arms_and_joints() {
        let start = EpisodeStart::new("episode_000042".to_owned(), "default".to_owned()).with_arms(
            vec![
                EpisodeArm::new("leader", None, vec!["shoulder".to_owned()]),
                EpisodeArm::new("follower", None, vec!["shoulder".to_owned()]),
            ],
            Vec::new(),
        );
        let normalized = vec![vec![Some(0.1)], vec![Some(0.2)]];
        assert!(
            Sample::new(0.0, normalized.clone(), vec![vec![1], vec![2]])
                .validate_for(&start)
                .is_ok()
        );
        let error = Sample::new(0.0, normalized, vec![vec![1]])
            .validate_for(&start)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("raw position has 1 arm streams, expected 2"),
            "{error}"
        );
    }
}
