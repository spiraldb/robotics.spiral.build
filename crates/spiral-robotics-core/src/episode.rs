//! Episode entity invariants independent of storage and serialization.

use std::collections::BTreeSet;
use std::fmt;

use crate::identity::DatasetId;

pub const MAX_EPISODE_FRAMES: u64 = 1_000_000;
pub const MAX_EPISODE_SAMPLE_HZ: f64 = 1_000.0;
pub const MAX_EPISODE_ARMS: usize = 16;
pub const MAX_JOINTS_PER_ARM: usize = 64;
pub const MAX_EPISODE_DURATION_SECONDS: f64 = 7.0 * 24.0 * 60.0 * 60.0;

pub struct EpisodeArmValidation<'a> {
    pub id: &'a str,
    pub joints: &'a [String],
}

pub struct EpisodeStartValidation<'a> {
    pub id: &'a str,
    pub dataset_id: &'a str,
    pub sample_hz: f64,
    pub source: &'a str,
    pub note: Option<&'a str>,
    pub arms: &'a [EpisodeArmValidation<'a>],
}

pub struct RecordingGapValidation {
    pub after_seconds: f64,
    pub duration_seconds: f64,
}

pub struct EpisodeCompletionValidation<'a> {
    pub frames: u64,
    pub duration_seconds: f64,
    pub recording_gaps: &'a [RecordingGapValidation],
    pub max_follow_error_degrees: Option<f64>,
}

impl EpisodeStartValidation<'_> {
    pub fn validate(self) -> Result<(), String> {
        let start = self;
        if start.id.is_empty()
            || start.id.len() > 96
            || !start
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(
                "episode id must use 1–96 letters, numbers, '-' or '_' characters".to_owned(),
            );
        }
        DatasetId::parse(start.dataset_id).map_err(|error| error.to_string())?;
        if !start.sample_hz.is_finite()
            || start.sample_hz <= 0.0
            || start.sample_hz > MAX_EPISODE_SAMPLE_HZ
        {
            return Err(format!(
                "sample rate must be finite, above 0 and at most {MAX_EPISODE_SAMPLE_HZ} Hz"
            ));
        }
        if start.source.trim().is_empty() || start.source.chars().count() > 256 {
            return Err("episode source must contain 1–256 characters".to_owned());
        }
        if start.note.is_some_and(|note| note.chars().count() > 4_096) {
            return Err("episode note must contain at most 4096 characters".to_owned());
        }
        if start.arms.is_empty() || start.arms.len() > MAX_EPISODE_ARMS {
            return Err(format!("episode must declare 1–{MAX_EPISODE_ARMS} arms"));
        }
        let mut arm_ids = BTreeSet::new();
        for arm in start.arms {
            if arm.id.trim().is_empty() || arm.id.chars().count() > 96 || !arm_ids.insert(arm.id) {
                return Err(format!(
                    "episode has an empty, duplicate, or overlong arm id {:?}",
                    arm.id
                ));
            }
            if arm.joints.is_empty() || arm.joints.len() > MAX_JOINTS_PER_ARM {
                return Err(format!(
                    "episode arm {:?} must declare 1–{MAX_JOINTS_PER_ARM} joints",
                    arm.id
                ));
            }
            let mut joints = BTreeSet::new();
            if arm.joints.iter().any(|joint| {
                joint.trim().is_empty() || joint.chars().count() > 96 || !joints.insert(joint)
            }) {
                return Err(format!(
                    "episode arm {:?} has an empty, duplicate, or overlong joint name",
                    arm.id
                ));
            }
        }
        Ok(())
    }
}

