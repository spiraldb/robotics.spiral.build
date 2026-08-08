//! Minimal Feetech protocol-0 support for calibration and motion control.
//!
//! The implementation covers exactly what the calibration workflow and the
//! studio's control loop need: reading present positions, commanding goal
//! positions, and switching holding torque. Keeping this adapter narrow avoids
//! coupling the protocol-neutral calibration format to a particular motor SDK.

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
const INSTRUCTION_SYNC_READ: u8 = 0x82;
const INSTRUCTION_SYNC_WRITE: u8 = 0x83;
const TORQUE_ENABLE_ADDRESS: u8 = 40;
const GOAL_POSITION_ADDRESS: u8 = 42;
const PRESENT_POSITION_ADDRESS: u8 = 56;
const IO_TIMEOUT: Duration = Duration::from_millis(20);
const RESPONSE_TIMEOUT: Duration = Duration::from_millis(100);
const RETRIES: usize = 3;
/// Consecutive failed sync-read probes before falling back for this bus.
const SYNC_READ_PROBES: u8 = 2;

/// Highest goal position an STS-series servo accepts on its 12-bit encoder.
pub const MAX_GOAL_POSITION: i64 = 4095;

/// Most motors that fit in one protocol-0 sync-write packet.
///
/// The length field is a single byte covering the instruction, the address and
/// size bytes, the per-motor payload, and the checksum. Goal positions are the
/// widest payload at three bytes per motor, so that is what bounds the bus.
const MAX_SYNC_WRITE_MOTORS: usize = (u8::MAX as usize - 4) / 3;

const SUPPORTED_MODELS: &[&str] = &["sts3215", "sts3250", "sts_series", "sms_series", "sm8512bl"];

/// Whether the bus has established that its servos answer sync-read requests.
///
/// Sync read collects every motor in one transaction, which matters at control
/// rates, but not all Feetech firmware implements instruction `0x82`. The bus
/// probes it briefly and remembers the answer instead of paying for a failed
/// transaction on every cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SyncRead {
    Untested(u8),
    Supported,
    Unsupported,
}

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
    UnknownMotor {
        motor: String,
    },
    TooManyMotors {
        count: usize,
        maximum: usize,
    },
    TorqueMismatch {
        motor: String,
        id: u8,
        requested: bool,
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
            Self::UnknownMotor { motor } => {
                write!(formatter, "motor {motor:?} is not configured on this bus")
            }
            Self::TooManyMotors { count, maximum } => write!(
                formatter,
                "{count} motors are configured; one Feetech bus supports at most {maximum}"
            ),
            Self::TorqueMismatch {
                motor,
                id,
                requested,
                value,
            } => write!(
                formatter,
                "motor {motor:?} (id {id}) reports Torque_Enable={value} after torque was turned {}",
                if *requested { "on" } else { "off" }
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
            | Self::UnknownMotor { .. }
            | Self::TooManyMotors { .. }
            | Self::TorqueMismatch { .. } => None,
        }
    }
}

