//! Minimal Feetech protocol-0 support used by live motor calibration.
//!
//! The implementation intentionally covers only the operations calibration
//! needs: reading registers and broadcasting torque disable. Keeping this
//! adapter narrow avoids coupling the protocol-neutral calibration format to a
//! particular motor SDK.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::io::{self, Read, Write};
use std::thread;
use std::time::{Duration, Instant};

use serialport::{ClearBuffer, SerialPort};

use crate::calibration::MotorSpec;

const BROADCAST_ID: u8 = 0xfe;
const INSTRUCTION_READ: u8 = 0x02;
const INSTRUCTION_SYNC_WRITE: u8 = 0x83;
const TORQUE_ENABLE_ADDRESS: u8 = 40;
const PRESENT_POSITION_ADDRESS: u8 = 56;
const IO_TIMEOUT: Duration = Duration::from_millis(20);
const RESPONSE_TIMEOUT: Duration = Duration::from_millis(100);
const RETRIES: usize = 3;

const SUPPORTED_MODELS: &[&str] = &["sts3215", "sts3250", "sts_series", "sms_series", "sm8512bl"];

/// A configured Feetech motor on the serial bus.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Motor {
    name: String,
    id: u8,
}

/// Errors reported while opening or communicating with a Feetech motor bus.
#[derive(Debug)]
pub enum FeetechError {
    InvalidMotorId {
        motor: String,
        id: u32,
    },
    InvalidModel {
        motor: String,
    },
    UnsupportedModel {
        motor: String,
        model: String,
    },
    Open {
        port: String,
        baud_rate: u32,
        source: serialport::Error,
    },
    Communication {
        operation: String,
        source: io::Error,
    },
    Protocol {
        operation: String,
        detail: String,
    },
    TorqueStillEnabled {
        motor: String,
        id: u8,
        value: u8,
    },
}

impl fmt::Display for FeetechError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMotorId { motor, id } => write!(
                formatter,
                "motor {motor:?} has ID {id}; Feetech IDs must be between 0 and 253"
            ),
            Self::InvalidModel { motor } => write!(
                formatter,
                "motor {motor:?} has a non-string 'model'; live calibration expects a Feetech model name"
            ),
            Self::UnsupportedModel { motor, model } => write!(
                formatter,
                "motor {motor:?} uses unsupported model {model:?}; live calibration currently supports {}",
                SUPPORTED_MODELS.join(", ")
            ),
            Self::Open {
                port,
                baud_rate,
                source,
            } => write!(
                formatter,
                "failed to open Feetech bus {port:?} at {baud_rate} baud: {source}"
            ),
            Self::Communication { operation, source } => {
                write!(formatter, "failed to {operation}: {source}")
            }
            Self::Protocol { operation, detail } => {
                write!(formatter, "failed to {operation}: {detail}")
            }
            Self::TorqueStillEnabled { motor, id, value } => write!(
                formatter,
                "motor {motor:?} (id {id}) still reports Torque_Enable={value} after torque disable"
            ),
        }
    }
}

impl Error for FeetechError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open { source, .. } => Some(source),
            Self::Communication { source, .. } => Some(source),
            Self::InvalidMotorId { .. }
            | Self::InvalidModel { .. }
            | Self::UnsupportedModel { .. }
            | Self::Protocol { .. }
            | Self::TorqueStillEnabled { .. } => None,
        }
    }
}

/// A connection to a Feetech protocol-0 serial motor bus.
pub struct FeetechBus {
    port: Box<dyn SerialPort>,
    motors: Vec<Motor>,
}

impl FeetechBus {
    /// Open `path` and validate that all motor definitions are supported.
    pub fn connect(
        path: &str,
        baud_rate: u32,
        specs: &BTreeMap<String, MotorSpec>,
    ) -> Result<Self, FeetechError> {
        let motors = motors_from_specs(specs)?;
        let port = serialport::new(path, baud_rate)
            .timeout(IO_TIMEOUT)
            .open()
            .map_err(|source| FeetechError::Open {
                port: path.to_owned(),
                baud_rate,
                source,
            })?;
        Ok(Self { port, motors })
    }