impl EpisodeCompletionValidation<'_> {
    pub fn validate(self) -> Result<(), String> {
        let completion = self;
        if completion.frames > MAX_EPISODE_FRAMES {
            return Err(format!(
                "episode declares more than {MAX_EPISODE_FRAMES} frames"
            ));
        }
        if !completion.duration_seconds.is_finite()
            || !(0.0..=MAX_EPISODE_DURATION_SECONDS).contains(&completion.duration_seconds)
        {
            return Err(format!(
                "episode duration must be finite and between 0 and {MAX_EPISODE_DURATION_SECONDS} seconds"
            ));
        }
        if completion
            .max_follow_error_degrees
            .is_some_and(|error| !error.is_finite() || error < 0.0)
        {
            return Err("maximum follow error must be finite and non-negative".to_owned());
        }
        if completion.recording_gaps.iter().any(|gap| {
            !gap.after_seconds.is_finite()
                || !(0.0..=completion.duration_seconds).contains(&gap.after_seconds)
                || !gap.duration_seconds.is_finite()
                || gap.duration_seconds < 0.0
        }) {
            return Err("episode contains an invalid recording gap".to_owned());
        }
        Ok(())
    }
}

/// The immutable arm/joint shape declared by an episode header.
pub trait ArmLayout {
    fn id(&self) -> &str;
    fn joint_count(&self) -> usize;
}

/// Borrowed values for one episode sample.
pub struct EpisodeFrameValidation<'a> {
    pub seconds: f64,
    pub normalized: &'a [Vec<Option<f64>>],
    pub raw: &'a [Vec<i64>],
    pub follow_error_degrees: Option<f64>,
    pub velocity: Option<&'a [Vec<Option<f64>>]>,
    pub effort: Option<&'a [Vec<Option<f64>>]>,
    pub action: &'a [Option<Vec<Option<f64>>>],
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrameInvariantError {
    InvalidTimestamp,
    TimestampMovedBackwards {
        current: f64,
        previous: f64,
    },
    Shape(String),
    NonFinite {
        field: &'static str,
    },
    OutOfRange {
        field: &'static str,
        arm_index: usize,
        joint_index: usize,
        value: f64,
    },
}

impl fmt::Display for FrameInvariantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimestamp => {
                formatter.write_str("timestamp must be finite and non-negative")
            }
            Self::TimestampMovedBackwards { current, previous } => {
                write!(
                    formatter,
                    "timestamp {current} precedes the previous sample at {previous}"
                )
            }
            Self::Shape(message) => formatter.write_str(message),
            Self::NonFinite { field } => write!(formatter, "{field} contains a non-finite value"),
            Self::OutOfRange {
                field,
                arm_index,
                joint_index,
                value,
            } => write!(
                formatter,
                "{field} arm {arm_index} joint {joint_index} has value {value}; expected a normalized value from -1 through 1"
            ),
        }
    }
}

impl std::error::Error for FrameInvariantError {}

fn validate_nested_shape<T, A: ArmLayout>(
    field: &'static str,
    values: &[Vec<T>],
    arms: &[A],
    absent_when_empty: bool,
) -> Result<(), FrameInvariantError> {
    // Legacy local takes represent a wholly unavailable raw/action field as an
    // empty outer list. Whole-field absence is valid; partial presence is not.
    if absent_when_empty && values.is_empty() {
        return Ok(());
    }
    if values.len() != arms.len() {
        return Err(FrameInvariantError::Shape(format!(
            "{field} has {} arm rows; the header declares {}",
            values.len(),
            arms.len()
        )));
    }
    for (arm_index, (values, arm)) in values.iter().zip(arms).enumerate() {
        if values.len() != arm.joint_count() {
            return Err(FrameInvariantError::Shape(format!(
                "{field} arm {arm_index} ({:?}) has {} joint values; the header declares {}",
                arm.id(),
                values.len(),
                arm.joint_count()
            )));
        }
    }
    Ok(())
}

fn validate_optional_numbers(
    field: &'static str,
    values: &[Vec<Option<f64>>],
) -> Result<(), FrameInvariantError> {
    if values
        .iter()
        .flatten()
        .flatten()
        .any(|value| !value.is_finite())
    {
        return Err(FrameInvariantError::NonFinite { field });
    }
    Ok(())
}

