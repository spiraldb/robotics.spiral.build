//! Protocol-neutral motor calibration data and validation.
//!
//! This module deliberately does not talk to a motor bus or synthesize
//! hardware-specific register values. Callers provide either center/min/max
//! readings or a center reading followed by raw sweep samples. Stored values
//! remain in the unhomed raw coordinate system in which they were observed.
//!
//! Domain values here deliberately have no serialization implementation. Robot
//! files and delivery contracts translate their wire representations at the
//! edge, while calibration policy receives and returns plain values.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

/// The current on-disk calibration format version.
pub const CALIBRATION_FORMAT_VERSION: u32 = 1;

/// Units used by calibrations produced by this module.
pub const CALIBRATION_UNITS: &str = "raw";

/// Coordinate system used by calibrations produced by this module.
pub const CALIBRATION_COORDINATE_SPACE: &str = "unhomed_raw";

/// Whether increasing raw readings represent the normal or reversed direction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Direction {
    #[default]
    Normal,
    Reversed,
}

/// Static configuration for one motor.
///
/// Unknown persisted fields belong to the robot-file adapter, not this domain
/// value. `model` is explicit because selecting a compatible bus driver is
/// product policy used by hardware adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MotorSpec {
    pub id: u32,
    pub direction: Direction,
    pub model: Option<String>,
}

impl MotorSpec {
    /// Construct a motor definition without any adapter-specific details.
    pub fn new(id: u32, direction: Direction) -> Self {
        Self {
            id,
            direction,
            model: None,
        }
    }

    /// State the motor model used to select a compatible hardware adapter.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

/// Raw readings for one motor after a completed range sweep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotorReading {
    pub center: i64,
    pub range_min: i64,
    pub range_max: i64,
}

/// Input accepted by [`build_calibration`].
///
/// `Direct` accepts readings already reduced to extrema. `Sweep` derives the
/// extrema from one or more complete snapshots. Domain validation rejects
/// missing and unexpected motors; an edge codec is responsible for rejecting
/// misspelled fields and duplicate wire keys before constructing this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalibrationInput {
    Direct {
        motors: BTreeMap<String, MotorReading>,
    },
    Sweep {
        center: BTreeMap<String, i64>,
        samples: Vec<BTreeMap<String, i64>>,
    },
}

/// Protocol-neutral calibration for one motor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotorCalibration {
    pub id: u32,
    pub direction: Direction,
    pub range_min: i64,
    pub center: i64,
    pub range_max: i64,
}

/// A validated, deterministic calibration document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Calibration {
    pub format_version: u32,
    pub units: String,
    pub coordinate_space: String,
    pub motors: BTreeMap<String, MotorCalibration>,
}

