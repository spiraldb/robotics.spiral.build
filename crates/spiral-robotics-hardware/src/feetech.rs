//! Minimal Feetech protocol-0 support for calibration and motion control.
//!
//! The implementation covers exactly what the calibration workflow and the
//! studio's control loop need: reading the motors' status block, commanding
//! goal positions, and switching holding torque. Keeping this adapter narrow
//! avoids coupling the protocol-neutral calibration format to a particular
//! motor SDK.
//!
//! Present position is not read on its own. Everything the servo reports about
//! itself — position, speed, load, supply voltage, temperature, current, and
//! its own fault byte — sits in one contiguous block of registers, so the bus
//! reads the whole block in the same number of transactions a bare position
//! read would have cost. That is the reason the studio can afford motor
//! telemetry on every control cycle instead of on a slower schedule of its own.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::io::{self, Read, Write};
use std::thread;
use std::time::{Duration, Instant};

use serialport::{ClearBuffer, SerialPort};

use crate::bus::{
    BusError, MotorBus, MotorElectricalSample, MotorMechanicalSample, MotorVoltageLimits,
};
use crate::bus::{MotorFaults, MotorLimits, MotorSample};
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
/// Consecutive failed status-block probes before reading position alone.
const STATUS_BLOCK_PROBES: u8 = 2;

/// The status registers every protocol-0 STS/SMS servo publishes, in one run.
///
/// The block starts at Present_Position and ends at Present_Current, covering
/// speed, load, supply voltage, temperature, the servo's own fault byte, and
/// its moving flag on the way. Reading all fifteen bytes costs exactly as many
/// transactions as reading the first two, which is what makes per-cycle
/// telemetry affordable.
///
/// Offsets below are relative to [`PRESENT_POSITION_ADDRESS`]. They come from
/// the protocol-0 memory table shared by the models in [`SUPPORTED_MODELS`];
/// [`MotorSample`] keeps every field optional so a servo that answers with
/// something else degrades to a missing reading rather than a wrong one.
const STATUS_BLOCK_LENGTH: u8 = 15;
const OFFSET_SPEED: usize = 2;
const OFFSET_LOAD: usize = 4;
const OFFSET_VOLTAGE: usize = 6;
const OFFSET_TEMPERATURE: usize = 7;
const OFFSET_STATUS: usize = 9;
const OFFSET_CURRENT: usize = 13;

/// The EEPROM limits a servo enforces on itself, read once when the bus opens.
///
/// Thresholds are never written into this program: a motor already knows the
/// temperature and voltage it will fault at, and a number copied out of a
/// datasheet is a number that can disagree with the hardware in front of the
/// operator.
const LIMITS_ADDRESS: u8 = 13;
const LIMITS_LENGTH: u8 = 5;

/// Volts per least-significant bit of the voltage registers.
const VOLTS_PER_COUNT: f64 = 0.1;
/// Amps per least-significant bit of Present_Current.
const AMPS_PER_COUNT: f64 = 0.0065;
/// Full scale of the load and torque-limit registers, in permille.
const LOAD_FULL_SCALE: f64 = 1000.0;

/// Bit carrying the direction of Present_Load.
///
/// The magnitude never exceeds 1000, so the register only needs ten bits for it
/// and puts the sign immediately above them rather than at the top of the word.
const LOAD_SIGN_BIT: u32 = 10;

/// Highest goal position an STS-series servo accepts on its 12-bit encoder.
const MAX_GOAL_POSITION: i64 = 4095;

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

/// Whether the bus has established that its servos publish the status block.
///
/// Same shape as [`SyncRead`], and for the same reason: a servo that answers a
/// two-byte position read but not a fifteen-byte one must not cost a failed
/// transaction on every cycle for the rest of the session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusBlock {
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

struct InstructionPacket(Vec<u8>);

struct PacketChecksum;

struct RegisterValues<'a>(&'a [u8]);

struct StatusPacketBuffer<'a>(&'a mut Vec<u8>);

#[derive(Clone, Copy)]
struct RegisterBlock<'a> {
    address: u8,
    length: u8,
    operation: &'a str,
}

#[derive(Clone, Copy)]
struct StatusExpectation<'a> {
    parameters: usize,
    operation: &'a str,
}