fn validate_normalized_numbers(
    field: &'static str,
    values: &[Vec<Option<f64>>],
) -> Result<(), FrameInvariantError> {
    for (arm_index, values) in values.iter().enumerate() {
        validate_normalized_row(field, arm_index, values)?;
    }
    Ok(())
}

fn validate_normalized_row(
    field: &'static str,
    arm_index: usize,
    values: &[Option<f64>],
) -> Result<(), FrameInvariantError> {
    for (joint_index, value) in values.iter().enumerate() {
        let Some(value) = value else { continue };
        if !value.is_finite() {
            return Err(FrameInvariantError::NonFinite { field });
        }
        if !(-1.0..=1.0).contains(value) {
            return Err(FrameInvariantError::OutOfRange {
                field,
                arm_index,
                joint_index,
                value: *value,
            });
        }
    }
    Ok(())
}

/// Bind one frame to its fixed header shape and preceding timestamp.
///
/// The success path is allocation-free. Two samples may share a controller
/// clock reading, but time may never move backwards. Normalized positions and
/// actions use the exact closed interval `[-1, 1]`: producers must clamp at
/// their own boundary rather than relying on a tolerance that would make an
/// invalid persisted coordinate part of the episode domain.
impl EpisodeFrameValidation<'_> {
    pub fn validate<A: ArmLayout>(
        self,
        arms: &[A],
        previous_seconds: Option<f64>,
    ) -> Result<(), FrameInvariantError> {
        let frame = self;
        if !frame.seconds.is_finite() || frame.seconds < 0.0 {
            return Err(FrameInvariantError::InvalidTimestamp);
        }
        if let Some(previous) = previous_seconds
            && frame.seconds < previous
        {
            return Err(FrameInvariantError::TimestampMovedBackwards {
                current: frame.seconds,
                previous,
            });
        }

        validate_nested_shape("normalized positions", frame.normalized, arms, false)?;
        validate_normalized_numbers("normalized positions", frame.normalized)?;
        validate_nested_shape("raw positions", frame.raw, arms, true)?;

        if frame
            .follow_error_degrees
            .is_some_and(|value| !value.is_finite())
        {
            return Err(FrameInvariantError::NonFinite {
                field: "follow error",
            });
        }
        for (field, values) in [("velocity", frame.velocity), ("effort", frame.effort)] {
            if let Some(values) = values {
                validate_nested_shape(field, values, arms, false)?;
                validate_optional_numbers(field, values)?;
            }
        }

        if !frame.action.is_empty() {
            if frame.action.len() != arms.len() {
                return Err(FrameInvariantError::Shape(format!(
                    "action has {} arm rows; the header declares {}",
                    frame.action.len(),
                    arms.len()
                )));
            }
            for (arm_index, (values, arm)) in frame.action.iter().zip(arms).enumerate() {
                let Some(values) = values else {
                    continue;
                };
                if values.len() != arm.joint_count() {
                    return Err(FrameInvariantError::Shape(format!(
                        "action arm {arm_index} ({:?}) has {} joint values; the header declares {}",
                        arm.id(),
                        values.len(),
                        arm.joint_count()
                    )));
                }
                validate_normalized_row("action", arm_index, values)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Arm(&'static str, usize);

    impl ArmLayout for Arm {
        fn id(&self) -> &str {
            self.0
        }

        fn joint_count(&self) -> usize {
            self.1
        }
    }

    fn valid_frame<'a>(
        normalized: &'a [Vec<Option<f64>>],
        raw: &'a [Vec<i64>],
    ) -> EpisodeFrameValidation<'a> {
        EpisodeFrameValidation {
            seconds: 1.0,
            normalized,
            raw,
            follow_error_degrees: None,
            velocity: None,
            effort: None,
            action: &[],
        }
    }

    #[test]
    fn frame_shape_and_time_are_entity_invariants() {
        let arms = [Arm("arm", 2)];
        let normalized = [vec![Some(0.0), Some(0.2)]];
        let raw = [vec![1, 2]];
        assert!(
            valid_frame(&normalized, &raw)
                .validate(&arms, Some(1.0))
                .is_ok()
        );

        let wrong = [vec![Some(0.0)]];
        assert!(matches!(
            valid_frame(&wrong, &raw).validate(&arms, None),
            Err(FrameInvariantError::Shape(_))
        ));

        let mut backwards = valid_frame(&normalized, &raw);
        backwards.seconds = 0.5;
        assert!(matches!(
            backwards.validate(&arms, Some(1.0)),
            Err(FrameInvariantError::TimestampMovedBackwards { .. })
        ));
    }

    #[test]
    fn episode_start_and_completion_are_domain_invariants() {
        let joints = vec!["shoulder".to_owned(), "elbow".to_owned()];
        let arms = [EpisodeArmValidation {
            id: "follower",
            joints: &joints,
        }];
        assert!(
            EpisodeStartValidation {
                id: "episode_000001",
                dataset_id: "pick_place",
                sample_hz: 60.0,
                source: "Teleoperation",
                note: None,
                arms: &arms,
            }
            .validate()
            .is_ok()
        );
        let duplicate = vec!["shoulder".to_owned(), "shoulder".to_owned()];
        assert!(
            EpisodeStartValidation {
                id: "episode_000001",
                dataset_id: "pick_place",
                sample_hz: 60.0,
                source: "Teleoperation",
                note: None,
                arms: &[EpisodeArmValidation {
                    id: "follower",
                    joints: &duplicate,
                }],
            }
            .validate()
            .is_err()
        );
        assert!(
            EpisodeCompletionValidation {
                frames: 1,
                duration_seconds: 1.0,
                recording_gaps: &[RecordingGapValidation {
                    after_seconds: 0.5,
                    duration_seconds: 0.1,
                }],
                max_follow_error_degrees: Some(2.0),
            }
            .validate()
            .is_ok()
        );
        assert!(
            EpisodeCompletionValidation {
                frames: MAX_EPISODE_FRAMES + 1,
                duration_seconds: 1.0,
                recording_gaps: &[],
                max_follow_error_degrees: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            EpisodeCompletionValidation {
                frames: 1,
                duration_seconds: 1.0,
                recording_gaps: &[RecordingGapValidation {
                    after_seconds: 1.1,
                    duration_seconds: 0.1,
                }],
                max_follow_error_degrees: None,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn normalized_positions_accept_the_exact_closed_interval() {
        let arms = [Arm("arm", 2)];
        let normalized = [vec![Some(-1.0), Some(1.0)]];
        let raw = [vec![1, 2]];

        assert!(valid_frame(&normalized, &raw).validate(&arms, None).is_ok());
    }

    #[test]
    fn normalized_positions_report_the_out_of_range_coordinate() {
        let arms = [Arm("first", 1), Arm("second", 2)];
        let normalized = [vec![Some(0.0)], vec![Some(0.5), Some(1.0 + f64::EPSILON)]];
        let raw = [vec![1], vec![2, 3]];

        assert_eq!(
            valid_frame(&normalized, &raw).validate(&arms, None),
            Err(FrameInvariantError::OutOfRange {
                field: "normalized positions",
                arm_index: 1,
                joint_index: 1,
                value: 1.0 + f64::EPSILON,
            })
        );
    }

    #[test]
    fn actions_report_the_out_of_range_coordinate() {
        let arms = [Arm("first", 1), Arm("second", 2)];
        let normalized = [vec![Some(0.0)], vec![Some(-1.0), Some(1.0)]];
        let raw = [vec![1], vec![2, 3]];
        let action = [
            Some(vec![Some(0.0)]),
            Some(vec![Some(-1.0 - f64::EPSILON), Some(0.5)]),
        ];
        let mut frame = valid_frame(&normalized, &raw);
        frame.action = &action;

        assert_eq!(
            frame.validate(&arms, None),
            Err(FrameInvariantError::OutOfRange {
                field: "action",
                arm_index: 1,
                joint_index: 0,
                value: -1.0 - f64::EPSILON,
            })
        );
    }
}