/// A validation error encountered while reading or building a calibration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalibrationError {
    NoMotors,
    InvalidMotorName {
        name: String,
    },
    DuplicateMotorId {
        id: u32,
        first: String,
        second: String,
    },
    KeyMismatch {
        context: String,
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
    MotorSpecMismatch {
        motor: String,
        expected_id: u32,
        calibration_id: u32,
        expected_direction: Direction,
        calibration_direction: Direction,
    },
    EmptySamples,
    NoPositions,
    MotorSetChanged {
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
    InsufficientTravel {
        motors: Vec<String>,
    },
    InvalidRange {
        motor: String,
        range_min: i64,
        center: i64,
        range_max: i64,
    },
    NonFiniteNormalizedCommand,
    NormalizedCommandOutOfRange,
    UnsupportedFormatVersion(u32),
    UnsupportedUnits(String),
    UnsupportedCoordinateSpace(String),
}

impl fmt::Display for CalibrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMotors => formatter.write_str("at least one motor must be configured"),
            Self::InvalidMotorName { name } => write!(
                formatter,
                "invalid motor name {name:?}: names must be nonempty and contain no whitespace or control characters"
            ),
            Self::DuplicateMotorId { id, first, second } => write!(
                formatter,
                "motor ID {id} is assigned to both {first:?} and {second:?}"
            ),
            Self::KeyMismatch {
                context,
                missing,
                unexpected,
            } => {
                write!(
                    formatter,
                    "{context} motor names do not match the specification"
                )?;
                if !missing.is_empty() {
                    write!(formatter, "; missing: {}", missing.join(", "))?;
                }
                if !unexpected.is_empty() {
                    write!(formatter, "; unexpected: {}", unexpected.join(", "))?;
                }
                Ok(())
            }
            Self::MotorSpecMismatch {
                motor,
                expected_id,
                calibration_id,
                expected_direction,
                calibration_direction,
            } => write!(
                formatter,
                "calibration for motor {motor:?} does not match its specification: expected ID {expected_id} and direction {expected_direction:?}, got ID {calibration_id} and direction {calibration_direction:?}"
            ),
            Self::EmptySamples => {
                formatter.write_str("a calibration sweep must contain at least one sample")
            }
            Self::NoPositions => formatter.write_str("the motor bus returned no positions"),
            Self::MotorSetChanged {
                missing,
                unexpected,
            } => {
                formatter.write_str("the motor set changed while calibration was running")?;
                if !missing.is_empty() {
                    write!(formatter, "; stopped reporting: {}", missing.join(", "))?;
                }
                if !unexpected.is_empty() {
                    write!(formatter, "; started reporting: {}", unexpected.join(", "))?;
                }
                Ok(())
            }
            Self::InsufficientTravel { motors } => write!(
                formatter,
                "these motors did not move far enough to calibrate: {}; move every joint through its full range",
                motors.join(", ")
            ),
            Self::InvalidRange {
                motor,
                range_min,
                center,
                range_max,
            } => write!(
                formatter,
                "motor {motor:?} has invalid range: expected range_min < center < range_max, got {range_min} < {center} < {range_max}"
            ),
            Self::NonFiniteNormalizedCommand => {
                formatter.write_str("normalized motor command must be finite")
            }
            Self::NormalizedCommandOutOfRange => {
                formatter.write_str("normalized motor command must be within [-1, 1]")
            }
            Self::UnsupportedFormatVersion(version) => write!(
                formatter,
                "unsupported calibration format_version {version}; expected {CALIBRATION_FORMAT_VERSION}"
            ),
            Self::UnsupportedUnits(units) => write!(
                formatter,
                "unsupported calibration units {units:?}; expected {CALIBRATION_UNITS:?}"
            ),
            Self::UnsupportedCoordinateSpace(space) => write!(
                formatter,
                "unsupported calibration coordinate_space {space:?}; expected {CALIBRATION_COORDINATE_SPACE:?}"
            ),
        }
    }
}

/// A finite position inside the product's normalized motion range.
///
/// Delivery and application layers construct this value before a command can
/// cross a motor-control port. Hardware adapters still recheck the primitive
/// value as defense in depth.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct NormalizedPosition(f64);