    /// Read every configured motor once, failing with the motor name and ID.
    pub fn read_positions(&mut self) -> Result<BTreeMap<String, i64>, FeetechError> {
        let motors = self.motors.clone();
        motors
            .into_iter()
            .map(|motor| {
                let bytes = self.read_register(
                    &motor,
                    PRESENT_POSITION_ADDRESS,
                    2,
                    "read present position",
                )?;
                let encoded = u16::from_le_bytes([bytes[0], bytes[1]]);
                Ok((motor.name, i64::from(decode_sign_magnitude(encoded))))
            })
            .collect()
    }

    /// Disable torque for every configured motor and verify the result.
    pub fn disable_torque(&mut self) -> Result<(), FeetechError> {
        let values = self
            .motors
            .iter()
            .map(|motor| (motor.id, 0_u8))
            .collect::<Vec<_>>();
        let mut parameters = Vec::with_capacity(2 + values.len() * 2);
        parameters.extend_from_slice(&[TORQUE_ENABLE_ADDRESS, 1]);
        for (id, value) in values {
            parameters.extend_from_slice(&[id, value]);
        }
        let packet = instruction_packet(BROADCAST_ID, INSTRUCTION_SYNC_WRITE, &parameters);
        self.clear_input("clear stale serial input before disabling torque")?;
        self.write_packet(&packet, "send torque-disable packet")?;

        // Give the servos time to apply the broadcast before reading it back.
        thread::sleep(Duration::from_millis(5));
        let motors = self.motors.clone();
        for motor in motors {
            let value = self.read_register(
                &motor,
                TORQUE_ENABLE_ADDRESS,
                1,
                "verify torque is disabled",
            )?[0];
            if value != 0 {
                return Err(FeetechError::TorqueStillEnabled {
                    motor: motor.name,
                    id: motor.id,
                    value,
                });
            }
        }
        Ok(())
    }

    fn read_register(
        &mut self,
        motor: &Motor,
        address: u8,
        length: u8,
        operation: &str,
    ) -> Result<Vec<u8>, FeetechError> {
        let packet = instruction_packet(motor.id, INSTRUCTION_READ, &[address, length]);
        let operation = format!("{operation} for motor {:?} (id {})", motor.name, motor.id);
        let mut last_detail = String::new();

        for _ in 0..RETRIES {
            if let Err(error) = self.clear_input(&operation) {
                last_detail = error.to_string();
                continue;
            }
            if let Err(error) = self.write_packet(&packet, &operation) {
                last_detail = error.to_string();
                continue;
            }
            match self.read_status(motor.id, usize::from(length), &operation) {
                Ok(parameters) => return Ok(parameters),
                Err(error) => last_detail = error.to_string(),
            }
        }

        Err(FeetechError::Protocol {
            operation,
            detail: format!("no valid response after {RETRIES} attempts ({last_detail})"),
        })
    }

    fn clear_input(&mut self, operation: &str) -> Result<(), FeetechError> {
        self.port
            .clear(ClearBuffer::Input)
            .map_err(|source| FeetechError::Protocol {
                operation: operation.to_owned(),
                detail: format!("could not clear serial input: {source}"),
            })
    }

    fn write_packet(&mut self, packet: &[u8], operation: &str) -> Result<(), FeetechError> {
        self.port
            .write_all(packet)
            .and_then(|()| self.port.flush())
            .map_err(|source| FeetechError::Communication {
                operation: operation.to_owned(),
                source,
            })
    }

