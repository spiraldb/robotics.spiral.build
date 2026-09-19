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
pub enum MotorDirection {
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
    id: u32,
    direction: MotorDirection,
    model: Option<String>,
}

impl MotorSpec {
    /// Build the default motor map for a new SO-101 robot instance.
    pub fn so101_arm() -> BTreeMap<String, Self> {
        spiral_robotics_common::robots::document::So101::motors()
            .into_iter()
            .map(|(name, id)| {
                (
                    name.to_owned(),
                    Self::new(id, MotorDirection::Normal)
                        .with_model(spiral_robotics_common::robots::document::So101::motor_model()),
                )
            })
            .collect()
    }

    /// Convert STS3215 encoder counts to output-shaft degrees.
    pub(crate) fn degrees_from_encoder_counts(counts: i64) -> f64 {
        counts as f64 * 360.0 / Self::encoder_counts_per_turn() as f64
    }

    pub(crate) const fn encoder_counts_per_turn() -> i64 {
        4096
    }

    /// Construct a motor definition without any adapter-specific details.
    pub fn new(id: u32, direction: MotorDirection) -> Self {
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

    pub const fn id(&self) -> u32 {
        self.id
    }

    pub const fn direction(&self) -> MotorDirection {
        self.direction
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

struct MotorName;

impl MotorName {
    fn validate(name: &str) -> Result<(), CalibrationError> {
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
}

/// Raw readings for one motor after a completed range sweep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotorReading {
    center: i64,
    range_min: i64,
    range_max: i64,
}

impl MotorReading {
    pub const fn new(center: i64, range_min: i64, range_max: i64) -> Self {
        Self {
            center,
            range_min,
            range_max,
        }
    }

    pub const fn center(self) -> i64 {
        self.center
    }

    pub const fn range_min(self) -> i64 {
        self.range_min
    }

    pub const fn range_max(self) -> i64 {
        self.range_max
    }
}

/// Input accepted by [`CalibrationInput::build`].
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
    id: u32,
    direction: MotorDirection,
    reading: MotorReading,
}

/// Version and coordinate metadata persisted alongside motor calibrations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalibrationFormat {
    version: u32,
    units: String,
    coordinate_space: String,
}

impl CalibrationFormat {
    pub fn new(
        version: u32,
        units: impl Into<String>,
        coordinate_space: impl Into<String>,
    ) -> Self {
        Self {
            version,
            units: units.into(),
            coordinate_space: coordinate_space.into(),
        }
    }

    pub const fn version(&self) -> u32 {
        self.version
    }

    pub fn units(&self) -> &str {
        &self.units
    }

    pub fn coordinate_space(&self) -> &str {
        &self.coordinate_space
    }
}

impl Default for CalibrationFormat {
    fn default() -> Self {
        Self::new(
            CALIBRATION_FORMAT_VERSION,
            CALIBRATION_UNITS,
            CALIBRATION_COORDINATE_SPACE,
        )
    }
}

/// A validated, deterministic calibration document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Calibration {
    format: CalibrationFormat,
    motors: BTreeMap<String, MotorCalibration>,
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
        expected_direction: MotorDirection,
        calibration_direction: MotorDirection,
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

    pub(crate) const fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for NormalizedPosition {
    type Error = CalibrationError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<NormalizedPosition> for f64 {
    fn from(value: NormalizedPosition) -> Self {
        value.get()
    }
}

impl Error for CalibrationError {}

/// A borrowed motor specification map whose names and bus IDs are valid.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MotorSpecifications<'a>(&'a BTreeMap<String, MotorSpec>);

impl<'a> TryFrom<&'a BTreeMap<String, MotorSpec>> for MotorSpecifications<'a> {
    type Error = CalibrationError;

    fn try_from(specifications: &'a BTreeMap<String, MotorSpec>) -> Result<Self, Self::Error> {
        if specifications.is_empty() {
            return Err(CalibrationError::NoMotors);
        }

        let mut ids = BTreeMap::<u32, &str>::new();
        for (name, specification) in specifications {
            MotorName::validate(name)?;
            if let Some(first) = ids.insert(specification.id, name) {
                return Err(CalibrationError::DuplicateMotorId {
                    id: specification.id,
                    first: first.to_owned(),
                    second: name.clone(),
                });
            }
        }
        Ok(Self(specifications))
    }
}

impl<'a> MotorSpecifications<'a> {
    const fn as_map(self) -> &'a BTreeMap<String, MotorSpec> {
        self.0
    }