impl NormalizedPosition {
    pub fn new(value: f64) -> Result<Self, CalibrationError> {
        if !value.is_finite() {
            return Err(CalibrationError::NonFiniteNormalizedCommand);
        }
        if !(-1.0..=1.0).contains(&value) {
            return Err(CalibrationError::NormalizedCommandOutOfRange);
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Error for CalibrationError {}

/// Validate protocol-neutral motor specifications.
pub fn validate_motor_specs(specs: &BTreeMap<String, MotorSpec>) -> Result<(), CalibrationError> {
    if specs.is_empty() {
        return Err(CalibrationError::NoMotors);
    }

    let mut ids = BTreeMap::<u32, &str>::new();
    for (name, spec) in specs {
        validate_motor_name(name)?;

        if let Some(first) = ids.insert(spec.id, name) {
            return Err(CalibrationError::DuplicateMotorId {
                id: spec.id,
                first: first.to_owned(),
                second: name.clone(),
            });
        }
    }

    Ok(())
}

/// Validate a fully materialized calibration, including deserialized data.
pub fn validate_calibration(calibration: &Calibration) -> Result<(), CalibrationError> {
    if calibration.format_version != CALIBRATION_FORMAT_VERSION {
        return Err(CalibrationError::UnsupportedFormatVersion(
            calibration.format_version,
        ));
    }
    if calibration.units != CALIBRATION_UNITS {
        return Err(CalibrationError::UnsupportedUnits(
            calibration.units.clone(),
        ));
    }
    if calibration.coordinate_space != CALIBRATION_COORDINATE_SPACE {
        return Err(CalibrationError::UnsupportedCoordinateSpace(
            calibration.coordinate_space.clone(),
        ));
    }
    if calibration.motors.is_empty() {
        return Err(CalibrationError::NoMotors);
    }

    let mut ids = BTreeMap::<u32, &str>::new();
    for (name, motor) in &calibration.motors {
        validate_motor_name(name)?;
        if let Some(first) = ids.insert(motor.id, name) {
            return Err(CalibrationError::DuplicateMotorId {
                id: motor.id,
                first: first.to_owned(),
                second: name.clone(),
            });
        }
        validate_range(
            name,
            MotorReading {
                center: motor.center,
                range_min: motor.range_min,
                range_max: motor.range_max,
            },
        )?;
    }
    Ok(())
}

/// Validate that a calibration belongs to one exact configured motor set.
///
/// A calibration range is meaningful only for the named physical motor it was
/// measured from. Matching names alone is insufficient: changing a bus ID or
/// direction while retaining an old range would make normalized commands refer
/// to different hardware semantics.
pub fn validate_calibration_for_motors(
    specs: &BTreeMap<String, MotorSpec>,
    calibration: &Calibration,
) -> Result<(), CalibrationError> {
    validate_motor_specs(specs)?;
    validate_calibration(calibration)?;
    validate_keys(specs, &calibration.motors, "calibration")?;

    for (name, spec) in specs {
        let motor = calibration
            .motors
            .get(name)
            .expect("validated calibration keys must contain every configured motor");
        if motor.id != spec.id || motor.direction != spec.direction {
            return Err(CalibrationError::MotorSpecMismatch {
                motor: name.clone(),
                expected_id: spec.id,
                calibration_id: motor.id,
                expected_direction: spec.direction,
                calibration_direction: motor.direction,
            });
        }
    }
    Ok(())
}

/// Build a calibration from direct readings or a raw range sweep.
pub fn build_calibration(
    specs: &BTreeMap<String, MotorSpec>,
    input: &CalibrationInput,
) -> Result<Calibration, CalibrationError> {
    validate_motor_specs(specs)?;

    let readings = match input {
        CalibrationInput::Direct { motors } => {
            validate_keys(specs, motors, "direct readings")?;
            motors.clone()
        }
        CalibrationInput::Sweep { center, samples } => {
            validate_keys(specs, center, "center readings")?;
            if samples.is_empty() {
                return Err(CalibrationError::EmptySamples);
            }

            let mut extrema = specs
                .keys()
                .map(|name| (name.clone(), (i64::MAX, i64::MIN)))
                .collect::<BTreeMap<_, _>>();
            for (index, sample) in samples.iter().enumerate() {
                validate_keys(specs, sample, &format!("sweep sample {index}"))?;
                for (name, value) in sample {
                    let (minimum, maximum) = extrema
                        .get_mut(name)
                        .expect("validated sample key must have an extrema entry");
                    *minimum = (*minimum).min(*value);
                    *maximum = (*maximum).max(*value);
                }
            }

            specs
                .keys()
                .map(|name| {
                    let &(range_min, range_max) = extrema
                        .get(name)
                        .expect("every motor must have an extrema entry");
                    let &center = center
                        .get(name)
                        .expect("validated center must contain every motor");
                    (
                        name.clone(),
                        MotorReading {
                            center,
                            range_min,
                            range_max,
                        },
                    )
                })
                .collect()
        }
    };

    let motors = specs
        .iter()
        .map(|(name, spec)| {
            let reading = *readings
                .get(name)
                .expect("validated input must contain every motor");
            validate_range(name, reading)?;
            Ok((
                name.clone(),
                MotorCalibration {
                    id: spec.id,
                    direction: spec.direction,
                    range_min: reading.range_min,
                    center: reading.center,
                    range_max: reading.range_max,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, CalibrationError>>()?;

    let calibration = Calibration {
        format_version: CALIBRATION_FORMAT_VERSION,
        units: CALIBRATION_UNITS.to_owned(),
        coordinate_space: CALIBRATION_COORDINATE_SPACE.to_owned(),
        motors,
    };
    validate_calibration(&calibration)?;
    Ok(calibration)
}

/// Normalize an unhomed raw reading to `[-1.0, 1.0]` around its center.
///
/// This is Spiral Robotics' protocol-neutral centered normalization, not a hardware
/// register encoding or LeRobot wire-format conversion. Values outside the
/// observed range are clamped, asymmetric sides are scaled independently, and
/// [`Direction::Reversed`] negates the result.
pub fn normalize_centered_raw(
    calibration: &MotorCalibration,
    raw: i64,
) -> Result<f64, CalibrationError> {
    validate_range(
        &format!("id {}", calibration.id),
        MotorReading {
            center: calibration.center,
            range_min: calibration.range_min,
            range_max: calibration.range_max,
        },
    )?;

    let bounded = raw.clamp(calibration.range_min, calibration.range_max);
    let (numerator, denominator) = if bounded < calibration.center {
        (
            i128::from(bounded) - i128::from(calibration.center),
            i128::from(calibration.center) - i128::from(calibration.range_min),
        )
    } else {
        (
            i128::from(bounded) - i128::from(calibration.center),
            i128::from(calibration.range_max) - i128::from(calibration.center),
        )
    };
    let normalized = numerator as f64 / denominator as f64;
    Ok(match calibration.direction {
        Direction::Normal => normalized,
        Direction::Reversed => -normalized,
    })
}

/// The travel one motor has shown so far during a live calibration sweep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LiveRange {
    pub minimum: i64,
    pub position: i64,
    pub maximum: i64,
}

impl LiveRange {
    /// Travel observed so far, saturating rather than overflowing.
    pub fn span(&self) -> i64 {
        self.maximum.saturating_sub(self.minimum)
    }
}

/// Accumulates the extrema of every motor while an operator sweeps the arm.
///
/// The terminal calibration command and the studio's control loop both feed
/// this the same way, so "did this joint move far enough" has exactly one
/// definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RangeTracker {
    ranges: BTreeMap<String, LiveRange>,
}

/// Encoder counts a joint must travel before its range is considered usable.
///
/// Two counts is the smallest span that leaves the center strictly inside the
/// range, which [`build_calibration`] requires.
pub const MINIMUM_CALIBRATION_TRAVEL: i64 = 2;

impl RangeTracker {
    /// Start tracking from an initial reading of every motor.
    pub fn new(positions: BTreeMap<String, i64>) -> Result<Self, CalibrationError> {
        if positions.is_empty() {
            return Err(CalibrationError::NoPositions);
        }
        Ok(Self {
            ranges: positions
                .into_iter()
                .map(|(name, position)| {
                    (
                        name,
                        LiveRange {
                            minimum: position,
                            position,
                            maximum: position,
                        },
                    )
                })
                .collect(),
        })
    }

    /// Fold one fresh reading of every motor into the tracked extrema.
    pub fn observe(&mut self, positions: &BTreeMap<String, i64>) -> Result<(), CalibrationError> {
        let expected = self.ranges.keys().cloned().collect::<BTreeSet<_>>();
        let actual = positions.keys().cloned().collect::<BTreeSet<_>>();
        if expected != actual {
            return Err(CalibrationError::MotorSetChanged {
                missing: expected.difference(&actual).cloned().collect(),
                unexpected: actual.difference(&expected).cloned().collect(),
            });
        }

        for (name, position) in positions {
            let range = self
                .ranges
                .get_mut(name)
                .expect("validated position must have a range");
            range.position = *position;
            range.minimum = range.minimum.min(*position);
            range.maximum = range.maximum.max(*position);
        }
        Ok(())
    }

    /// Borrow the tracked travel of every motor.
    pub fn ranges(&self) -> &BTreeMap<String, LiveRange> {
        &self.ranges
    }

    /// Motors that have not yet moved far enough to produce a valid range.
    pub fn insufficient_travel(&self) -> Vec<String> {
        self.ranges
            .iter()
            .filter(|(_, range)| range.span() < MINIMUM_CALIBRATION_TRAVEL)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// True once every motor has swept a usable range.
    pub fn is_ready(&self) -> bool {
        self.insufficient_travel().is_empty()
    }

    /// Reduce the sweep to calibration input, centering each motor on the
    /// midpoint of its observed travel.
    pub fn into_input(self) -> Result<CalibrationInput, CalibrationError> {
        let insufficient = self.insufficient_travel();
        if !insufficient.is_empty() {
            return Err(CalibrationError::InsufficientTravel {
                motors: insufficient,
            });
        }

        let motors = self
            .ranges
            .into_iter()
            .map(|(name, range)| {
                let center = (i128::from(range.minimum) + i128::from(range.maximum)) / 2;
                (
                    name,
                    MotorReading {
                        center: i64::try_from(center)
                            .expect("the midpoint of two i64 values must fit in i64"),
                        range_min: range.minimum,
                        range_max: range.maximum,
                    },
                )
            })
            .collect();
        Ok(CalibrationInput::Direct { motors })
    }
}

/// Map a normalized `[-1.0, 1.0]` command back to an unhomed raw position.
///
/// This is the exact inverse of [`normalize_centered_raw`] up to rounding: the
/// command is clamped to `[-1.0, 1.0]`, the direction is undone, each side of
/// the center is scaled by its own half-range, and the result is clamped to the
/// calibrated range so a caller can never command a joint past a limit it was
/// calibrated against. Non-finite input is rejected: silently translating a
/// corrupt command into motion toward center can move a physical arm while
/// hiding the upstream fault.
pub fn denormalize_centered_raw(
    calibration: &MotorCalibration,
    normalized: f64,
) -> Result<i64, CalibrationError> {
    validate_range(
        &format!("id {}", calibration.id),
        MotorReading {
            center: calibration.center,
            range_min: calibration.range_min,
            range_max: calibration.range_max,
        },
    )?;

    if !normalized.is_finite() {
        return Err(CalibrationError::NonFiniteNormalizedCommand);
    }
    let command = normalized;
    let command = command.clamp(-1.0, 1.0);
    let command = match calibration.direction {
        Direction::Normal => command,
        Direction::Reversed => -command,
    };

    let half_range = if command < 0.0 {
        i128::from(calibration.center) - i128::from(calibration.range_min)
    } else {
        i128::from(calibration.range_max) - i128::from(calibration.center)
    };
    let offset = command * half_range as f64;
    let raw = (calibration.center as f64 + offset).round();

    // f64 cannot represent every i64, so clamp in float space before casting.
    Ok(if raw <= calibration.range_min as f64 {
        calibration.range_min
    } else if raw >= calibration.range_max as f64 {
        calibration.range_max
    } else {
        (raw as i64).clamp(calibration.range_min, calibration.range_max)
    })
}

fn validate_motor_name(name: &str) -> Result<(), CalibrationError> {
    if name.is_empty()
        || name
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        Err(CalibrationError::InvalidMotorName {
            name: name.to_owned(),
        })
    } else {
        Ok(())
    }
}

fn validate_keys<T>(
    specs: &BTreeMap<String, MotorSpec>,
    values: &BTreeMap<String, T>,
    context: &str,
) -> Result<(), CalibrationError> {
    let expected = specs.keys().cloned().collect::<BTreeSet<_>>();
    let actual = values.keys().cloned().collect::<BTreeSet<_>>();
    if expected == actual {
        return Ok(());
    }
    Err(CalibrationError::KeyMismatch {
        context: context.to_owned(),
        missing: expected.difference(&actual).cloned().collect(),
        unexpected: actual.difference(&expected).cloned().collect(),
    })
}

fn validate_range(name: &str, reading: MotorReading) -> Result<(), CalibrationError> {
    if reading.range_min < reading.center && reading.center < reading.range_max {
        Ok(())
    } else {
        Err(CalibrationError::InvalidRange {
            motor: name.to_owned(),
            range_min: reading.range_min,
            center: reading.center,
            range_max: reading.range_max,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: u32) -> MotorSpec {
        MotorSpec {
            id,
            direction: Direction::Normal,
            model: None,
        }
    }

    fn two_specs() -> BTreeMap<String, MotorSpec> {
        BTreeMap::from([
            ("elbow".to_owned(), spec(2)),
            ("shoulder".to_owned(), spec(1)),
        ])
    }

    fn direct_input() -> CalibrationInput {
        CalibrationInput::Direct {
            motors: BTreeMap::from([
                (
                    "shoulder".to_owned(),
                    MotorReading {
                        center: 2100,
                        range_min: 900,
                        range_max: 3300,
                    },
                ),
                (
                    "elbow".to_owned(),
                    MotorReading {
                        center: 500,
                        range_min: 100,
                        range_max: 800,
                    },
                ),
            ]),
        }
    }

    #[test]
    fn motor_spec_has_only_explicit_domain_fields() {
        let spec = MotorSpec::new(7, Direction::Normal).with_model("sts3215");
        assert_eq!(spec.id, 7);
        assert_eq!(spec.direction, Direction::Normal);
        assert_eq!(spec.model.as_deref(), Some("sts3215"));
    }

    #[test]
    fn direct_input_preserves_truthful_unhomed_raw_values() {
        let mut specs = two_specs();
        specs.get_mut("elbow").unwrap().direction = Direction::Reversed;
        let calibration = build_calibration(&specs, &direct_input()).unwrap();

        assert_eq!(calibration.coordinate_space, "unhomed_raw");
        assert_eq!(calibration.motors["shoulder"].center, 2100);
        assert_eq!(calibration.motors["shoulder"].range_min, 900);
        assert_eq!(calibration.motors["shoulder"].range_max, 3300);
        assert_eq!(calibration.motors["elbow"].direction, Direction::Reversed);
    }

    #[test]
    fn sweep_derives_extrema_from_all_samples() {
        let input = CalibrationInput::Sweep {
            center: BTreeMap::from([("shoulder".to_owned(), 2000), ("elbow".to_owned(), 1900)]),
            samples: vec![
                BTreeMap::from([("shoulder".to_owned(), 1500), ("elbow".to_owned(), 2300)]),
                BTreeMap::from([("shoulder".to_owned(), 2800), ("elbow".to_owned(), 900)]),
            ],
        };
        let calibration = build_calibration(&two_specs(), &input).unwrap();
        assert_eq!(calibration.motors["shoulder"].range_min, 1500);
        assert_eq!(calibration.motors["shoulder"].range_max, 2800);
        assert_eq!(calibration.motors["elbow"].range_min, 900);
        assert_eq!(calibration.motors["elbow"].range_max, 2300);
    }

    #[test]
    fn validates_motor_specs() {
        assert_eq!(
            validate_motor_specs(&BTreeMap::new()),
            Err(CalibrationError::NoMotors)
        );

        let mut specs = two_specs();
        specs.insert("bad name".to_owned(), spec(3));
        assert!(matches!(
            validate_motor_specs(&specs),
            Err(CalibrationError::InvalidMotorName { .. })
        ));

        let mut specs = two_specs();
        specs.get_mut("elbow").unwrap().id = 1;
        assert!(matches!(
            validate_motor_specs(&specs),
            Err(CalibrationError::DuplicateMotorId { .. })
        ));
    }

    #[test]
    fn calibration_must_match_motor_names_ids_and_directions() {
        let specs = two_specs();
        let calibration = build_calibration(&specs, &direct_input()).unwrap();
        assert_eq!(
            validate_calibration_for_motors(&specs, &calibration),
            Ok(())
        );

        let mut missing = calibration.clone();
        missing.motors.remove("elbow");
        assert!(matches!(
            validate_calibration_for_motors(&specs, &missing),
            Err(CalibrationError::KeyMismatch { .. })
        ));

        let mut wrong_id = calibration.clone();
        wrong_id.motors.get_mut("elbow").unwrap().id = 99;
        assert!(matches!(
            validate_calibration_for_motors(&specs, &wrong_id),
            Err(CalibrationError::MotorSpecMismatch {
                motor,
                expected_id: 2,
                calibration_id: 99,
                ..
            }) if motor == "elbow"
        ));

        let mut wrong_direction = calibration;
        wrong_direction
            .motors
            .get_mut("shoulder")
            .unwrap()
            .direction = Direction::Reversed;
        assert!(matches!(
            validate_calibration_for_motors(&specs, &wrong_direction),
            Err(CalibrationError::MotorSpecMismatch {
                motor,
                expected_direction: Direction::Normal,
                calibration_direction: Direction::Reversed,
                ..
            }) if motor == "shoulder"
        ));
    }

    #[test]
    fn direct_and_sweep_inputs_require_exact_motor_sets() {
        let specs = two_specs();
        let direct = CalibrationInput::Direct {
            motors: BTreeMap::from([(
                "shoulder".to_owned(),
                MotorReading {
                    center: 2,
                    range_min: 1,
                    range_max: 3,
                },
            )]),
        };
        assert!(matches!(
            build_calibration(&specs, &direct),
            Err(CalibrationError::KeyMismatch { .. })
        ));

        let empty_sweep = CalibrationInput::Sweep {
            center: BTreeMap::from([("shoulder".to_owned(), 2), ("elbow".to_owned(), 2)]),
            samples: Vec::new(),
        };
        assert_eq!(
            build_calibration(&specs, &empty_sweep),
            Err(CalibrationError::EmptySamples)
        );
    }

    #[test]
    fn center_must_be_strictly_inside_range() {
        for reading in [
            MotorReading {
                center: 1,
                range_min: 1,
                range_max: 3,
            },
            MotorReading {
                center: 3,
                range_min: 1,
                range_max: 3,
            },
            MotorReading {
                center: 2,
                range_min: 3,
                range_max: 1,
            },
        ] {
            let input = CalibrationInput::Direct {
                motors: BTreeMap::from([("joint".to_owned(), reading)]),
            };
            assert!(matches!(
                build_calibration(&BTreeMap::from([("joint".to_owned(), spec(1))]), &input),
                Err(CalibrationError::InvalidRange { .. })
            ));
        }
    }

    #[test]
    fn centered_normalization_is_piecewise_clamped_and_direction_aware() {
        let normal = MotorCalibration {
            id: 1,
            direction: Direction::Normal,
            range_min: 0,
            center: 25,
            range_max: 100,
        };
        assert_eq!(normalize_centered_raw(&normal, -10).unwrap(), -1.0);
        assert_eq!(normalize_centered_raw(&normal, 25).unwrap(), 0.0);
        assert_eq!(normalize_centered_raw(&normal, 1000).unwrap(), 1.0);
        assert!((normalize_centered_raw(&normal, 50).unwrap() - (1.0 / 3.0)).abs() < 1e-12);

        let reversed = MotorCalibration {
            direction: Direction::Reversed,
            ..normal
        };
        assert_eq!(normalize_centered_raw(&reversed, 0).unwrap(), 1.0);
        assert_eq!(normalize_centered_raw(&reversed, 100).unwrap(), -1.0);
    }

    #[test]
    fn range_tracker_records_extrema_and_derives_the_midpoint() {
        let mut tracker = RangeTracker::new(BTreeMap::from([
            ("shoulder".to_owned(), 100),
            ("elbow".to_owned(), -5),
        ]))
        .unwrap();
        tracker
            .observe(&BTreeMap::from([
                ("shoulder".to_owned(), 40),
                ("elbow".to_owned(), 15),
            ]))
            .unwrap();
        tracker
            .observe(&BTreeMap::from([
                ("shoulder".to_owned(), 180),
                ("elbow".to_owned(), -25),
            ]))
            .unwrap();

        assert!(tracker.is_ready());
        assert_eq!(tracker.ranges()["shoulder"].position, 180);
        assert_eq!(tracker.ranges()["shoulder"].span(), 140);

        let CalibrationInput::Direct { motors } = tracker.into_input().unwrap() else {
            panic!("live range should become direct calibration input");
        };
        assert_eq!(
            motors["shoulder"],
            MotorReading {
                center: 110,
                range_min: 40,
                range_max: 180
            }
        );
        assert_eq!(
            motors["elbow"],
            MotorReading {
                center: -5,
                range_min: -25,
                range_max: 15
            }
        );
    }

    #[test]
    fn range_tracker_requires_every_motor_to_move() {
        let mut tracker = RangeTracker::new(BTreeMap::from([
            ("moving".to_owned(), 10),
            ("still".to_owned(), 20),
        ]))
        .unwrap();
        tracker
            .observe(&BTreeMap::from([
                ("moving".to_owned(), 30),
                ("still".to_owned(), 20),
            ]))
            .unwrap();

        assert!(!tracker.is_ready());
        assert_eq!(tracker.insufficient_travel(), ["still"]);
        assert_eq!(
            tracker.into_input(),
            Err(CalibrationError::InsufficientTravel {
                motors: vec!["still".to_owned()]
            })
        );
    }

    #[test]
    fn range_tracker_rejects_an_empty_or_changing_motor_set() {
        assert_eq!(
            RangeTracker::new(BTreeMap::new()),
            Err(CalibrationError::NoPositions)
        );

        let mut tracker = RangeTracker::new(BTreeMap::from([("joint".to_owned(), 1)])).unwrap();
        let error = tracker
            .observe(&BTreeMap::from([("other".to_owned(), 1)]))
            .expect_err("a different motor set must fail");
        assert_eq!(
            error,
            CalibrationError::MotorSetChanged {
                missing: vec!["joint".to_owned()],
                unexpected: vec!["other".to_owned()],
            }
        );
        // The tracker keeps the ranges it already had.
        assert_eq!(tracker.ranges()["joint"].position, 1);
    }

    #[test]
    fn a_completed_sweep_builds_a_valid_calibration() {
        let mut tracker = RangeTracker::new(BTreeMap::from([
            ("shoulder".to_owned(), 2000),
            ("elbow".to_owned(), 2000),
        ]))
        .unwrap();
        tracker
            .observe(&BTreeMap::from([
                ("shoulder".to_owned(), 900),
                ("elbow".to_owned(), 1200),
            ]))
            .unwrap();
        tracker
            .observe(&BTreeMap::from([
                ("shoulder".to_owned(), 3300),
                ("elbow".to_owned(), 2800),
            ]))
            .unwrap();

        let calibration = build_calibration(&two_specs(), &tracker.into_input().unwrap()).unwrap();
        assert_eq!(calibration.motors["shoulder"].range_min, 900);
        assert_eq!(calibration.motors["shoulder"].center, 2100);
        assert_eq!(calibration.motors["shoulder"].range_max, 3300);
    }

    #[test]
    fn denormalization_inverts_normalization_and_respects_limits() {
        let calibration = MotorCalibration {
            id: 1,
            direction: Direction::Normal,
            range_min: 900,
            center: 2048,
            range_max: 3300,
        };

        for raw in [900, 1500, 2048, 2900, 3300] {
            let normalized = normalize_centered_raw(&calibration, raw).unwrap();
            assert_eq!(
                denormalize_centered_raw(&calibration, normalized).unwrap(),
                raw
            );
        }

        // Commands beyond the calibrated travel stop at the limits.
        assert_eq!(denormalize_centered_raw(&calibration, -5.0).unwrap(), 900);
        assert_eq!(denormalize_centered_raw(&calibration, 5.0).unwrap(), 3300);
        assert_eq!(
            denormalize_centered_raw(&calibration, f64::NAN),
            Err(CalibrationError::NonFiniteNormalizedCommand)
        );
        assert_eq!(
            denormalize_centered_raw(&calibration, f64::INFINITY),
            Err(CalibrationError::NonFiniteNormalizedCommand)
        );

        let reversed = MotorCalibration {
            direction: Direction::Reversed,
            ..calibration
        };
        assert_eq!(denormalize_centered_raw(&reversed, 1.0).unwrap(), 900);
        assert_eq!(denormalize_centered_raw(&reversed, -1.0).unwrap(), 3300);
    }

    #[test]
    fn normalized_positions_accept_exact_limits_and_reject_the_next_float_outside() {
        assert_eq!(NormalizedPosition::new(-1.0).unwrap().get(), -1.0);
        assert_eq!(NormalizedPosition::new(1.0).unwrap().get(), 1.0);

        let above_one = f64::from_bits(1.0_f64.to_bits() + 1);
        let below_negative_one = f64::from_bits((-1.0_f64).to_bits() + 1);
        for value in [above_one, below_negative_one] {
            assert_eq!(
                NormalizedPosition::new(value),
                Err(CalibrationError::NormalizedCommandOutOfRange)
            );
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                NormalizedPosition::new(value),
                Err(CalibrationError::NonFiniteNormalizedCommand)
            );
        }
    }

    #[test]
    fn denormalization_stays_inside_an_extreme_range() {
        let calibration = MotorCalibration {
            id: 1,
            direction: Direction::Normal,
            range_min: i64::MIN,
            center: 0,
            range_max: i64::MAX,
        };

        assert_eq!(
            denormalize_centered_raw(&calibration, -1.0).unwrap(),
            i64::MIN
        );
        assert_eq!(
            denormalize_centered_raw(&calibration, 1.0).unwrap(),
            i64::MAX
        );
        assert_eq!(denormalize_centered_raw(&calibration, 0.0).unwrap(), 0);
    }

    #[test]
    fn normalization_handles_the_full_i64_range() {
        let calibration = MotorCalibration {
            id: 1,
            direction: Direction::Normal,
            range_min: i64::MIN,
            center: 0,
            range_max: i64::MAX,
        };
        assert_eq!(
            normalize_centered_raw(&calibration, i64::MIN).unwrap(),
            -1.0
        );
        assert_eq!(normalize_centered_raw(&calibration, i64::MAX).unwrap(), 1.0);
    }
}
