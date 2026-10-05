//! Protocol-neutral motor calibration: measuring ranges and normalizing positions.
//!
//! This module deliberately does not talk to a motor bus or synthesize
//! hardware-specific register values. A sweep's ranges stay in the unhomed raw
//! coordinate system in which they were observed; each motor's configuration
//! carries its own range, and normalization pairs it with that motor's direction.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use spiral_robotics_common::robots::config::{
    MotorConfig, MotorDirection, MotorRange, RobotConfig,
};

/// Resolution of the STS3215 encoder behind every raw position.
pub(crate) struct EncoderCounts;

impl EncoderCounts {
    pub(crate) const PER_TURN: i64 = 4096;

    /// Convert encoder counts to output-shaft degrees.
    pub(crate) fn degrees(counts: i64) -> f64 {
        counts as f64 * 360.0 / Self::PER_TURN as f64
    }
}

/// A validation error encountered while measuring or applying a calibration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalibrationError {
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
        reason: String,
    },
    NonFiniteNormalizedCommand,
    NormalizedCommandOutOfRange,
}

impl fmt::Display for CalibrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
            Self::InvalidRange { motor, reason } => {
                write!(formatter, "motor {motor:?} has an invalid {reason}")
            }
            Self::NonFiniteNormalizedCommand => {
                formatter.write_str("normalized motor command must be finite")
            }
            Self::NormalizedCommandOutOfRange => {
                formatter.write_str("normalized motor command must be within [-1, 1]")
            }
        }
    }
}

impl Error for CalibrationError {}

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

/// Every motor of one arm with its measured range, keyed by motor name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Calibration {
    motors: BTreeMap<String, CalibratedMotor>,
}

impl Calibration {
    /// `None` until every configured motor has been measured.
    pub(crate) fn of(config: &RobotConfig) -> Option<Self> {
        let motors = config
            .motors()
            .iter()
            .map(|(name, motor)| Some((name.clone(), CalibratedMotor::of(motor)?)))
            .collect::<Option<_>>()?;
        Some(Self { motors })
    }

    pub(crate) const fn motors(&self) -> &BTreeMap<String, CalibratedMotor> {
        &self.motors
    }
}

/// A configured motor whose range has been measured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CalibratedMotor {
    direction: MotorDirection,
    range: MotorRange,
}

impl CalibratedMotor {
    pub(crate) fn of(motor: &MotorConfig) -> Option<Self> {
        Some(Self {
            direction: motor.direction(),
            range: motor.calibration()?,
        })
    }

    pub(crate) const fn range(self) -> MotorRange {
        self.range
    }

    /// Normalize an unhomed raw reading to `[-1.0, 1.0]` around its center.
    ///
    /// This is Spiral Robotics' protocol-neutral centered normalization, not a
    /// hardware register encoding or LeRobot wire-format conversion. Values
    /// outside the measured range are clamped, asymmetric sides are scaled
    /// independently, and [`MotorDirection::Reversed`] negates the result.
    pub(crate) fn normalize_raw(self, raw: i64) -> f64 {
        let (range_min, center, range_max) = self.bounds();
        let bounded = raw.clamp(range_min, range_max);
        let half_range = if bounded < center {
            i128::from(center) - i128::from(range_min)
        } else {
            i128::from(range_max) - i128::from(center)
        };
        let normalized = (i128::from(bounded) - i128::from(center)) as f64 / half_range as f64;
        match self.direction {
            MotorDirection::Normal => normalized,
            MotorDirection::Reversed => -normalized,
        }
    }

    /// Map a normalized `[-1.0, 1.0]` command back to an unhomed raw position.
    ///
    /// The exact inverse of [`Self::normalize_raw`] up to rounding: the command
    /// is clamped to `[-1.0, 1.0]`, the direction is undone, each side of the
    /// center is scaled by its own half-range, and the result is clamped to the
    /// measured range so a caller can never command a joint past a limit it was
    /// calibrated against. Non-finite input is rejected: silently translating a
    /// corrupt command into motion toward center can move a physical arm while
    /// hiding the upstream fault.
    pub(crate) fn denormalize(self, normalized: f64) -> Result<i64, CalibrationError> {
        if !normalized.is_finite() {
            return Err(CalibrationError::NonFiniteNormalizedCommand);
        }
        let (range_min, center, range_max) = self.bounds();
        let command = normalized.clamp(-1.0, 1.0);
        let command = match self.direction {
            MotorDirection::Normal => command,
            MotorDirection::Reversed => -command,
        };
        let half_range = if command < 0.0 {
            i128::from(center) - i128::from(range_min)
        } else {
            i128::from(range_max) - i128::from(center)
        };
        let raw = (center as f64 + command * half_range as f64).round();

        // f64 cannot represent every i64, so clamp in float space before casting.
        Ok(if raw <= range_min as f64 {
            range_min
        } else if raw >= range_max as f64 {
            range_max
        } else {
            (raw as i64).clamp(range_min, range_max)
        })
    }

