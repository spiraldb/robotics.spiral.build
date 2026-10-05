//! One interface over real and simulated motor buses.
//!
//! The control loop is written against [`MotorBus`], so a Feetech serial bus
//! and the simulated one in [`crate::simulated`] are interchangeable beneath it.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use spiral_robotics_common::robots::MotorLimits;

/// A fault reported by a motor controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MotorFault {
    Voltage,
    Overheat,
    Overload,
}

impl fmt::Display for MotorFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Voltage => "supply voltage",
            Self::Overheat => "overheating",
            Self::Overload => "overloaded",
        })
    }
}

/// Raw motor fault flags, decoded as far as the shared bus contract can name.
///
/// The raw byte remains available so an adapter never turns an unfamiliar
/// hardware fault into a healthy reading.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MotorFaults {
    raw: u8,
}

impl MotorFaults {
    pub(crate) const VOLTAGE: u8 = 0x01;
    pub(crate) const OVERHEAT: u8 = 0x04;
    pub(crate) const OVERLOAD: u8 = 0x20;

    /// True when the motor is reporting anything at all.
    #[cfg(test)]
    pub(crate) fn has_faults(self) -> bool {
        self.raw != 0
    }

    /// The faults this program can name, in the order it names them.
    pub(crate) fn known_faults(self) -> Vec<MotorFault> {
        let mut faults = Vec::new();
        if self.raw & Self::OVERHEAT != 0 {
            faults.push(MotorFault::Overheat);
        }
        if self.raw & Self::OVERLOAD != 0 {
            faults.push(MotorFault::Overload);
        }
        if self.raw & Self::VOLTAGE != 0 {
            faults.push(MotorFault::Voltage);
        }
        faults
    }

    /// Bits that are set but not named above.
    pub(crate) fn unknown_bits(self) -> u8 {
        self.raw & !(Self::VOLTAGE | Self::OVERHEAT | Self::OVERLOAD)
    }

    pub(crate) const fn from_raw(raw: u8) -> Self {
        Self { raw }
    }
}

/// Everything one motor reports in a single bus sample.
///
/// Values use physical units. Every field but position is optional so an
/// adapter can faithfully represent hardware that supplies fewer readings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MotorSample {
    position: i64,
    mechanical: MotorMechanicalSample,
    electrical: MotorElectricalSample,
    faults: MotorFaults,
}

/// Motion and load readings captured in one motor transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MotorMechanicalSample {
    /// Shaft speed in degrees per second, signed.
    speed_degrees_per_second: Option<f64>,
    /// Load as a signed fraction of the motor's rated torque.
    load: Option<f64>,
}

/// Electrical and thermal readings captured in the same motor transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MotorElectricalSample {
    voltage_volts: Option<f64>,
    temperature_celsius: Option<f64>,
    current_amps: Option<f64>,
}

impl MotorSample {
    pub(crate) fn new(
        position: i64,
        mechanical: MotorMechanicalSample,
        electrical: MotorElectricalSample,
    ) -> Self {
        Self {
            position,
            mechanical,
            electrical,
            faults: MotorFaults::default(),
        }
    }

    /// A reading carrying only a position, from a bus that answers nothing else.
    pub(crate) fn at(position: i64) -> Self {
        Self::new(
            position,
            MotorMechanicalSample::default(),
            MotorElectricalSample::default(),
        )
    }

    pub(crate) fn with_faults(mut self, faults: MotorFaults) -> Self {
        self.faults = faults;
        self
    }

    pub(crate) fn position(self) -> i64 {
        self.position
    }

    pub(crate) fn mechanical(self) -> MotorMechanicalSample {
        self.mechanical
    }

    pub(crate) fn electrical(self) -> MotorElectricalSample {
        self.electrical
    }

    pub(crate) fn faults(self) -> MotorFaults {
        self.faults
    }
}

impl MotorMechanicalSample {
    pub(crate) fn new(speed_degrees_per_second: Option<f64>, load: Option<f64>) -> Self {
        Self {
            speed_degrees_per_second,
            load,
        }
    }

    pub(crate) fn speed_degrees_per_second(self) -> Option<f64> {
        self.speed_degrees_per_second
    }

    pub(crate) fn load(self) -> Option<f64> {
        self.load
    }
}

impl MotorElectricalSample {
    pub(crate) fn new(
        voltage_volts: Option<f64>,
        temperature_celsius: Option<f64>,
        current_amps: Option<f64>,
    ) -> Self {
        Self {
            voltage_volts,
            temperature_celsius,
            current_amps,
        }
    }

    pub(crate) fn voltage_volts(self) -> Option<f64> {
        self.voltage_volts
    }

    pub(crate) fn temperature_celsius(self) -> Option<f64> {
        self.temperature_celsius
    }

    pub(crate) fn current_amps(self) -> Option<f64> {
        self.current_amps
    }
}

/// Something went wrong talking to a motor bus.
#[derive(Debug)]
pub(crate) enum BusError {
    /// A concrete driver reported a protocol or transport failure.
    Driver(Box<dyn Error + Send + Sync>),
    /// A command named a motor the bus does not have.
    UnknownMotor { motor: String },
}

impl fmt::Display for BusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Driver(source) => source.fmt(formatter),
            Self::UnknownMotor { motor } => {
                write!(formatter, "motor {motor:?} is not configured on this bus")
            }
        }
    }
}

impl Error for BusError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Driver(source) => Some(source.as_ref()),
            Self::UnknownMotor { .. } => None,
        }
    }
}

impl BusError {
    /// Preserve a driver's concrete error as a source without exposing its
    /// type in the policy-facing boundary.
    pub(crate) fn driver(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Driver(Box::new(source))
    }
}

/// A bank of position-controlled motors addressed by name.
///
/// Positions are raw encoder counts in the coordinate system the motors report;
/// converting to and from calibrated units is the caller's job, because only
/// the caller knows the calibration.
pub(crate) trait MotorBus: Send {
    /// Read the present position of every motor.
    fn read_positions(&mut self) -> Result<BTreeMap<String, i64>, BusError>;

    /// Read everything every motor reports about itself.
    ///
    /// This is what the control loop calls: on a real bus the readings arrive
    /// in the same transactions a bare position read would have taken, so
    /// telemetry costs the loop nothing it was not already paying.
    fn read_samples(&mut self) -> Result<BTreeMap<String, MotorSample>, BusError>;

    /// The limits the motors enforce on themselves.
    ///
    /// Called once when a robot connects rather than on the control loop's
    /// path. A motor that will not answer simply reports no limits: an arm that
    /// drives without known thresholds is far more useful than one that refuses
    /// to open.
    fn read_limits(&mut self) -> BTreeMap<String, MotorLimits>;

    /// Command a goal position for each named motor.
    fn write_goal_positions(&mut self, goals: &BTreeMap<String, i64>) -> Result<(), BusError>;

    /// Turn holding torque on or off for every motor.
    ///
    /// Implementations must seed each motor's goal with its present position
    /// before energizing it, so enabling torque never makes the arm jump to a
    /// stale target.
    fn set_torque(&mut self, enabled: bool) -> Result<(), BusError>;
}