/// Errors reported while opening or communicating with a Feetech motor bus.
#[derive(Debug)]
pub(crate) enum FeetechError {
    InvalidMotorId {
        motor: String,
        id: u32,
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
    /// Enabling may have reached the motors before its verification failed.
    /// `release_error == None` means the compensating torque-off was verified;
    /// otherwise physical state remains unknown and both errors are retained.
    TorqueEnableFailed {
        cause: Box<FeetechError>,
        release_error: Option<Box<FeetechError>>,
    },
}

impl fmt::Display for FeetechError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMotorId { motor, id } => write!(
                formatter,
                "motor {motor:?} has ID {id}; Feetech IDs must be between 0 and 253"
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
            Self::TorqueEnableFailed {
                cause,
                release_error: None,
            } => write!(
                formatter,
                "could not enable torque: {cause}; compensating torque-off was verified"
            ),
            Self::TorqueEnableFailed {
                cause,
                release_error: Some(release_error),
            } => write!(
                formatter,
                "could not enable torque: {cause}; compensating torque-off also failed: {release_error}"
            ),
        }
    }
}

impl Error for FeetechError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open { source, .. } => Some(source),
            Self::Communication { source, .. } => Some(source),
            Self::TorqueEnableFailed { cause, .. } => Some(cause.as_ref()),
            Self::InvalidMotorId { .. }
            | Self::UnsupportedModel { .. }
            | Self::Protocol { .. }
            | Self::UnknownMotor { .. }
            | Self::TooManyMotors { .. }
            | Self::TorqueMismatch { .. } => None,
        }
    }
}

impl FeetechError {
    /// Finish an enable transition without losing either the enable failure or
    /// the outcome of the compensating physical release.
    fn finish_torque_enable(
        enable: Result<(), Self>,
        release: impl FnOnce() -> Result<(), Self>,
    ) -> Result<(), Self> {
        match enable {
            Ok(()) => Ok(()),
            Err(cause) => {
                let release_error = release().err().map(Box::new);
                Err(Self::TorqueEnableFailed {
                    cause: Box::new(cause),
                    release_error,
                })
            }
        }
    }
}

