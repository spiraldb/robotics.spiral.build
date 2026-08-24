//! JSON codec for calibration input supplied to the command-line adapter.
//!
//! Calibration policy accepts plain [`CalibrationInput`] values. Strict JSON
//! shape, unknown-field rejection, and duplicate-object-key detection are
//! delivery concerns kept here at the edge.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use spiral_robotics_core::calibration::{
    Calibration, CalibrationInput, Direction, MotorCalibration, MotorReading,
};

/// Serialize a domain direction at a delivery or persistence edge.
pub fn serialize_direction<S>(direction: &Direction, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(match direction {
        Direction::Normal => "normal",
        Direction::Reversed => "reversed",
    })
}

/// Deserialize a persisted or delivered direction into the domain value.
pub fn deserialize_direction<'de, D>(deserializer: D) -> Result<Direction, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match String::deserialize(deserializer)?.as_str() {
        "normal" => Ok(Direction::Normal),
        "reversed" => Ok(Direction::Reversed),
        value => Err(de::Error::unknown_variant(value, &["normal", "reversed"])),
    }
}

/// Edge-owned wire shape for a per-motor calibration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MotorCalibrationWire {
    pub id: u32,
    #[serde(
        deserialize_with = "deserialize_direction",
        serialize_with = "serialize_direction"
    )]
    pub direction: Direction,
    pub range_min: i64,
    pub center: i64,
    pub range_max: i64,
}

impl From<MotorCalibration> for MotorCalibrationWire {
    fn from(value: MotorCalibration) -> Self {
        Self {
            id: value.id,
            direction: value.direction,
            range_min: value.range_min,
            center: value.center,
            range_max: value.range_max,
        }
    }
}