    fn read_status(
        &mut self,
        expected_id: u8,
        expected_parameters: usize,
        operation: &str,
    ) -> Result<Vec<u8>, FeetechError> {
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        let mut received = Vec::with_capacity(32);
        let mut chunk = [0_u8; 32];
        let mut last_parse_error = None;

        while Instant::now() < deadline {
            match self.port.read(&mut chunk) {
                Ok(0) => continue,
                Ok(count) => received.extend_from_slice(&chunk[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(source) => {
                    return Err(FeetechError::Communication {
                        operation: operation.to_owned(),
                        source,
                    });
                }
            }

            loop {
                match take_status_packet(&mut received, expected_id, expected_parameters) {
                    PacketParse::NeedMore => break,
                    PacketParse::Skip(detail) => last_parse_error = Some(detail),
                    PacketParse::Ready(parameters) => return Ok(parameters),
                }
            }
        }

        Err(FeetechError::Protocol {
            operation: operation.to_owned(),
            detail: last_parse_error.unwrap_or_else(|| {
                format!("timed out waiting for a response from motor ID {expected_id}")
            }),
        })
    }
}

fn motors_from_specs(specs: &BTreeMap<String, MotorSpec>) -> Result<Vec<Motor>, FeetechError> {
    let mut motors = Vec::with_capacity(specs.len());
    for (name, spec) in specs {
        let id = u8::try_from(spec.id)
            .ok()
            .filter(|id| *id < BROADCAST_ID)
            .ok_or_else(|| FeetechError::InvalidMotorId {
                motor: name.clone(),
                id: spec.id,
            })?;
        if let Some(value) = spec.extra.get("model") {
            let model = value.as_str().ok_or_else(|| FeetechError::InvalidModel {
                motor: name.clone(),
            })?;
            if !SUPPORTED_MODELS.contains(&model) {
                return Err(FeetechError::UnsupportedModel {
                    motor: name.clone(),
                    model: model.to_owned(),
                });
            }
        }
        motors.push(Motor {
            name: name.clone(),
            id,
        });
    }
    motors.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(motors)
}

fn instruction_packet(id: u8, instruction: u8, parameters: &[u8]) -> Vec<u8> {
    let length = u8::try_from(parameters.len() + 2)
        .expect("Feetech instruction parameters must fit in one packet");
    let mut packet = Vec::with_capacity(parameters.len() + 6);
    packet.extend_from_slice(&[0xff, 0xff, id, length, instruction]);
    packet.extend_from_slice(parameters);
    packet.push(checksum(&packet[2..]));
    packet
}

fn checksum(bytes: &[u8]) -> u8 {
    !bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte))
}

fn decode_sign_magnitude(value: u16) -> i16 {
    if value & 0x8000 == 0 {
        value as i16
    } else {
        -((value & 0x7fff) as i16)
    }
}

enum PacketParse {
    NeedMore,
    Skip(String),
    Ready(Vec<u8>),
}