/// A connection to a Feetech protocol-0 serial motor bus.
pub struct FeetechBus {
    port: Box<dyn SerialPort>,
    motors: Vec<Motor>,
    sync_read: SyncRead,
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
        Ok(Self {
            port,
            motors,
            sync_read: SyncRead::Untested(0),
        })
    }

    /// Names of the configured motors, in bus-ID order.
    pub fn motor_names(&self) -> Vec<String> {
        self.motors.iter().map(|motor| motor.name.clone()).collect()
    }

    /// Read every configured motor once, failing with the motor name and ID.
    ///
    /// The first calls probe sync-read support. If the servos do not implement
    /// it, the bus falls back to one read per motor for the rest of its life
    /// rather than retrying an unsupported instruction forever.
    pub fn read_positions(&mut self) -> Result<BTreeMap<String, i64>, FeetechError> {
        if self.sync_read != SyncRead::Unsupported {
            match self.sync_read_positions() {
                Ok(positions) => {
                    self.sync_read = SyncRead::Supported;
                    return Ok(positions);
                }
                Err(error) if self.sync_read == SyncRead::Supported => return Err(error),
                Err(_) => {
                    // A noisy first transaction is not proof that firmware
                    // lacks sync-read support. Giving it one more probe avoids
                    // permanently falling back to six sequential reads — a
                    // common cause of the control loop settling well below
                    // its target rate.
                    let failures = match self.sync_read {
                        SyncRead::Untested(failures) => failures.saturating_add(1),
                        SyncRead::Supported | SyncRead::Unsupported => unreachable!(),
                    };
                    self.sync_read = if failures >= SYNC_READ_PROBES {
                        SyncRead::Unsupported
                    } else {
                        SyncRead::Untested(failures)
                    };
                }
            }
        }

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
                Ok((motor.name, decode_position(&bytes)))
            })
            .collect()
    }

    /// Command a goal position for each named motor in one transaction.
    ///
    /// Positions are clamped to the servo's physical encoder span; the
    /// calibrated per-joint limits are enforced by the caller, which is the
    /// only layer that knows them. Names that are not on this bus are rejected
    /// so a typo cannot silently move nothing.
    pub fn write_goal_positions(
        &mut self,
        goals: &BTreeMap<String, i64>,
    ) -> Result<(), FeetechError> {
        if goals.is_empty() {
            return Ok(());
        }

        let mut parameters = Vec::with_capacity(2 + goals.len() * 3);
        parameters.extend_from_slice(&[GOAL_POSITION_ADDRESS, 2]);
        for (name, goal) in goals {
            let motor = self
                .motors
                .iter()
                .find(|motor| motor.name == *name)
                .ok_or_else(|| FeetechError::UnknownMotor {
                    motor: name.clone(),
                })?;
            let encoded = u16::try_from((*goal).clamp(0, MAX_GOAL_POSITION))
                .expect("a value clamped into 0..=4095 fits in u16");
            parameters.push(motor.id);
            parameters.extend_from_slice(&encoded.to_le_bytes());
        }

        let packet = instruction_packet(BROADCAST_ID, INSTRUCTION_SYNC_WRITE, &parameters);
        self.write_packet(&packet, "send goal-position packet")
    }

    /// Switch holding torque for every configured motor and verify the result.
    ///
    /// Enabling torque first copies each motor's present position into its goal
    /// register. Without that, a servo would snap to whatever goal was left in
    /// its registers by a previous session the instant it is energized.
    pub fn set_torque(&mut self, enabled: bool) -> Result<(), FeetechError> {
        if enabled {
            let positions = self.read_positions()?;
            self.write_goal_positions(&positions)?;
        }

        let value = u8::from(enabled);
        let mut parameters = Vec::with_capacity(2 + self.motors.len() * 2);
        parameters.extend_from_slice(&[TORQUE_ENABLE_ADDRESS, 1]);
        for motor in &self.motors {
            parameters.extend_from_slice(&[motor.id, value]);
        }
        let packet = instruction_packet(BROADCAST_ID, INSTRUCTION_SYNC_WRITE, &parameters);
        let operation = if enabled {
            "clear stale serial input before enabling torque"
        } else {
            "clear stale serial input before disabling torque"
        };
        self.clear_input(operation)?;
        self.write_packet(&packet, "send torque packet")?;

        // Give the servos time to apply the broadcast before reading it back.
        thread::sleep(Duration::from_millis(5));
        let motors = self.motors.clone();
        for motor in motors {
            let reported =
                self.read_register(&motor, TORQUE_ENABLE_ADDRESS, 1, "verify torque state")?[0];
            if (reported != 0) != enabled {
                return Err(FeetechError::TorqueMismatch {
                    motor: motor.name,
                    id: motor.id,
                    requested: enabled,
                    value: reported,
                });
            }
        }
        Ok(())
    }

    /// Disable torque for every configured motor and verify the result.
    pub fn disable_torque(&mut self) -> Result<(), FeetechError> {
        self.set_torque(false)
    }

    /// Read every motor's present position in one broadcast transaction.
    fn sync_read_positions(&mut self) -> Result<BTreeMap<String, i64>, FeetechError> {
        let motors = self.motors.clone();
        let mut parameters = Vec::with_capacity(2 + motors.len());
        parameters.extend_from_slice(&[PRESENT_POSITION_ADDRESS, 2]);
        parameters.extend(motors.iter().map(|motor| motor.id));
        let packet = instruction_packet(BROADCAST_ID, INSTRUCTION_SYNC_READ, &parameters);
        let ids = motors.iter().map(|motor| motor.id).collect::<Vec<_>>();

        self.clear_input("clear stale serial input before a sync read")?;
        self.write_packet(&packet, "send sync-read packet")?;
        let responses = self.read_status_group(&ids, 2, "sync read present positions")?;

        motors
            .into_iter()
            .map(|motor| {
                let bytes = responses.get(&motor.id).ok_or_else(|| {
                    // Unreachable while `read_status_group` waits for every ID,
                    // but a missing motor must never be reported as position 0.
                    FeetechError::Protocol {
                        operation: "sync read present positions".to_owned(),
                        detail: format!("motor {:?} (id {}) did not answer", motor.name, motor.id),
                    }
                })?;
                Ok((motor.name, decode_position(bytes)))
            })
            .collect()
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
        let mut responses =
            self.read_status_group(&[expected_id], expected_parameters, operation)?;
        responses
            .remove(&expected_id)
            .ok_or_else(|| FeetechError::Protocol {
                operation: operation.to_owned(),
                detail: format!("motor ID {expected_id} did not answer"),
            })
    }

    /// Collect one status packet from each of `expected_ids`.
    ///
    /// Responses may arrive in any order and interleaved with line noise, so
    /// packets are matched by ID rather than by position, and the call only
    /// succeeds once every requested motor has answered.
    fn read_status_group(
        &mut self,
        expected_ids: &[u8],
        expected_parameters: usize,
        operation: &str,
    ) -> Result<BTreeMap<u8, Vec<u8>>, FeetechError> {
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        let mut received = Vec::with_capacity(32);
        let mut chunk = [0_u8; 64];
        let mut responses = BTreeMap::new();
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
                match take_status_packet(&mut received, expected_ids, expected_parameters) {
                    PacketParse::NeedMore => break,
                    PacketParse::Skip(detail) => last_parse_error = Some(detail),
                    PacketParse::Ready((id, parameters)) => {
                        responses.insert(id, parameters);
                    }
                }
            }

            if responses.len() == expected_ids.len() {
                return Ok(responses);
            }
        }

        let missing = expected_ids
            .iter()
            .filter(|id| !responses.contains_key(id))
            .map(|id| id.to_string())
            .collect::<Vec<_>>();
        Err(FeetechError::Protocol {
            operation: operation.to_owned(),
            detail: last_parse_error.unwrap_or_else(|| {
                format!(
                    "timed out waiting for a response from motor ID {}",
                    missing.join(", ")
                )
            }),
        })
    }
}