impl Motor {
    fn from_specs(specs: &BTreeMap<String, MotorSpec>) -> Result<Vec<Self>, FeetechError> {
        if specs.len() > MAX_SYNC_WRITE_MOTORS {
            return Err(FeetechError::TooManyMotors {
                count: specs.len(),
                maximum: MAX_SYNC_WRITE_MOTORS,
            });
        }

        let mut motors = Vec::with_capacity(specs.len());
        for (name, spec) in specs {
            let id = u8::try_from(spec.id())
                .ok()
                .filter(|id| *id < BROADCAST_ID)
                .ok_or_else(|| FeetechError::InvalidMotorId {
                    motor: name.clone(),
                    id: spec.id(),
                })?;
            if let Some(model) = spec.model()
                && !SUPPORTED_MODELS.contains(&model)
            {
                return Err(FeetechError::UnsupportedModel {
                    motor: name.clone(),
                    model: model.to_owned(),
                });
            }
            motors.push(Self {
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
}

/// A connection to a Feetech protocol-0 serial motor bus.
pub(crate) struct FeetechBus {
    port: Box<dyn SerialPort>,
    motors: Vec<Motor>,
    sync_read: SyncRead,
    status_block: StatusBlock,
}

impl FeetechBus {
    /// Open `path` and validate that all motor definitions are supported.
    pub(crate) fn connect(
        path: &str,
        baud_rate: u32,
        specs: &BTreeMap<String, MotorSpec>,
    ) -> Result<Self, FeetechError> {
        let motors = Motor::from_specs(specs)?;
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
            status_block: StatusBlock::Untested(0),
        })
    }

    /// Names of the configured motors, in bus-ID order.
    /// Read every configured motor's status block once.
    ///
    /// The first calls probe whether the servos answer the wide request. A
    /// servo that does not is demoted to a bare position read for the rest of
    /// the session — but only once a position read has succeeded in its place,
    /// so an arm that was simply unplugged mid-transaction keeps its telemetry
    /// when it comes back.
    fn read_samples(&mut self) -> Result<BTreeMap<String, MotorSample>, FeetechError> {
        if self.status_block != StatusBlock::Unsupported {
            match self.read_block(RegisterBlock {
                address: PRESENT_POSITION_ADDRESS,
                length: STATUS_BLOCK_LENGTH,
                operation: "read motor status",
            }) {
                Ok(blocks) => {
                    self.status_block = StatusBlock::Supported;
                    return Ok(blocks
                        .into_iter()
                        .map(|(name, bytes)| (name, RegisterValues::new(&bytes).sample()))
                        .collect());
                }
                Err(error) if self.status_block == StatusBlock::Supported => return Err(error),
                Err(_) => {
                    let positions = self.read_positions()?;
                    // The narrow read went through where the wide one did not,
                    // which is the only evidence that separates firmware
                    // without these registers from a bus having a bad moment.
                    let failures = match self.status_block {
                        StatusBlock::Untested(failures) => failures.saturating_add(1),
                        StatusBlock::Supported | StatusBlock::Unsupported => {
                            unreachable!("a supported block returns its error above")
                        }
                    };
                    self.status_block = if failures >= STATUS_BLOCK_PROBES {
                        StatusBlock::Unsupported
                    } else {
                        StatusBlock::Untested(failures)
                    };
                    return Ok(positions
                        .into_iter()
                        .map(|(name, position)| (name, MotorSample::at(position)))
                        .collect());
                }
            }
        }

        Ok(self
            .read_positions()?
            .into_iter()
            .map(|(name, position)| (name, MotorSample::at(position)))
            .collect())
    }

    /// Read every configured motor's present position, and nothing else.
    ///
    /// Torque changes and calibration sweeps want the position and have no use
    /// for the rest of the block, so they pay for two bytes per motor instead
    /// of fifteen.
    fn read_positions(&mut self) -> Result<BTreeMap<String, i64>, FeetechError> {
        Ok(self
            .read_block(RegisterBlock {
                address: PRESENT_POSITION_ADDRESS,
                length: 2,
                operation: "read present position",
            })?
            .into_iter()
            .map(|(name, bytes)| (name, RegisterValues::new(&bytes).position()))
            .collect())
    }

    /// Read the self-imposed limits each servo stores in EEPROM.
    ///
    /// Called once when a robot connects, off the control loop's path. A motor
    /// that will not answer reports no limits rather than failing the
    /// connection: an arm that drives without known thresholds is far more
    /// useful than an arm that refuses to open.
    fn read_limits(&mut self) -> BTreeMap<String, MotorLimits> {
        let motors = self.motors.clone();
        motors
            .into_iter()
            .filter_map(|motor| {
                let bytes = self
                    .read_register(
                        &motor,
                        RegisterBlock {
                            address: LIMITS_ADDRESS,
                            length: LIMITS_LENGTH,
                            operation: "read motor limits",
                        },
                    )
                    .ok()?;
                Some((motor.name, RegisterValues::new(&bytes).limits()))
            })
            .collect()
    }

    /// Read `length` bytes from `address` on every configured motor.
    ///
    /// The first calls probe sync-read support. If the servos do not implement
    /// it, the bus falls back to one read per motor for the rest of its life
    /// rather than retrying an unsupported instruction forever.
    fn read_block(
        &mut self,
        register: RegisterBlock<'_>,
    ) -> Result<BTreeMap<String, Vec<u8>>, FeetechError> {
        if self.sync_read != SyncRead::Unsupported {
            match self.sync_read_block(register) {
                Ok(values) => {
                    self.sync_read = SyncRead::Supported;
                    return Ok(values);
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
                let bytes = self.read_register(&motor, register)?;
                Ok((motor.name, bytes))
            })
            .collect()
    }

    /// Command a goal position for each named motor in one transaction.
    ///
    /// Positions are clamped to the servo's physical encoder span; the
    /// calibrated per-joint limits are enforced by the caller, which is the
    /// only layer that knows them. Names that are not on this bus are rejected
    /// so a typo cannot silently move nothing.
    fn write_goal_positions(&mut self, goals: &BTreeMap<String, i64>) -> Result<(), FeetechError> {
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

        let packet = InstructionPacket::new(BROADCAST_ID, INSTRUCTION_SYNC_WRITE, &parameters);
        self.write_packet(packet.as_ref(), "send goal-position packet")
    }

    /// Switch holding torque for every configured motor and verify the result.
    ///
    /// Enabling torque first copies each motor's present position into its goal
    /// register. Without that, a servo would snap to whatever goal was left in
    /// its registers by a previous session the instant it is energized.
    fn set_torque(&mut self, enabled: bool) -> Result<(), FeetechError> {
        if !enabled {
            return self.apply_torque_and_verify(false);
        }

        let enable = (|| {
            let positions = self.read_positions()?;
            self.write_goal_positions(&positions)?;
            self.apply_torque_and_verify(true)
        })();

        FeetechError::finish_torque_enable(enable, || self.apply_torque_and_verify(false))
    }

    /// Broadcast one torque state and read every motor back. Once the packet
    /// write begins, an error cannot prove the broadcast had no physical effect.
    fn apply_torque_and_verify(&mut self, enabled: bool) -> Result<(), FeetechError> {
        let value = u8::from(enabled);
        let mut parameters = Vec::with_capacity(2 + self.motors.len() * 2);
        parameters.extend_from_slice(&[TORQUE_ENABLE_ADDRESS, 1]);
        for motor in &self.motors {
            parameters.extend_from_slice(&[motor.id, value]);
        }
        let packet = InstructionPacket::new(BROADCAST_ID, INSTRUCTION_SYNC_WRITE, &parameters);
        let operation = if enabled {
            "clear stale serial input before enabling torque"
        } else {
            "clear stale serial input before disabling torque"
        };
        self.clear_input(operation)?;
        self.write_packet(packet.as_ref(), "send torque packet")?;

        // Give the servos time to apply the broadcast before reading it back.
        thread::sleep(Duration::from_millis(5));
        let motors = self.motors.clone();
        for motor in motors {
            let reported = self.read_register(
                &motor,
                RegisterBlock {
                    address: TORQUE_ENABLE_ADDRESS,
                    length: 1,
                    operation: "verify torque state",
                },
            )?[0];
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

    /// Read the same register run from every motor in one broadcast
    /// transaction.
    fn sync_read_block(
        &mut self,
        register: RegisterBlock<'_>,
    ) -> Result<BTreeMap<String, Vec<u8>>, FeetechError> {
        let motors = self.motors.clone();
        let mut parameters = Vec::with_capacity(2 + motors.len());
        parameters.extend_from_slice(&[register.address, register.length]);
        parameters.extend(motors.iter().map(|motor| motor.id));
        let packet = InstructionPacket::new(BROADCAST_ID, INSTRUCTION_SYNC_READ, &parameters);
        let ids = motors.iter().map(|motor| motor.id).collect::<Vec<_>>();
        let operation = format!("sync {}", register.operation);

        self.clear_input("clear stale serial input before a sync read")?;
        self.write_packet(packet.as_ref(), "send sync-read packet")?;
        let mut responses = self.read_status_group(
            &ids,
            StatusExpectation {
                parameters: usize::from(register.length),
                operation: &operation,
            },
        )?;

        motors
            .into_iter()
            .map(|motor| {
                let bytes = responses.remove(&motor.id).ok_or_else(|| {
                    // Unreachable while `read_status_group` waits for every ID,
                    // but a missing motor must never be reported as position 0.
                    FeetechError::Protocol {
                        operation: operation.clone(),
                        detail: format!("motor {:?} (id {}) did not answer", motor.name, motor.id),
                    }
                })?;
                Ok((motor.name, bytes))
            })
            .collect()
    }

    fn read_register(
        &mut self,
        motor: &Motor,
        register: RegisterBlock<'_>,
    ) -> Result<Vec<u8>, FeetechError> {
        let packet = InstructionPacket::new(
            motor.id,
            INSTRUCTION_READ,
            &[register.address, register.length],
        );
        let operation = format!(
            "{} for motor {:?} (id {})",
            register.operation, motor.name, motor.id
        );
        let mut last_detail = String::new();

        for _ in 0..RETRIES {
            if let Err(error) = self.clear_input(&operation) {
                last_detail = error.to_string();
                continue;
            }
            if let Err(error) = self.write_packet(packet.as_ref(), &operation) {
                last_detail = error.to_string();
                continue;
            }
            match self.read_status(
                motor.id,
                StatusExpectation {
                    parameters: usize::from(register.length),
                    operation: &operation,
                },
            ) {
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
        expectation: StatusExpectation<'_>,
    ) -> Result<Vec<u8>, FeetechError> {
        let mut responses = self.read_status_group(&[expected_id], expectation)?;
        responses
            .remove(&expected_id)
            .ok_or_else(|| FeetechError::Protocol {
                operation: expectation.operation.to_owned(),
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
        expectation: StatusExpectation<'_>,
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
                        operation: expectation.operation.to_owned(),
                        source,
                    });
                }
            }

            loop {
                match StatusPacketBuffer::new(&mut received)
                    .take(expected_ids, expectation.parameters)
                {
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
            operation: expectation.operation.to_owned(),
            detail: last_parse_error.unwrap_or_else(|| {
                format!(
                    "timed out waiting for a response from motor ID {}",
                    missing.join(", ")
                )
            }),
        })
    }
}

/// Adapt the concrete serial driver to the policy-facing motor-bus boundary.
///
/// This is the only place where driver errors cross that seam. Samples and
/// limits have already been decoded from Feetech registers into the neutral
/// physical-unit types owned by [`crate::bus`].
impl MotorBus for FeetechBus {
    fn read_positions(&mut self) -> Result<BTreeMap<String, i64>, BusError> {
        FeetechBus::read_positions(self).map_err(BusError::driver)
    }

    fn read_samples(&mut self) -> Result<BTreeMap<String, MotorSample>, BusError> {
        FeetechBus::read_samples(self).map_err(BusError::driver)
    }

    fn read_limits(&mut self) -> BTreeMap<String, MotorLimits> {
        FeetechBus::read_limits(self)
    }

    fn write_goal_positions(&mut self, goals: &BTreeMap<String, i64>) -> Result<(), BusError> {
        FeetechBus::write_goal_positions(self, goals).map_err(BusError::driver)
    }

    fn set_torque(&mut self, enabled: bool) -> Result<(), BusError> {
        FeetechBus::set_torque(self, enabled).map_err(BusError::driver)
    }
}

impl InstructionPacket {
    fn new(id: u8, instruction: u8, parameters: &[u8]) -> Self {
        let length = u8::try_from(parameters.len() + 2)
            .expect("Feetech instruction parameters must fit in one packet");
        let mut packet = Vec::with_capacity(parameters.len() + 6);
        packet.extend_from_slice(&[0xff, 0xff, id, length, instruction]);
        packet.extend_from_slice(parameters);
        packet.push(PacketChecksum::calculate(&packet[2..]));
        Self(packet)
    }
}

impl AsRef<[u8]> for InstructionPacket {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl PacketChecksum {
    fn calculate(bytes: &[u8]) -> u8 {
        !bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte))
    }
}

impl<'a> RegisterValues<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }

    fn sign_magnitude(value: u16) -> i16 {
        Self::sign_magnitude_at(value, 15)
    }

    /// Decode a sign-magnitude word whose direction lives at `sign_bit`.
    ///
    /// Not every Feetech register puts the sign in the top bit. Load is an 11-bit
    /// field — a magnitude of at most 1000 in the low ten bits and its direction in
    /// bit 10 — so reading it as a 16-bit sign-magnitude word turns a motor pushing
    /// gently backwards into one working at a hundred and seven percent of its
    /// rated torque. Released arms sitting on a bench reported exactly that.
    fn sign_magnitude_at(value: u16, sign_bit: u32) -> i16 {
        let sign = 1_u16 << sign_bit;
        let magnitude = (value & (sign - 1)) as i16;
        if value & sign == 0 {
            magnitude
        } else {
            -magnitude
        }
    }

    /// Read a little-endian word at `offset`, if the response reaches that far.
    fn word(&self, offset: usize) -> Option<u16> {
        let pair = self.0.get(offset..offset + 2)?;
        Some(u16::from_le_bytes([pair[0], pair[1]]))
    }

    /// Decode the Present_Position response.
    fn position(&self) -> i64 {
        i64::from(Self::sign_magnitude(
            self.word(0)
                .expect("a position response contains two bytes"),
        ))
    }

    /// Decode one motor's status block into physical units.
    ///
    /// A response that stops short simply reports fewer measurements. Nothing here
    /// substitutes a zero for a byte that never arrived.
    fn sample(&self) -> MotorSample {
        let mechanical = MotorMechanicalSample::new(
            self.word(OFFSET_SPEED).map(|speed| {
                MotorSpec::degrees_from_encoder_counts(i64::from(Self::sign_magnitude(speed)))
            }),
            self.word(OFFSET_LOAD).map(|load| {
                f64::from(Self::sign_magnitude_at(load, LOAD_SIGN_BIT)) / LOAD_FULL_SCALE
            }),
        );
        let electrical = MotorElectricalSample::new(
            // A servo answering at all is a servo with supply voltage, so a zero
            // here is a register this firmware does not fill rather than a reading.
            self.0
                .get(OFFSET_VOLTAGE)
                .copied()
                .filter(|volts| *volts != 0)
                .map(|volts| f64::from(volts) * VOLTS_PER_COUNT),
            self.0
                .get(OFFSET_TEMPERATURE)
                .map(|celsius| f64::from(*celsius)),
            // Direction of current draw says nothing an operator acts on, and the
            // magnitude is what a stall shows up in.
            self.word(OFFSET_CURRENT)
                .map(|current| f64::from(Self::sign_magnitude(current).abs()) * AMPS_PER_COUNT),
        );
        MotorSample::new(self.position(), mechanical, electrical).with_faults(
            MotorFaults::from_raw(self.0.get(OFFSET_STATUS).copied().unwrap_or_default()),
        )
    }

    /// Decode the EEPROM limit run beginning at [`LIMITS_ADDRESS`].
    fn limits(&self) -> MotorLimits {
        // A limit register left at zero is a limit the servo is not enforcing, and
        // a zero-degree ceiling would put every motor permanently in caution.
        let byte = |offset: usize| self.0.get(offset).copied().filter(|value| *value != 0);
        MotorLimits::new(
            byte(0).map(f64::from),
            MotorVoltageLimits::new(
                byte(2).map(|volts| f64::from(volts) * VOLTS_PER_COUNT),
                byte(1).map(|volts| f64::from(volts) * VOLTS_PER_COUNT),
            ),
            self.word(3)
                .filter(|torque| *torque != 0)
                .map(|torque| f64::from(torque) / LOAD_FULL_SCALE),
        )
    }
}

enum PacketParse {
    NeedMore,
    Skip(String),
    Ready((u8, Vec<u8>)),
}

impl<'a> StatusPacketBuffer<'a> {
    fn new(bytes: &'a mut Vec<u8>) -> Self {
        Self(bytes)
    }

    fn take(&mut self, expected_ids: &[u8], expected_parameters: usize) -> PacketParse {
        let bytes = &mut self.0;
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

        let expected_checksum = PacketChecksum::calculate(&packet[2..packet_length - 1]);
        if packet[packet_length - 1] != expected_checksum {
            return PacketParse::Skip(
                "received a status packet with an invalid checksum".to_owned(),
            );
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
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::bus::MotorFault;
    use crate::calibration::MotorDirection;

    /// Build a well-formed status packet carrying `parameters` from `id`.
    fn status_packet(id: u8, parameters: &[u8]) -> Vec<u8> {
        let length = u8::try_from(parameters.len() + 2).expect("test packets are short");
        let mut packet = vec![0xff, 0xff, id, length, 0x00];
        packet.extend_from_slice(parameters);
        packet.push(PacketChecksum::calculate(&packet[2..]));
        packet
    }

    fn spec(id: u32, model: Option<&str>) -> MotorSpec {
        let specification = MotorSpec::new(id, MotorDirection::Normal);
        match model {
            Some(model) => specification.with_model(model),
            None => specification,
        }
    }

    fn torque_mismatch(requested: bool, value: u8) -> FeetechError {
        FeetechError::TorqueMismatch {
            motor: "joint".to_owned(),
            id: 1,
            requested,
            value,
        }
    }

    #[test]
    fn failed_enable_always_attempts_release_and_preserves_both_outcomes() {
        let releases = Cell::new(0);
        let error = FeetechError::finish_torque_enable(Err(torque_mismatch(true, 0)), || {
            releases.set(releases.get() + 1);
            Ok(())
        })
        .expect_err("verification failure must fail the enable");
        assert_eq!(releases.get(), 1, "torque-off compensation is mandatory");
        assert!(error.to_string().contains("torque-off was verified"));

        let error = FeetechError::finish_torque_enable(Err(torque_mismatch(true, 0)), || {
            releases.set(releases.get() + 1);
            Err(torque_mismatch(false, 1))
        })
        .expect_err("failed compensation keeps the transition failed");
        assert_eq!(releases.get(), 2);
        let FeetechError::TorqueEnableFailed {
            cause,
            release_error: Some(release_error),
        } = error
        else {
            panic!("both errors must remain structured")
        };
        assert!(cause.to_string().contains("turned on"));
        assert!(release_error.to_string().contains("turned off"));
    }

    #[test]
    fn builds_protocol_zero_read_packet() {
        assert_eq!(
            InstructionPacket::new(1, INSTRUCTION_READ, &[PRESENT_POSITION_ADDRESS, 2]).as_ref(),
            [0xff, 0xff, 0x01, 0x04, 0x02, 0x38, 0x02, 0xbe]
        );
    }

    #[test]
    fn parses_status_packets_after_noise_and_in_chunks() {
        let mut bytes = vec![0x00, 0x12, 0xff];
        assert!(matches!(
            StatusPacketBuffer::new(&mut bytes).take(&[1], 2),
            PacketParse::NeedMore
        ));
        bytes.extend_from_slice(&[0xff, 0x01, 0x04, 0x00, 0x34]);
        assert!(matches!(
            StatusPacketBuffer::new(&mut bytes).take(&[1], 2),
            PacketParse::NeedMore
        ));
        bytes.extend_from_slice(&[0x12, 0xb4]);
        match StatusPacketBuffer::new(&mut bytes).take(&[1], 2) {
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
            StatusPacketBuffer::new(&mut corrupt).take(&[1], 2),
            PacketParse::Skip(detail) if detail.contains("checksum")
        ));

        let mut error = vec![0xff, 0xff, 1, 2, 4, 0xf8];
        assert!(matches!(
            StatusPacketBuffer::new(&mut error).take(&[1], 0),
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
            match StatusPacketBuffer::new(&mut bytes).take(&[1, 3], 2) {
                PacketParse::Ready((id, parameters)) => {
                    collected.insert(id, parameters);
                }
                PacketParse::NeedMore => break,
                PacketParse::Skip(detail) => panic!("unexpected skip: {detail}"),
            }
        }

        assert_eq!(collected[&1], [0x34, 0x12]);
        assert_eq!(collected[&3], [0x00, 0x08]);
        assert_eq!(RegisterValues::new(&collected[&1]).position(), 0x1234);
        assert_eq!(RegisterValues::new(&collected[&3]).position(), 2048);
    }

    #[test]
    fn a_response_from_an_unrequested_motor_is_skipped() {
        let mut bytes = status_packet(9, &[0x00, 0x00]);
        assert!(matches!(
            StatusPacketBuffer::new(&mut bytes).take(&[1, 3], 2),
            PacketParse::Skip(detail) if detail.contains("unexpected motor ID 9")
        ));
    }

    #[test]
    fn builds_a_sync_read_packet_for_every_motor() {
        let packet = InstructionPacket::new(
            BROADCAST_ID,
            INSTRUCTION_SYNC_READ,
            &[PRESENT_POSITION_ADDRESS, 2, 1, 2, 3],
        );

        let packet = packet.as_ref();
        // header, broadcast ID, length, instruction, address, size, three IDs,
        // checksum.
        assert_eq!(&packet[..5], &[0xff, 0xff, 0xfe, 0x07, 0x82]);
        assert_eq!(&packet[5..packet.len() - 1], &[0x38, 0x02, 1, 2, 3]);
        assert_eq!(
            *packet.last().unwrap(),
            PacketChecksum::calculate(&packet[2..packet.len() - 1])
        );
    }

    #[test]
    fn builds_a_sync_write_packet_for_goal_positions() {
        let parameters = [GOAL_POSITION_ADDRESS, 2, 1, 0x00, 0x08, 2, 0xff, 0x0f];
        let packet = InstructionPacket::new(BROADCAST_ID, INSTRUCTION_SYNC_WRITE, &parameters);
        let packet = packet.as_ref();

        assert_eq!(&packet[..5], &[0xff, 0xff, 0xfe, 0x0a, 0x83]);
        assert_eq!(&packet[5..packet.len() - 1], &parameters);
    }

    #[test]
    fn decodes_feetech_sign_magnitude_positions() {
        assert_eq!(RegisterValues::sign_magnitude(0x1234), 0x1234);
        assert_eq!(RegisterValues::sign_magnitude(0x800a), -10);
        assert_eq!(RegisterValues::sign_magnitude(0xffff), -32767);
    }

    #[test]
    fn a_bus_too_large_for_one_sync_write_is_refused_up_front() {
        // Building the packet would otherwise overflow its one-byte length.
        let specs = (0..MAX_SYNC_WRITE_MOTORS as u32 + 1)
            .map(|index| (format!("joint_{index}"), spec(index, None)))
            .collect::<BTreeMap<_, _>>();

        assert!(matches!(
            Motor::from_specs(&specs),
            Err(FeetechError::TooManyMotors { .. })
        ));

        let largest = (0..MAX_SYNC_WRITE_MOTORS as u32)
            .map(|index| (format!("joint_{index}"), spec(index, None)))
            .collect::<BTreeMap<_, _>>();
        let motors = Motor::from_specs(&largest).expect("the largest supported bus is accepted");
        let mut parameters = vec![GOAL_POSITION_ADDRESS, 2];
        for motor in &motors {
            parameters.extend_from_slice(&[motor.id, 0, 0]);
        }
        assert!(u8::try_from(parameters.len() + 2).is_ok());
    }

    #[test]
    fn validates_motor_ids_and_models_and_sorts_by_id() {
        let motors = Motor::from_specs(&BTreeMap::from([
            ("elbow".to_owned(), spec(2, Some("sts3215"))),
            ("shoulder".to_owned(), spec(1, None)),
        ]))
        .unwrap();
        assert_eq!(motors[0].name, "shoulder");
        assert_eq!(motors[1].name, "elbow");

        assert!(matches!(
            Motor::from_specs(&BTreeMap::from([("joint".to_owned(), spec(254, None))])),
            Err(FeetechError::InvalidMotorId { .. })
        ));
        assert!(matches!(
            Motor::from_specs(&BTreeMap::from([(
                "joint".to_owned(),
                spec(1, Some("dynamixel"))
            )])),
            Err(FeetechError::UnsupportedModel { .. })
        ));
    }

    /// A status block with every register filled, in the order the servo
    /// publishes them: position, speed, load, voltage, temperature, the async
    /// write flag, the fault byte, moving, two reserved bytes, then current.
    fn status_block() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2048_u16.to_le_bytes());
        bytes.extend_from_slice(&(0x8000_u16 | 512).to_le_bytes());
        bytes.extend_from_slice(&250_u16.to_le_bytes());
        // Load's direction bit is 10, so a magnitude of 250 stays positive
        // here; `a_backwards_load_is_small_and_negative` covers the other case.
        bytes.push(74);
        bytes.push(43);
        bytes.push(0);
        bytes.push(0);
        bytes.push(1);
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&120_u16.to_le_bytes());
        assert_eq!(bytes.len(), usize::from(STATUS_BLOCK_LENGTH));
        bytes
    }

    #[test]
    fn a_status_block_decodes_into_physical_units() {
        let reading = RegisterValues::new(&status_block()).sample();

        assert_eq!(reading.position(), 2048);
        assert_eq!(reading.mechanical().speed_degrees_per_second(), Some(-45.0));
        assert_eq!(reading.mechanical().load(), Some(0.25));
        assert_eq!(reading.electrical().voltage_volts(), Some(7.4));
        assert_eq!(reading.electrical().temperature_celsius(), Some(43.0));
        assert!((reading.electrical().current_amps().unwrap() - 0.78).abs() < 1e-9);
        assert!(!reading.faults().has_faults());
    }

    #[test]
    fn a_truncated_block_reports_fewer_readings_rather_than_zeroes() {
        let reading = RegisterValues::new(&status_block()[..OFFSET_TEMPERATURE]).sample();

        assert_eq!(reading.position(), 2048);
        assert_eq!(reading.electrical().temperature_celsius(), None);
        assert_eq!(reading.electrical().current_amps(), None);
        // Voltage is the last field that did arrive, so it is still reported.
        assert_eq!(reading.electrical().voltage_volts(), Some(7.4));
    }

    #[test]
    fn a_backwards_load_is_small_and_negative_rather_than_over_full_torque() {
        let mut bytes = status_block();
        // What released SO-101 motors resting on a bench actually reported:
        // a magnitude of 44 with the direction bit set, at bit 10.
        bytes[OFFSET_LOAD..OFFSET_LOAD + 2].copy_from_slice(&1068_u16.to_le_bytes());

        let load = RegisterValues::new(&bytes)
            .sample()
            .mechanical()
            .load()
            .expect("a load reading");
        assert!(
            (load + 0.044).abs() < 1e-9,
            "reading the sign at bit 15 would call this 106.8% of rated torque, got {load}"
        );
    }

    #[test]
    fn a_zero_voltage_register_is_absent_rather_than_a_flat_battery() {
        let mut bytes = status_block();
        bytes[OFFSET_VOLTAGE] = 0;

        assert_eq!(
            RegisterValues::new(&bytes)
                .sample()
                .electrical()
                .voltage_volts(),
            None
        );
    }

    #[test]
    fn the_servo_reports_its_own_faults_and_keeps_the_bits_it_cannot_name() {
        let mut bytes = status_block();
        bytes[OFFSET_STATUS] = MotorFaults::OVERHEAT | MotorFaults::OVERLOAD | 0x40;
        let faults = RegisterValues::new(&bytes).sample().faults();

        assert!(faults.has_faults());
        assert_eq!(
            faults.known_faults(),
            [MotorFault::Overheat, MotorFault::Overload]
        );
        assert_eq!(faults.unknown_bits(), 0x40);
    }

    #[test]
    fn limits_come_from_the_servos_own_eeprom() {
        let mut bytes = vec![70, 90, 45];
        bytes.extend_from_slice(&1000_u16.to_le_bytes());
        let limits = RegisterValues::new(&bytes).limits();

        assert_eq!(limits.max_temperature_celsius(), Some(70.0));
        assert_eq!(limits.max_voltage_volts(), Some(9.0));
        assert_eq!(limits.min_voltage_volts(), Some(4.5));
        assert_eq!(limits.max_load(), Some(1.0));
    }

    #[test]
    fn an_unset_limit_register_is_not_a_limit_of_zero() {
        let limits = RegisterValues::new(&[0, 0, 0, 0, 0]).limits();

        assert_eq!(limits.max_temperature_celsius(), None);
        assert_eq!(limits.max_voltage_volts(), None);
        assert_eq!(limits.min_voltage_volts(), None);
        assert_eq!(limits.max_load(), None);
    }
}