    fn validate_keys<T>(
        self,
        values: &BTreeMap<String, T>,
        context: &str,
    ) -> Result<(), CalibrationError> {
        let expected = self.0.keys().cloned().collect::<BTreeSet<_>>();
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
}

impl MotorSpec {
    /// Validate a complete named motor set.
    ///
    /// This associated operation keeps the validation policy with the motor
    /// definition without exposing the borrowed wrapper used internally by
    /// calibration and control.
    pub fn validate_set(specifications: &BTreeMap<String, Self>) -> Result<(), CalibrationError> {
        MotorSpecifications::try_from(specifications).map(|_| ())
    }
}

impl Calibration {
    /// Construct and validate a calibration imported from a persistence edge.
    pub fn from_persisted(
        format: CalibrationFormat,
        motors: BTreeMap<String, MotorCalibration>,
    ) -> Result<Self, CalibrationError> {
        let calibration = Self { format, motors };
        calibration.validate()?;
        Ok(calibration)
    }

    fn from_current_format(
        motors: BTreeMap<String, MotorCalibration>,
    ) -> Result<Self, CalibrationError> {
        Self::from_persisted(CalibrationFormat::default(), motors)
    }

    pub const fn format(&self) -> &CalibrationFormat {
        &self.format
    }

    pub const fn motors(&self) -> &BTreeMap<String, MotorCalibration> {
        &self.motors
    }

    /// Validate a fully materialized calibration, including deserialized data.
    pub fn validate(&self) -> Result<(), CalibrationError> {
        let calibration = self;
        if calibration.format.version != CALIBRATION_FORMAT_VERSION {
            return Err(CalibrationError::UnsupportedFormatVersion(
                calibration.format.version,
            ));
        }
        if calibration.format.units != CALIBRATION_UNITS {
            return Err(CalibrationError::UnsupportedUnits(
                calibration.format.units.clone(),
            ));
        }
        if calibration.format.coordinate_space != CALIBRATION_COORDINATE_SPACE {
            return Err(CalibrationError::UnsupportedCoordinateSpace(
                calibration.format.coordinate_space.clone(),
            ));
        }
        if calibration.motors.is_empty() {
            return Err(CalibrationError::NoMotors);
        }

        let mut ids = BTreeMap::<u32, &str>::new();
        for (name, motor) in &calibration.motors {
            MotorName::validate(name)?;
            if let Some(first) = ids.insert(motor.id, name) {
                return Err(CalibrationError::DuplicateMotorId {
                    id: motor.id,
                    first: first.to_owned(),
                    second: name.clone(),
                });
            }
            motor.reading.validate_for(name)?;
        }
        Ok(())
    }