fn motors_from_specs(specs: &BTreeMap<String, MotorSpec>) -> Result<Vec<Motor>, FeetechError> {
    if specs.len() > MAX_SYNC_WRITE_MOTORS {
        return Err(FeetechError::TooManyMotors {
            count: specs.len(),
            maximum: MAX_SYNC_WRITE_MOTORS,
        });
    }

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

/// Decode the two little-endian bytes of a Present_Position response.
fn decode_position(bytes: &[u8]) -> i64 {
    let encoded = u16::from_le_bytes([bytes[0], bytes[1]]);
    i64::from(decode_sign_magnitude(encoded))
}

enum PacketParse {
    NeedMore,
    Skip(String),
    Ready((u8, Vec<u8>)),
}

fn take_status_packet(
    bytes: &mut Vec<u8>,
    expected_ids: &[u8],
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
    let id = packet[2];
    if !expected_ids.contains(&id) {
        return PacketParse::Skip(format!(
            "received a response from unexpected motor ID {id} while waiting for {}",
            expected_ids
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let parameter_count = length - 2;
    if parameter_count != expected_parameters {
        return PacketParse::Skip(format!(
            "motor ID {id} returned {parameter_count} data bytes; expected {expected_parameters}"
        ));
    }
    if packet[4] != 0 {
        return PacketParse::Skip(format!(
            "motor ID {id} returned status error 0x{:02x}",
            packet[4]
        ));
    }

    PacketParse::Ready((id, packet[5..packet_length - 1].to_vec()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::calibration::Direction;

    /// Build a well-formed status packet carrying `parameters` from `id`.
    fn status_packet(id: u8, parameters: &[u8]) -> Vec<u8> {
        let length = u8::try_from(parameters.len() + 2).expect("test packets are short");
        let mut packet = vec![0xff, 0xff, id, length, 0x00];
        packet.extend_from_slice(parameters);
        packet.push(checksum(&packet[2..]));
        packet
    }

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
            take_status_packet(&mut bytes, &[1], 2),
            PacketParse::NeedMore
        ));
        bytes.extend_from_slice(&[0xff, 0x01, 0x04, 0x00, 0x34]);
        assert!(matches!(
            take_status_packet(&mut bytes, &[1], 2),
            PacketParse::NeedMore
        ));
        bytes.extend_from_slice(&[0x12, 0xb4]);
        match take_status_packet(&mut bytes, &[1], 2) {
            PacketParse::Ready((id, parameters)) => {
                assert_eq!(id, 1);
                assert_eq!(parameters, [0x34, 0x12]);
            }
            _ => panic!("complete valid packet should be returned"),
        }
    }

    #[test]
    fn rejects_corrupt_and_error_status_packets() {
        let mut corrupt = vec![0xff, 0xff, 1, 4, 0, 0x34, 0x12, 0];
        assert!(matches!(
            take_status_packet(&mut corrupt, &[1], 2),
            PacketParse::Skip(detail) if detail.contains("checksum")
        ));

        let mut error = vec![0xff, 0xff, 1, 2, 4, 0xf8];
        assert!(matches!(
            take_status_packet(&mut error, &[1], 0),
            PacketParse::Skip(detail) if detail.contains("0x04")
        ));
    }

    #[test]
    fn sync_read_responses_are_matched_by_id_in_any_order() {
        // Two servos answer a sync read, the second one first.
        let mut bytes = status_packet(3, &[0x00, 0x08]);
        bytes.extend(status_packet(1, &[0x34, 0x12]));

        let mut collected = BTreeMap::new();
        loop {
            match take_status_packet(&mut bytes, &[1, 3], 2) {
                PacketParse::Ready((id, parameters)) => {
                    collected.insert(id, parameters);
                }
                PacketParse::NeedMore => break,
                PacketParse::Skip(detail) => panic!("unexpected skip: {detail}"),
            }
        }

        assert_eq!(collected[&1], [0x34, 0x12]);
        assert_eq!(collected[&3], [0x00, 0x08]);
        assert_eq!(decode_position(&collected[&1]), 0x1234);
        assert_eq!(decode_position(&collected[&3]), 2048);
    }

    #[test]
    fn a_response_from_an_unrequested_motor_is_skipped() {
        let mut bytes = status_packet(9, &[0x00, 0x00]);
        assert!(matches!(
            take_status_packet(&mut bytes, &[1, 3], 2),
            PacketParse::Skip(detail) if detail.contains("unexpected motor ID 9")
        ));
    }

    #[test]
    fn builds_a_sync_read_packet_for_every_motor() {
        let packet = instruction_packet(
            BROADCAST_ID,
            INSTRUCTION_SYNC_READ,
            &[PRESENT_POSITION_ADDRESS, 2, 1, 2, 3],
        );

        // header, broadcast ID, length, instruction, address, size, three IDs,
        // checksum.
        assert_eq!(&packet[..5], &[0xff, 0xff, 0xfe, 0x07, 0x82]);
        assert_eq!(&packet[5..packet.len() - 1], &[0x38, 0x02, 1, 2, 3]);
        assert_eq!(
            *packet.last().unwrap(),
            checksum(&packet[2..packet.len() - 1])
        );
    }

    #[test]
    fn builds_a_sync_write_packet_for_goal_positions() {
        let parameters = [GOAL_POSITION_ADDRESS, 2, 1, 0x00, 0x08, 2, 0xff, 0x0f];
        let packet = instruction_packet(BROADCAST_ID, INSTRUCTION_SYNC_WRITE, &parameters);

        assert_eq!(&packet[..5], &[0xff, 0xff, 0xfe, 0x0a, 0x83]);
        assert_eq!(&packet[5..packet.len() - 1], &parameters);
    }

    #[test]
    fn decodes_feetech_sign_magnitude_positions() {
        assert_eq!(decode_sign_magnitude(0x1234), 0x1234);
        assert_eq!(decode_sign_magnitude(0x800a), -10);
        assert_eq!(decode_sign_magnitude(0xffff), -32767);
    }

    #[test]
    fn a_bus_too_large_for_one_sync_write_is_refused_up_front() {
        // Building the packet would otherwise overflow its one-byte length.
        let specs = (0..MAX_SYNC_WRITE_MOTORS as u32 + 1)
            .map(|index| (format!("joint_{index}"), spec(index, None)))
            .collect::<BTreeMap<_, _>>();

        assert!(matches!(
            motors_from_specs(&specs),
            Err(FeetechError::TooManyMotors { .. })
        ));

        let largest = (0..MAX_SYNC_WRITE_MOTORS as u32)
            .map(|index| (format!("joint_{index}"), spec(index, None)))
            .collect::<BTreeMap<_, _>>();
        let motors = motors_from_specs(&largest).expect("the largest supported bus is accepted");
        let mut parameters = vec![GOAL_POSITION_ADDRESS, 2];
        for motor in &motors {
            parameters.extend_from_slice(&[motor.id, 0, 0]);
        }
        assert!(u8::try_from(parameters.len() + 2).is_ok());
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