impl From<MotorCalibrationWire> for MotorCalibration {
    fn from(value: MotorCalibrationWire) -> Self {
        Self {
            id: value.id,
            direction: value.direction,
            range_min: value.range_min,
            center: value.center,
            range_max: value.range_max,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CalibrationWire {
    format_version: u32,
    units: String,
    coordinate_space: String,
    motors: UniqueMap<MotorCalibrationWire>,
}

/// Decode a strict calibration document and apply all domain invariants.
pub fn decode_calibration(value: serde_json::Value) -> serde_json::Result<Calibration> {
    calibration_from_wire(serde_json::from_value::<CalibrationWire>(value)?)
}

/// Decode calibration JSON before generic values can erase duplicate keys.
pub fn decode_calibration_json(json: &str) -> serde_json::Result<Calibration> {
    let wire = serde_json::from_str::<CalibrationWire>(json)?;
    calibration_from_wire(wire)
}

fn calibration_from_wire(wire: CalibrationWire) -> serde_json::Result<Calibration> {
    let calibration = Calibration {
        format_version: wire.format_version,
        units: wire.units,
        coordinate_space: wire.coordinate_space,
        motors: wire
            .motors
            .0
            .into_iter()
            .map(|(name, motor)| (name, motor.into()))
            .collect(),
    };
    calibration.validate().map_err(json_error)?;
    Ok(calibration)
}

/// Encode a validated calibration document for a robot file.
pub fn encode_calibration(calibration: &Calibration) -> serde_json::Result<serde_json::Value> {
    calibration.validate().map_err(json_error)?;
    serde_json::to_value(CalibrationWire {
        format_version: calibration.format_version,
        units: calibration.units.clone(),
        coordinate_space: calibration.coordinate_space.clone(),
        motors: UniqueMap(
            calibration
                .motors
                .iter()
                .map(|(name, motor)| (name.clone(), (*motor).into()))
                .collect(),
        ),
    })
}

fn json_error(error: impl fmt::Display) -> serde_json::Error {
    <serde_json::Error as de::Error>::custom(error.to_string())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MotorReadingWire {
    center: i64,
    range_min: i64,
    range_max: i64,
}

impl From<MotorReadingWire> for MotorReading {
    fn from(reading: MotorReadingWire) -> Self {
        Self {
            center: reading.center,
            range_min: reading.range_min,
            range_max: reading.range_max,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectInput {
    motors: UniqueMap<MotorReadingWire>,
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

/// Decode strict calibration JSON into a framework-neutral domain input.
pub fn decode_input(json: &str) -> serde_json::Result<CalibrationInput> {
    let wire = serde_json::from_str::<CalibrationInputWire>(json)?;
    Ok(match wire {
        CalibrationInputWire::Direct(input) => CalibrationInput::Direct {
            motors: input
                .motors
                .0
                .into_iter()
                .map(|(name, reading)| (name, reading.into()))
                .collect(),
        },
        CalibrationInputWire::Sweep(input) => CalibrationInput::Sweep {
            center: input.center.0,
            samples: input.samples.into_iter().map(|sample| sample.0).collect(),
        },
    })
}

#[derive(Serialize)]
struct UniqueMap<T>(BTreeMap<String, T>);

impl<'de, T> Deserialize<'de> for UniqueMap<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
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
    use spiral_robotics_core::calibration::{
        CALIBRATION_COORDINATE_SPACE, CALIBRATION_FORMAT_VERSION, CALIBRATION_UNITS,
    };

    #[test]
    fn sweep_input_decodes_without_transport_types_in_domain_policy() {
        let input = decode_input(
            r#"{
                "center":{"shoulder":2000,"elbow":1900},
                "samples":[
                    {"shoulder":1500,"elbow":2300},
                    {"shoulder":2800,"elbow":900}
                ]
            }"#,
        )
        .unwrap();

        let CalibrationInput::Sweep { center, samples } = input else {
            panic!("expected sweep input")
        };
        assert_eq!(center["shoulder"], 2000);
        assert_eq!(samples[1]["elbow"], 900);
    }

    #[test]
    fn input_codec_rejects_unknown_fields_and_duplicate_motor_keys() {
        for invalid in [
            r#"{"motors":{},"samplez":[]}"#,
            r#"{"motors":{"joint":{"center":2,"range_min":1,"range_max":3,"typo":4}}}"#,
            r#"{"motors":{"joint":{"center":2,"range_min":1,"range_max":3},"joint":{"center":2,"range_min":1,"range_max":3}}}"#,
        ] {
            assert!(decode_input(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn calibration_codec_round_trips_and_applies_domain_validation() {
        let calibration = Calibration {
            format_version: CALIBRATION_FORMAT_VERSION,
            units: CALIBRATION_UNITS.to_owned(),
            coordinate_space: CALIBRATION_COORDINATE_SPACE.to_owned(),
            motors: BTreeMap::from([(
                "joint".to_owned(),
                MotorCalibration {
                    id: 1,
                    direction: Direction::Reversed,
                    range_min: 10,
                    center: 20,
                    range_max: 30,
                },
            )]),
        };

        let encoded = encode_calibration(&calibration).unwrap();
        assert_eq!(decode_calibration(encoded).unwrap(), calibration);
    }

    #[test]
    fn calibration_codec_rejects_duplicates_unknown_fields_and_invalid_policy() {
        let valid_motor = r#"{"id":1,"direction":"normal","range_min":1,"center":2,"range_max":3}"#;
        for invalid in [
            format!(
                r#"{{"format_version":1,"units":"raw","coordinate_space":"unhomed_raw","motors":{{"joint":{valid_motor},"joint":{valid_motor}}}}}"#
            ),
            r#"{"format_version":1,"units":"raw","coordinate_space":"unhomed_raw","motors":{"joint":{"id":1,"direction":"normal","range_min":1,"center":2,"range_max":3,"typo":true}}}"#.to_owned(),
            r#"{"format_version":2,"units":"raw","coordinate_space":"unhomed_raw","motors":{"joint":{"id":1,"direction":"normal","range_min":1,"center":2,"range_max":3}}}"#.to_owned(),
            r#"{"format_version":1,"units":"raw","coordinate_space":"unhomed_raw","motors":{"joint":{"id":1,"direction":"normal","range_min":1,"center":1,"range_max":3}}}"#.to_owned(),
        ] {
            assert!(decode_calibration_json(&invalid).is_err(), "accepted {invalid}");
        }
    }
}