    /// Validate that a calibration belongs to one exact configured motor set.
    ///
    /// A calibration range is meaningful only for the named physical motor it was
    /// measured from. Matching names alone is insufficient: changing a bus ID or
    /// direction while retaining an old range would make normalized commands refer
    /// to different hardware semantics.
    pub fn validate_for_motor_specs(
        &self,
        specs: &BTreeMap<String, MotorSpec>,
    ) -> Result<(), CalibrationError> {
        let calibration = self;
        let specifications = MotorSpecifications::try_from(specs)?;
        calibration.validate()?;
        specifications.validate_keys(&calibration.motors, "calibration")?;

        for (name, specification) in specifications.as_map() {
            let motor = calibration
                .motors
                .get(name)
                .expect("validated calibration keys must contain every configured motor");
            if motor.id != specification.id || motor.direction != specification.direction {
                return Err(CalibrationError::MotorSpecMismatch {
                    motor: name.clone(),
                    expected_id: specification.id,
                    calibration_id: motor.id,
                    expected_direction: specification.direction,
                    calibration_direction: motor.direction,
                });
            }
        }
        Ok(())
    }
}

impl CalibrationInput {
    /// Build a calibration from direct readings or a raw range sweep.
    pub fn build(
        &self,
        specs: &BTreeMap<String, MotorSpec>,
    ) -> Result<Calibration, CalibrationError> {
        let input = self;
        let specifications = MotorSpecifications::try_from(specs)?;
        let specs = specifications.as_map();

        let readings = match input {
            CalibrationInput::Direct { motors } => {
                specifications.validate_keys(motors, "direct readings")?;
                motors.clone()
            }
            CalibrationInput::Sweep { center, samples } => {
                specifications.validate_keys(center, "center readings")?;
                if samples.is_empty() {
                    return Err(CalibrationError::EmptySamples);
                }

                let mut extrema = specs
                    .keys()
                    .map(|name| (name.clone(), (i64::MAX, i64::MIN)))
                    .collect::<BTreeMap<_, _>>();
                for (index, sample) in samples.iter().enumerate() {
                    specifications.validate_keys(sample, &format!("sweep sample {index}"))?;
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
                            MotorReading::new(center, range_min, range_max),
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
                reading.validate_for(name)?;
                Ok((
                    name.clone(),
                    MotorCalibration::new(spec.id, spec.direction, reading)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, CalibrationError>>()?;

        Calibration::from_current_format(motors)
    }
}

/// Normalize an unhomed raw reading to `[-1.0, 1.0]` around its center.
///
/// This is Spiral Robotics' protocol-neutral centered normalization, not a hardware
/// register encoding or LeRobot wire-format conversion. Values outside the
/// observed range are clamped, asymmetric sides are scaled independently, and
/// [`MotorDirection::Reversed`] negates the result.
impl MotorCalibration {
    pub fn new(
        id: u32,
        direction: MotorDirection,
        reading: MotorReading,
    ) -> Result<Self, CalibrationError> {
        reading.validate_for(&format!("id {id}"))?;
        Ok(Self {
            id,
            direction,
            reading,
        })
    }

    pub const fn id(self) -> u32 {
        self.id
    }

    pub const fn direction(self) -> MotorDirection {
        self.direction
    }

    pub const fn reading(self) -> MotorReading {
        self.reading
    }

    pub(crate) fn normalize_raw(&self, raw: i64) -> Result<f64, CalibrationError> {
        let calibration = self;
        let bounded = raw.clamp(calibration.reading.range_min, calibration.reading.range_max);
        let (numerator, denominator) = if bounded < calibration.reading.center {
            (
                i128::from(bounded) - i128::from(calibration.reading.center),
                i128::from(calibration.reading.center) - i128::from(calibration.reading.range_min),
            )
        } else {
            (
                i128::from(bounded) - i128::from(calibration.reading.center),
                i128::from(calibration.reading.range_max) - i128::from(calibration.reading.center),
            )
        };
        let normalized = numerator as f64 / denominator as f64;
        Ok(match calibration.direction {
            MotorDirection::Normal => normalized,
            MotorDirection::Reversed => -normalized,
        })
    }

    /// Convert a raw reading to degrees from this motor's calibrated center,
    /// preserving its configured joint direction.
    pub(crate) fn degrees_from_center(&self, raw: i64) -> f64 {
        let degrees =
            MotorSpec::degrees_from_encoder_counts(raw.saturating_sub(self.reading.center));
        match self.direction {
            MotorDirection::Normal => degrees,
            MotorDirection::Reversed => -degrees,
        }
    }
}

/// The travel one motor has shown so far during a live calibration sweep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MotorTravelRange {
    minimum: i64,
    position: i64,
    maximum: i64,
}

impl MotorTravelRange {
    pub(crate) const fn minimum(self) -> i64 {
        self.minimum
    }

    pub(crate) const fn position(self) -> i64 {
        self.position
    }

    pub(crate) const fn maximum(self) -> i64 {
        self.maximum
    }

    /// Travel observed so far, saturating rather than overflowing.
    fn span(&self) -> i64 {
        self.maximum.saturating_sub(self.minimum)
    }
}

/// Accumulates the extrema of every motor while an operator sweeps the arm.
///
/// The terminal calibration command and the studio's control loop both feed
/// this the same way, so "did this joint move far enough" has exactly one
/// definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MotorRangeTracker {
    ranges: BTreeMap<String, MotorTravelRange>,
}

/// Encoder counts a joint must travel before its range is considered usable.
///
/// Two counts is the smallest span that leaves the center strictly inside the
/// range, which [`CalibrationInput::build`] requires.
pub(crate) const MINIMUM_CALIBRATION_TRAVEL: i64 = 2;

impl MotorRangeTracker {
    /// Start tracking from an initial reading of every motor.
    pub(crate) fn new(positions: BTreeMap<String, i64>) -> Result<Self, CalibrationError> {
        if positions.is_empty() {
            return Err(CalibrationError::NoPositions);
        }
        Ok(Self {
            ranges: positions
                .into_iter()
                .map(|(name, position)| {
                    (
                        name,
                        MotorTravelRange {
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
    pub(crate) fn observe(
        &mut self,
        positions: &BTreeMap<String, i64>,
    ) -> Result<(), CalibrationError> {
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
    pub(crate) fn ranges(&self) -> &BTreeMap<String, MotorTravelRange> {
        &self.ranges
    }

    /// Motors that have not yet moved far enough to produce a valid range.
    pub(crate) fn insufficient_travel(&self) -> Vec<String> {
        self.ranges
            .iter()
            .filter(|(_, range)| range.span() < MINIMUM_CALIBRATION_TRAVEL)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// True once every motor has swept a usable range.
    pub(crate) fn is_ready(&self) -> bool {
        self.insufficient_travel().is_empty()
    }

    /// Reduce the sweep to calibration input, centering each motor on the
    /// midpoint of its observed travel.
    pub(crate) fn into_input(self) -> Result<CalibrationInput, CalibrationError> {
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
/// This is the exact inverse of [`MotorCalibration::normalize_raw`] up to rounding: the
/// command is clamped to `[-1.0, 1.0]`, the direction is undone, each side of
/// the center is scaled by its own half-range, and the result is clamped to the
/// calibrated range so a caller can never command a joint past a limit it was
/// calibrated against. Non-finite input is rejected: silently translating a
/// corrupt command into motion toward center can move a physical arm while
/// hiding the upstream fault.
impl MotorCalibration {
    pub(crate) fn denormalize(&self, normalized: f64) -> Result<i64, CalibrationError> {
        let calibration = self;
        if !normalized.is_finite() {
            return Err(CalibrationError::NonFiniteNormalizedCommand);
        }
        let command = normalized;
        let command = command.clamp(-1.0, 1.0);
        let command = match calibration.direction {
            MotorDirection::Normal => command,
            MotorDirection::Reversed => -command,
        };

        let half_range = if command < 0.0 {
            i128::from(calibration.reading.center) - i128::from(calibration.reading.range_min)
        } else {
            i128::from(calibration.reading.range_max) - i128::from(calibration.reading.center)
        };
        let offset = command * half_range as f64;
        let raw = (calibration.reading.center as f64 + offset).round();

        // f64 cannot represent every i64, so clamp in float space before casting.
        Ok(if raw <= calibration.reading.range_min as f64 {
            calibration.reading.range_min
        } else if raw >= calibration.reading.range_max as f64 {
            calibration.reading.range_max
        } else {
            (raw as i64).clamp(calibration.reading.range_min, calibration.reading.range_max)
        })
    }
}

impl MotorReading {
    fn validate_for(self, name: &str) -> Result<(), CalibrationError> {
        if self.range_min < self.center && self.center < self.range_max {
            Ok(())
        } else {
            Err(CalibrationError::InvalidRange {
                motor: name.to_owned(),
                range_min: self.range_min,
                center: self.center,
                range_max: self.range_max,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motor_spec(id: u32) -> MotorSpec {
        MotorSpec::new(id, MotorDirection::Normal)
    }

    fn two_specs() -> BTreeMap<String, MotorSpec> {
        BTreeMap::from([
            ("elbow".to_owned(), motor_spec(2)),
            ("shoulder".to_owned(), motor_spec(1)),
        ])
    }

    fn direct_input() -> CalibrationInput {
        CalibrationInput::Direct {
            motors: BTreeMap::from([
                ("shoulder".to_owned(), MotorReading::new(2100, 900, 3300)),
                ("elbow".to_owned(), MotorReading::new(500, 100, 800)),
            ]),
        }
    }

    #[test]
    fn motor_spec_has_only_explicit_domain_fields() {
        let spec = MotorSpec::new(7, MotorDirection::Normal).with_model("sts3215");
        assert_eq!(spec.id(), 7);
        assert_eq!(spec.direction(), MotorDirection::Normal);
        assert_eq!(spec.model(), Some("sts3215"));
    }

    #[test]
    fn so101_motor_map_owns_hardware_ids_and_encoder_units() {
        let motors = MotorSpec::so101_arm();
        assert_eq!(motors.len(), 6);
        assert_eq!(motors["shoulder_pan"].id(), 1);
        assert_eq!(motors["gripper"].id(), 6);
        assert!(
            motors
                .values()
                .all(|motor| motor.model() == Some("sts3215"))
        );
        assert_eq!(MotorSpec::encoder_counts_per_turn(), 4096);
        assert_eq!(MotorSpec::degrees_from_encoder_counts(4096), 360.0);
        assert_eq!(MotorSpec::degrees_from_encoder_counts(-1024), -90.0);
    }

    #[test]
    fn direct_input_preserves_truthful_unhomed_raw_values() {
        let mut specs = two_specs();
        specs.get_mut("elbow").unwrap().direction = MotorDirection::Reversed;
        let calibration = direct_input().build(&specs).unwrap();

        assert_eq!(calibration.format().coordinate_space(), "unhomed_raw");
        assert_eq!(calibration.motors()["shoulder"].reading().center(), 2100);
        assert_eq!(calibration.motors()["shoulder"].reading().range_min(), 900);
        assert_eq!(calibration.motors()["shoulder"].reading().range_max(), 3300);
        assert_eq!(
            calibration.motors()["elbow"].direction(),
            MotorDirection::Reversed
        );
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
        let calibration = input.build(&two_specs()).unwrap();
        assert_eq!(calibration.motors()["shoulder"].reading().range_min(), 1500);
        assert_eq!(calibration.motors()["shoulder"].reading().range_max(), 2800);
        assert_eq!(calibration.motors()["elbow"].reading().range_min(), 900);
        assert_eq!(calibration.motors()["elbow"].reading().range_max(), 2300);
    }

    #[test]
    fn validates_motor_specs() {
        assert_eq!(
            MotorSpecifications::try_from(&BTreeMap::<String, MotorSpec>::new()).map(|_| ()),
            Err(CalibrationError::NoMotors)
        );

        let mut specs = two_specs();
        specs.insert("bad name".to_owned(), motor_spec(3));
        assert!(matches!(
            MotorSpecifications::try_from(&specs).map(|_| ()),
            Err(CalibrationError::InvalidMotorName { .. })
        ));

        let mut specs = two_specs();
        specs.get_mut("elbow").unwrap().id = 1;
        assert!(matches!(
            MotorSpecifications::try_from(&specs).map(|_| ()),
            Err(CalibrationError::DuplicateMotorId { .. })
        ));
    }

    #[test]
    fn calibration_must_match_motor_names_ids_and_directions() {
        let specs = two_specs();
        let calibration = direct_input().build(&specs).unwrap();
        assert_eq!(calibration.validate_for_motor_specs(&specs), Ok(()));

        let mut missing = calibration.clone();
        missing.motors.remove("elbow");
        assert!(matches!(
            missing.validate_for_motor_specs(&specs),
            Err(CalibrationError::KeyMismatch { .. })
        ));

        let mut wrong_id = calibration.clone();
        wrong_id.motors.get_mut("elbow").unwrap().id = 99;
        assert!(matches!(
            wrong_id.validate_for_motor_specs(&specs),
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
            .direction = MotorDirection::Reversed;
        assert!(matches!(
            wrong_direction.validate_for_motor_specs(&specs),
            Err(CalibrationError::MotorSpecMismatch {
                motor,
                expected_direction: MotorDirection::Normal,
                calibration_direction: MotorDirection::Reversed,
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
            direct.build(&specs),
            Err(CalibrationError::KeyMismatch { .. })
        ));

        let empty_sweep = CalibrationInput::Sweep {
            center: BTreeMap::from([("shoulder".to_owned(), 2), ("elbow".to_owned(), 2)]),
            samples: Vec::new(),
        };
        assert_eq!(
            empty_sweep.build(&specs),
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
                input.build(&BTreeMap::from([("joint".to_owned(), motor_spec(1))])),
                Err(CalibrationError::InvalidRange { .. })
            ));
        }
    }

    #[test]
    fn centered_normalization_is_piecewise_clamped_and_direction_aware() {
        let normal =
            MotorCalibration::new(1, MotorDirection::Normal, MotorReading::new(25, 0, 100))
                .unwrap();
        assert_eq!(normal.normalize_raw(-10).unwrap(), -1.0);
        assert_eq!(normal.normalize_raw(25).unwrap(), 0.0);
        assert_eq!(normal.normalize_raw(1000).unwrap(), 1.0);
        assert!((normal.normalize_raw(50).unwrap() - (1.0 / 3.0)).abs() < 1e-12);

        let reversed =
            MotorCalibration::new(1, MotorDirection::Reversed, MotorReading::new(25, 0, 100))
                .unwrap();
        assert_eq!(reversed.normalize_raw(0).unwrap(), 1.0);
        assert_eq!(reversed.normalize_raw(100).unwrap(), -1.0);
    }

    #[test]
    fn range_tracker_records_extrema_and_derives_the_midpoint() {
        let mut tracker = MotorRangeTracker::new(BTreeMap::from([
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
        assert_eq!(tracker.ranges()["shoulder"].position(), 180);
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
        let mut tracker = MotorRangeTracker::new(BTreeMap::from([
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
            MotorRangeTracker::new(BTreeMap::new()),
            Err(CalibrationError::NoPositions)
        );

        let mut tracker =
            MotorRangeTracker::new(BTreeMap::from([("joint".to_owned(), 1)])).unwrap();
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
        assert_eq!(tracker.ranges()["joint"].position(), 1);
    }

    #[test]
    fn a_completed_sweep_builds_a_valid_calibration() {
        let mut tracker = MotorRangeTracker::new(BTreeMap::from([
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

        let calibration = tracker.into_input().unwrap().build(&two_specs()).unwrap();
        assert_eq!(calibration.motors()["shoulder"].reading().range_min(), 900);
        assert_eq!(calibration.motors()["shoulder"].reading().center(), 2100);
        assert_eq!(calibration.motors()["shoulder"].reading().range_max(), 3300);
    }

    #[test]
    fn denormalization_inverts_normalization_and_respects_limits() {
        let calibration = MotorCalibration::new(
            1,
            MotorDirection::Normal,
            MotorReading::new(2048, 900, 3300),
        )
        .unwrap();

        for raw in [900, 1500, 2048, 2900, 3300] {
            let normalized = calibration.normalize_raw(raw).unwrap();
            assert_eq!(calibration.denormalize(normalized).unwrap(), raw);
        }

        // Commands beyond the calibrated travel stop at the limits.
        assert_eq!(calibration.denormalize(-5.0).unwrap(), 900);
        assert_eq!(calibration.denormalize(5.0).unwrap(), 3300);
        assert_eq!(
            calibration.denormalize(f64::NAN),
            Err(CalibrationError::NonFiniteNormalizedCommand)
        );
        assert_eq!(
            calibration.denormalize(f64::INFINITY),
            Err(CalibrationError::NonFiniteNormalizedCommand)
        );

        let reversed = MotorCalibration::new(
            1,
            MotorDirection::Reversed,
            MotorReading::new(2048, 900, 3300),
        )
        .unwrap();
        assert_eq!(reversed.denormalize(1.0).unwrap(), 900);
        assert_eq!(reversed.denormalize(-1.0).unwrap(), 3300);
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
        let calibration = MotorCalibration::new(
            1,
            MotorDirection::Normal,
            MotorReading::new(0, i64::MIN, i64::MAX),
        )
        .unwrap();

        assert_eq!(calibration.denormalize(-1.0).unwrap(), i64::MIN);
        assert_eq!(calibration.denormalize(1.0).unwrap(), i64::MAX);
        assert_eq!(calibration.denormalize(0.0).unwrap(), 0);
    }

    #[test]
    fn normalization_handles_the_full_i64_range() {
        let calibration = MotorCalibration::new(
            1,
            MotorDirection::Normal,
            MotorReading::new(0, i64::MIN, i64::MAX),
        )
        .unwrap();
        assert_eq!(calibration.normalize_raw(i64::MIN).unwrap(), -1.0);
        assert_eq!(calibration.normalize_raw(i64::MAX).unwrap(), 1.0);
    }
}
