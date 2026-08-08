//! Protocol-neutral motor calibration data and validation.
//!
//! This module deliberately does not talk to a motor bus or synthesize
//! hardware-specific register values. Callers provide either center/min/max
//! readings or a center reading followed by raw sweep samples. Stored values
//! remain in the unhomed raw coordinate system in which they were observed.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// The current on-disk calibration format version.
pub const CALIBRATION_FORMAT_VERSION: u32 = 1;

/// Units used by calibrations produced by this module.
pub const CALIBRATION_UNITS: &str = "raw";

/// Coordinate system used by calibrations produced by this module.
pub const CALIBRATION_COORDINATE_SPACE: &str = "unhomed_raw";

/// Whether increasing raw readings represent the normal or reversed direction.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Normal,
    Reversed,
}

/// Static configuration for one motor.
///
/// Unknown fields are retained so a future protocol adapter can keep its model,
/// register, or transport details beside the protocol-neutral fields.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MotorSpec {
    pub id: u32,
    #[serde(default)]
    pub direction: Direction,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Raw readings for one motor after a completed range sweep.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MotorReading {
    pub center: i64,
    pub range_min: i64,
    pub range_max: i64,
}

/// Input accepted by [`build_calibration`].
///
/// `Direct` accepts readings already reduced to extrema. `Sweep` derives the
/// extrema from one or more complete snapshots. Both forms reject misspelled
/// fields, missing motors, unexpected motors, and duplicate motor keys.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CalibrationInput {
    Direct {
        motors: BTreeMap<String, MotorReading>,
    },
    Sweep {
        center: BTreeMap<String, i64>,
        samples: Vec<BTreeMap<String, i64>>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectInput {
    motors: UniqueMap<MotorReading>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SweepInput {
    center: UniqueMap<i64>,
    samples: Vec<UniqueMap<i64>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CalibrationInputWire {
    Direct(DirectInput),
    Sweep(SweepInput),
}

impl<'de> Deserialize<'de> for CalibrationInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match CalibrationInputWire::deserialize(deserializer)? {
            CalibrationInputWire::Direct(input) => Self::Direct {
                motors: input.motors.0,
            },
            CalibrationInputWire::Sweep(input) => Self::Sweep {
                center: input.center.0,
                samples: input.samples.into_iter().map(|sample| sample.0).collect(),
            },
        })
    }
}

/// Protocol-neutral calibration for one motor.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MotorCalibration {
    pub id: u32,
    pub direction: Direction,
    pub range_min: i64,
    pub center: i64,
    pub range_max: i64,
}

/// A validated, deterministic calibration document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Calibration {
    pub format_version: u32,
    pub units: String,
    pub coordinate_space: String,
    pub motors: BTreeMap<String, MotorCalibration>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CalibrationWire {
    format_version: u32,
    units: String,
    coordinate_space: String,
    motors: UniqueMap<MotorCalibration>,
}