    /// Convert a raw reading to degrees from this motor's calibrated center,
    /// preserving its configured joint direction.
    pub(crate) fn degrees_from_center(self, raw: i64) -> f64 {
        let degrees = EncoderCounts::degrees(raw.saturating_sub(self.range.center()));
        match self.direction {
            MotorDirection::Normal => degrees,
            MotorDirection::Reversed => -degrees,
        }
    }

    const fn bounds(self) -> (i64, i64, i64) {
        (
            self.range.range_min(),
            self.range.center(),
            self.range.range_max(),
        )
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
/// The control loop feeds this every sweep sample, so "did this joint move
/// far enough" has exactly one definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MotorRangeTracker {
    ranges: BTreeMap<String, MotorTravelRange>,
}

/// Encoder counts a joint must travel before its range is considered usable.
///
/// Two counts is the smallest span that leaves the center strictly inside the
/// range, which [`MotorRange`] requires.
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

    /// Reduce the sweep to measured ranges, centering each motor on the
    /// midpoint of its observed travel.
    pub(crate) fn into_ranges(self) -> Result<BTreeMap<String, MotorRange>, CalibrationError> {
        let insufficient = self.insufficient_travel();
        if !insufficient.is_empty() {
            return Err(CalibrationError::InsufficientTravel {
                motors: insufficient,
            });
        }

        self.ranges
            .into_iter()
            .map(|(name, travel)| {
                let center = (i128::from(travel.minimum) + i128::from(travel.maximum)) / 2;
                let center =
                    i64::try_from(center).expect("the midpoint of two i64 values must fit in i64");
                MotorRange::new(travel.minimum, center, travel.maximum)
                    .map(|range| (name.clone(), range))
                    .map_err(|reason| CalibrationError::InvalidRange {
                        motor: name,
                        reason,
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motor(
        direction: MotorDirection,
        range_min: i64,
        center: i64,
        range_max: i64,
    ) -> CalibratedMotor {
        let range = MotorRange::new(range_min, center, range_max).unwrap();
        CalibratedMotor::of(&MotorConfig::new(1, direction).with_calibration(range)).unwrap()
    }

    #[test]
    fn encoder_counts_convert_to_output_shaft_degrees() {
        assert_eq!(EncoderCounts::degrees(4096), 360.0);
        assert_eq!(EncoderCounts::degrees(-1024), -90.0);
    }

    #[test]
    fn an_unmeasured_motor_has_no_calibration() {
        assert!(CalibratedMotor::of(&MotorConfig::new(1, MotorDirection::Normal)).is_none());
    }

    #[test]
    fn centered_normalization_is_piecewise_clamped_and_direction_aware() {
        let normal = motor(MotorDirection::Normal, 0, 25, 100);
        assert_eq!(normal.normalize_raw(-10), -1.0);
        assert_eq!(normal.normalize_raw(25), 0.0);
        assert_eq!(normal.normalize_raw(1000), 1.0);
        assert!((normal.normalize_raw(50) - (1.0 / 3.0)).abs() < 1e-12);

        let reversed = motor(MotorDirection::Reversed, 0, 25, 100);
        assert_eq!(reversed.normalize_raw(0), 1.0);
        assert_eq!(reversed.normalize_raw(100), -1.0);
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

        let ranges = tracker.into_ranges().unwrap();
        assert_eq!(ranges["shoulder"], MotorRange::new(40, 110, 180).unwrap());
        assert_eq!(ranges["elbow"], MotorRange::new(-25, -5, 15).unwrap());
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
            tracker.into_ranges(),
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
    fn denormalization_inverts_normalization_and_respects_limits() {
        let calibration = motor(MotorDirection::Normal, 900, 2048, 3300);

        for raw in [900, 1500, 2048, 2900, 3300] {
            let normalized = calibration.normalize_raw(raw);
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

        let reversed = motor(MotorDirection::Reversed, 900, 2048, 3300);
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
        let calibration = motor(MotorDirection::Normal, i64::MIN, 0, i64::MAX);

        assert_eq!(calibration.denormalize(-1.0).unwrap(), i64::MIN);
        assert_eq!(calibration.denormalize(1.0).unwrap(), i64::MAX);
        assert_eq!(calibration.denormalize(0.0).unwrap(), 0);
    }

    #[test]
    fn normalization_handles_the_full_i64_range() {
        let calibration = motor(MotorDirection::Normal, i64::MIN, 0, i64::MAX);
        assert_eq!(calibration.normalize_raw(i64::MIN), -1.0);
        assert_eq!(calibration.normalize_raw(i64::MAX), 1.0);
    }
}