fn take_status_packet(
    bytes: &mut Vec<u8>,
    expected_id: u8,
    expected_parameters: usize,
) -> PacketParse {
    let Some(header) = bytes.windows(2).position(|window| window == [0xff, 0xff]) else {
        if bytes.last() == Some(&0xff) {
            bytes.drain(..bytes.len().saturating_sub(1));
        } else {
            bytes.clear();
        }
        return PacketParse::NeedMore;
    };
    if header > 0 {
        bytes.drain(..header);
    }
    if bytes.len() < 4 {
        return PacketParse::NeedMore;
    }

    let length = usize::from(bytes[3]);
    if !(2..=64).contains(&length) || bytes[2] == 0xff {
        bytes.drain(..1);
        return PacketParse::Skip("discarded malformed status-packet header".to_owned());
    }
    let packet_length = length + 4;
    if bytes.len() < packet_length {
        return PacketParse::NeedMore;
    }
    let packet = bytes.drain(..packet_length).collect::<Vec<_>>();

    let expected_checksum = checksum(&packet[2..packet_length - 1]);
    if packet[packet_length - 1] != expected_checksum {
        return PacketParse::Skip("received a status packet with an invalid checksum".to_owned());
    }
    if packet[2] != expected_id {
        return PacketParse::Skip(format!(
            "received a response from unexpected motor ID {} while waiting for {expected_id}",
            packet[2]
        ));
    }
    let parameter_count = length - 2;
    if parameter_count != expected_parameters {
        return PacketParse::Skip(format!(
            "motor ID {expected_id} returned {parameter_count} data bytes; expected {expected_parameters}"
        ));
    }
    if packet[4] != 0 {
        return PacketParse::Skip(format!(
            "motor ID {expected_id} returned status error 0x{:02x}",
            packet[4]
        ));
    }

    PacketParse::Ready(packet[5..packet_length - 1].to_vec())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::calibration::Direction;

    fn spec(id: u32, model: Option<serde_json::Value>) -> MotorSpec {
        let mut extra = BTreeMap::new();
        if let Some(model) = model {
            extra.insert("model".to_owned(), model);
        }
        MotorSpec {
            id,
            direction: Direction::Normal,
            extra,
        }
    }

    #[test]
    fn builds_protocol_zero_read_packet() {
        assert_eq!(
            instruction_packet(1, INSTRUCTION_READ, &[PRESENT_POSITION_ADDRESS, 2]),
            [0xff, 0xff, 0x01, 0x04, 0x02, 0x38, 0x02, 0xbe]
        );
    }

    #[test]
    fn parses_status_packets_after_noise_and_in_chunks() {
        let mut bytes = vec![0x00, 0x12, 0xff];
        assert!(matches!(
            take_status_packet(&mut bytes, 1, 2),
            PacketParse::NeedMore
        ));
        bytes.extend_from_slice(&[0xff, 0x01, 0x04, 0x00, 0x34]);
        assert!(matches!(
            take_status_packet(&mut bytes, 1, 2),
            PacketParse::NeedMore
        ));
        bytes.extend_from_slice(&[0x12, 0xb4]);
        match take_status_packet(&mut bytes, 1, 2) {
            PacketParse::Ready(parameters) => assert_eq!(parameters, [0x34, 0x12]),
            _ => panic!("complete valid packet should be returned"),
        }
    }

    #[test]
    fn rejects_corrupt_and_error_status_packets() {
        let mut corrupt = vec![0xff, 0xff, 1, 4, 0, 0x34, 0x12, 0];
        assert!(matches!(
            take_status_packet(&mut corrupt, 1, 2),
            PacketParse::Skip(detail) if detail.contains("checksum")
        ));

        let mut error = vec![0xff, 0xff, 1, 2, 4, 0xf8];
        assert!(matches!(
            take_status_packet(&mut error, 1, 0),
            PacketParse::Skip(detail) if detail.contains("0x04")
        ));
    }

    #[test]
    fn decodes_feetech_sign_magnitude_positions() {
        assert_eq!(decode_sign_magnitude(0x1234), 0x1234);
        assert_eq!(decode_sign_magnitude(0x800a), -10);
        assert_eq!(decode_sign_magnitude(0xffff), -32767);
    }

    #[test]
    fn validates_motor_ids_and_models_and_sorts_by_id() {
        let motors = motors_from_specs(&BTreeMap::from([
            ("elbow".to_owned(), spec(2, Some(json!("sts3215")))),
            ("shoulder".to_owned(), spec(1, None)),
        ]))
        .unwrap();
        assert_eq!(motors[0].name, "shoulder");
        assert_eq!(motors[1].name, "elbow");

        assert!(matches!(
            motors_from_specs(&BTreeMap::from([("joint".to_owned(), spec(254, None))])),
            Err(FeetechError::InvalidMotorId { .. })
        ));
        assert!(matches!(
            motors_from_specs(&BTreeMap::from([(
                "joint".to_owned(),
                spec(1, Some(json!("dynamixel")))
            )])),
            Err(FeetechError::UnsupportedModel { .. })
        ));
    }
}