impl<'de> Deserialize<'de> for Calibration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CalibrationWire::deserialize(deserializer)?;
        let calibration = Self {
            format_version: wire.format_version,
            units: wire.units,
            coordinate_space: wire.coordinate_space,
            motors: wire.motors.0,
        };
        validate_calibration(&calibration).map_err(de::Error::custom)?;
        Ok(calibration)
    }
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
    ReservedExtraField {
        motor: String,
        field: String,
    },
    KeyMismatch {
        context: String,
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
    EmptySamples,
    InvalidRange {
        motor: String,
        range_min: i64,
        center: i64,
        range_max: i64,
    },
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
            Self::ReservedExtraField { motor, field } => write!(
                formatter,
                "motor {motor:?} contains reserved field {field:?} in its flattened extra fields"
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
            Self::EmptySamples => {
                formatter.write_str("a calibration sweep must contain at least one sample")
            }
            Self::InvalidRange {
                motor,
                range_min,
                center,
                range_max,
            } => write!(
                formatter,
                "motor {motor:?} has invalid range: expected range_min < center < range_max, got {range_min} < {center} < {range_max}"
            ),
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

        for reserved in ["id", "direction"] {
            if spec.extra.contains_key(reserved) {
                return Err(CalibrationError::ReservedExtraField {
                    motor: name.clone(),
                    field: reserved.to_owned(),
                });
            }
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
/// This is rerobot's protocol-neutral centered normalization, not a hardware
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

struct UniqueMap<T>(BTreeMap<String, T>);

impl<'de, T> Deserialize<'de> for UniqueMap<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UniqueMapVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for UniqueMapVisitor<T>
        where
            T: DeserializeOwned,
        {
            type Value = UniqueMap<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object with unique keys")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = access.next_entry::<String, T>()? {
                    if values.insert(key.clone(), value).is_some() {
                        return Err(de::Error::custom(format!("duplicate motor key {key:?}")));
                    }
                }
                Ok(UniqueMap(values))
            }
        }

        deserializer.deserialize_map(UniqueMapVisitor(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: u32) -> MotorSpec {
        MotorSpec {
            id,
            direction: Direction::Normal,
            extra: BTreeMap::new(),
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
    fn motor_spec_defaults_and_retains_protocol_fields() {
        let parsed: MotorSpec =
            serde_json::from_str(r#"{"id":7,"model":"sts3215","baud_rate":1000000}"#).unwrap();
        assert_eq!(parsed.direction, Direction::Normal);
        assert_eq!(parsed.extra["model"], "sts3215");
        assert_eq!(parsed.extra["baud_rate"], 1_000_000);
        assert_eq!(serde_json::to_value(parsed).unwrap()["model"], "sts3215");
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
        let json = serde_json::to_string(&calibration).unwrap();
        assert!(!json.contains("homing_offset"));
        assert!(json.find("elbow").unwrap() < json.find("shoulder").unwrap());
    }

    #[test]
    fn sweep_derives_extrema_from_all_samples() {
        let input: CalibrationInput = serde_json::from_str(
            r#"{
                "center":{"shoulder":2000,"elbow":1900},
                "samples":[
                    {"shoulder":1500,"elbow":2300},
                    {"shoulder":2800,"elbow":900}
                ]
            }"#,
        )
        .unwrap();
        let calibration = build_calibration(&two_specs(), &input).unwrap();
        assert_eq!(calibration.motors["shoulder"].range_min, 1500);
        assert_eq!(calibration.motors["shoulder"].range_max, 2800);
        assert_eq!(calibration.motors["elbow"].range_min, 900);
        assert_eq!(calibration.motors["elbow"].range_max, 2300);
    }

    #[test]
    fn input_is_strict_and_rejects_duplicate_motor_keys() {
        assert!(serde_json::from_str::<CalibrationInput>(r#"{"motors":{},"samplez":[]}"#).is_err());
        assert!(
            serde_json::from_str::<CalibrationInput>(
                r#"{"motors":{"joint":{"center":2,"range_min":1,"range_max":3,"typo":4}}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<CalibrationInput>(
                r#"{"motors":{"joint":{"center":2,"range_min":1,"range_max":3},"joint":{"center":2,"range_min":1,"range_max":3}}}"#
            )
            .is_err()
        );
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

        let mut specs = two_specs();
        specs
            .get_mut("elbow")
            .unwrap()
            .extra
            .insert("direction".to_owned(), Value::String("normal".to_owned()));
        assert!(matches!(
            validate_motor_specs(&specs),
            Err(CalibrationError::ReservedExtraField { .. })
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
    fn calibration_deserialization_validates_metadata_and_contents() {
        let valid = r#"{
            "format_version":1,
            "units":"raw",
            "coordinate_space":"unhomed_raw",
            "motors":{"joint":{"id":1,"direction":"normal","range_min":1,"center":2,"range_max":3}}
        }"#;
        assert!(serde_json::from_str::<Calibration>(valid).is_ok());

        for invalid in [
            valid.replace("\"format_version\":1", "\"format_version\":2"),
            valid.replace("\"units\":\"raw\"", "\"units\":\"degrees\""),
            valid.replace("\"unhomed_raw\"", "\"homed_raw\""),
            valid.replace(
                "\"motors\":{\"joint\":{\"id\":1,\"direction\":\"normal\",\"range_min\":1,\"center\":2,\"range_max\":3}}",
                "\"motors\":{}",
            ),
            valid.replace("\"center\":2", "\"center\":1"),
        ] {
            assert!(serde_json::from_str::<Calibration>(&invalid).is_err());
        }
    }

    #[test]
    fn calibration_deserialization_rejects_duplicate_ids_and_keys() {
        let duplicate_ids = r#"{
            "format_version":1,"units":"raw","coordinate_space":"unhomed_raw",
            "motors":{
                "one":{"id":1,"direction":"normal","range_min":1,"center":2,"range_max":3},
                "two":{"id":1,"direction":"normal","range_min":4,"center":5,"range_max":6}
            }
        }"#;
        assert!(serde_json::from_str::<Calibration>(duplicate_ids).is_err());

        let duplicate_keys = r#"{
            "format_version":1,"units":"raw","coordinate_space":"unhomed_raw",
            "motors":{
                "one":{"id":1,"direction":"normal","range_min":1,"center":2,"range_max":3},
                "one":{"id":2,"direction":"normal","range_min":4,"center":5,"range_max":6}
            }
        }"#;
        assert!(serde_json::from_str::<Calibration>(duplicate_keys).is_err());
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
