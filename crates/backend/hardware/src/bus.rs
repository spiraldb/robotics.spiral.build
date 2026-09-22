//! One interface over real and simulated motor buses.
//!
//! The studio's control loop is written against [`MotorBus`] so that every
//! screen — setup, calibration, joint control, teleoperation — can be exercised
//! without an arm on the desk. The test-only `SimulatedBus` is deterministic: it advances
//! only when it is read, so a test can step it exactly as far as it likes.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

#[cfg(test)]
use crate::calibration::EncoderCounts;
#[cfg(test)]
use spiral_robotics_common::robots::config::MotorConfig;

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

/// Limits a motor controller enforces on itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MotorLimits {
    max_temperature_celsius: Option<f64>,
    voltage: MotorVoltageLimits,
    /// Configured torque ceiling as a fraction of the motor's rated torque.
    max_load: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MotorVoltageLimits {
    minimum_volts: Option<f64>,
    maximum_volts: Option<f64>,
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

impl MotorVoltageLimits {
    pub(crate) fn new(minimum_volts: Option<f64>, maximum_volts: Option<f64>) -> Self {
        Self {
            minimum_volts,
            maximum_volts,
        }
    }
}

impl MotorLimits {
    pub(crate) fn new(
        max_temperature_celsius: Option<f64>,
        voltage: MotorVoltageLimits,
        max_load: Option<f64>,
    ) -> Self {
        Self {
            max_temperature_celsius,
            voltage,
            max_load,
        }
    }

    pub(crate) fn max_temperature_celsius(self) -> Option<f64> {
        self.max_temperature_celsius
    }

    pub(crate) fn min_voltage_volts(self) -> Option<f64> {
        self.voltage.minimum_volts
    }

    pub(crate) fn max_voltage_volts(self) -> Option<f64> {
        self.voltage.maximum_volts
    }

    pub(crate) fn max_load(self) -> Option<f64> {
        self.max_load
    }
}

/// Something went wrong talking to a motor bus.
#[derive(Debug)]
pub(crate) enum BusError {
    /// A concrete driver reported a protocol or transport failure.
    Driver(Box<dyn Error + Send + Sync>),
    /// A command named a motor the bus does not have.
    #[cfg(test)]
    UnknownMotor { motor: String },
}

impl fmt::Display for BusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Driver(source) => source.fmt(formatter),
            #[cfg(test)]
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
            #[cfg(test)]
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

/// Encoder counts a simulated motor travels between two reads while driven.
///
/// At the studio's control rate this is roughly a quarter turn per second,
/// slow enough to watch and fast enough to feel responsive.
#[cfg(test)]
const SIMULATED_STEP: i64 = 24;

/// The simulated motor's physical 12-bit encoder span.
#[cfg(test)]
const SIMULATED_MAX_POSITION: i64 = 4095;

/// Center of a simulated motor's travel, matching a 12-bit encoder.
#[cfg(test)]
const SIMULATED_CENTER: i64 = (SIMULATED_MAX_POSITION + 1) / 2;

/// How far an idle simulated motor sweeps either side of center.
#[cfg(test)]
const SIMULATED_DRIFT_AMPLITUDE: f64 = 800.0;

/// Reads it takes an idle simulated motor to complete one sweep.
#[cfg(test)]
const SIMULATED_DRIFT_PERIOD: f64 = 420.0;

/// Supply a simulated bus reports, matching the 2S pack an SO-101 runs on.
#[cfg(test)]
const SIMULATED_VOLTS: f64 = 7.4;

/// Reads per second the simulation assumes when it reports a rate.
///
/// The bus advances per read rather than per second — that is what makes it
/// steppable from a test — so it has to be told what one read is worth before
/// it can answer in degrees per second. This matches the studio's control rate
/// and affects nothing but simulated readings.
#[cfg(test)]
const SIMULATED_READS_PER_SECOND: f64 = 60.0;

/// Temperature a simulated motor rests at.
#[cfg(test)]
const SIMULATED_AMBIENT_CELSIUS: f64 = 28.0;

/// Temperature a simulated motor climbs toward while it is holding torque.
#[cfg(test)]
const SIMULATED_DRIVEN_CELSIUS: f64 = 52.0;

/// Fraction of the remaining thermal gap a simulated motor closes per read.
///
/// At the control rate this is a climb of roughly nine degrees in the first
/// minute under load — slow enough to read as a real motor warming up, fast
/// enough that the studio's rise-rate readout has something to say during a
/// session someone is actually watching.
#[cfg(test)]
const SIMULATED_THERMAL_RATE: f64 = 0.0001;

/// Current a simulated motor draws at full effort, in amps.
#[cfg(test)]
const SIMULATED_STALL_AMPS: f64 = 1.4;

/// Effort a simulated motor spends merely holding its position.
#[cfg(test)]
const SIMULATED_HOLDING_LOAD: f64 = 0.08;

/// Position error at which a simulated motor is working at its rated torque.
#[cfg(test)]
const SIMULATED_FULL_LOAD_ERROR: f64 = 400.0;

#[derive(Clone, Debug, PartialEq)]
#[cfg(test)]
struct SimulatedMotor {
    id: u32,
    position: i64,
    goal: i64,
    /// Fraction of a sweep this motor starts ahead of the first one, so an
    /// undriven arm looks like an arm rather than a metronome.
    phase: f64,
    /// Position at the previous read, so simulated speed is measured the same
    /// way a servo measures it rather than declared.
    previous_position: i64,
    temperature: f64,
    load: f64,
}

#[cfg(test)]
impl SimulatedMotor {
    /// Limits a simulated motor claims for itself, in the shape a real one reports.
    fn limits() -> MotorLimits {
        MotorLimits::new(
            Some(70.0),
            MotorVoltageLimits::new(Some(4.5), Some(9.0)),
            Some(1.0),
        )
    }
}

/// An in-process stand-in for a Feetech bus.
///
/// Undriven motors sweep slowly through their travel so that an operator can
/// see the UI, the calibration sweep, and teleoperation work end to end without
/// hardware. A motor under torque ignores the sweep and tracks its goal.
#[derive(Clone, Debug, PartialEq)]
#[cfg(test)]
pub(crate) struct SimulatedBus {
    motors: BTreeMap<String, SimulatedMotor>,
    torque: bool,
    drift: bool,
    reads: u64,
}

#[cfg(test)]
impl SimulatedBus {
    /// Create a simulated bus for `specs`, with every motor parked at center.
    ///
    /// When `drift` is set, motors that are not being driven sweep through
    /// their travel on every read.
    pub(crate) fn new(specs: &BTreeMap<String, MotorConfig>, drift: bool) -> Self {
        let count = specs.len().max(1) as f64;
        let motors = specs
            .iter()
            .enumerate()
            .map(|(index, (name, spec))| {
                (
                    name.clone(),
                    SimulatedMotor {
                        id: spec.id(),
                        position: SIMULATED_CENTER,
                        goal: SIMULATED_CENTER,
                        phase: index as f64 / count,
                        previous_position: SIMULATED_CENTER,
                        temperature: SIMULATED_AMBIENT_CELSIUS,
                        load: 0.0,
                    },
                )
            })
            .collect();

        Self {
            motors,
            torque: false,
            drift,
            reads: 0,
        }
    }

    fn motor_names(&self) -> Vec<String> {
        let mut names = self.motors.keys().cloned().collect::<Vec<_>>();
        names.sort_by_key(|name| self.motors[name].id);
        names
    }

    /// Advance the simulation by one step and return the new positions.
    fn step(&mut self) -> BTreeMap<String, i64> {
        self.reads = self.reads.wrapping_add(1);
        let reads = self.reads;
        let (torque, drift) = (self.torque, self.drift);

        for motor in self.motors.values_mut() {
            motor.previous_position = motor.position;
            let error = motor.goal - motor.position;
            if torque {
                motor.position += error.clamp(-SIMULATED_STEP, SIMULATED_STEP);
            } else if drift {
                let turns = reads as f64 / SIMULATED_DRIFT_PERIOD + motor.phase;
                let sweep = (turns * std::f64::consts::TAU).sin() * SIMULATED_DRIFT_AMPLITUDE;
                motor.position = (SIMULATED_CENTER as f64 + sweep).round() as i64;
                // An undriven motor is back-driven, so its goal follows along;
                // enabling torque must not yank it back to an old target.
                motor.goal = motor.position;
            }

            // A released motor spends no effort and cools; a motor holding a
            // target spends at least its holding load and warms toward the
            // temperature that load settles at. Neither number is measured
            // from anything real, but the studio's simulated environment is
            // already announced on the page, and a bench that reports nothing
            // cannot be used to check that the bench reports things.
            motor.load = if torque {
                let reach = error.abs() as f64 / SIMULATED_FULL_LOAD_ERROR;
                let magnitude = (SIMULATED_HOLDING_LOAD + reach).min(0.9);
                if error < 0 { -magnitude } else { magnitude }
            } else {
                0.0
            };
            let settles_at = SIMULATED_AMBIENT_CELSIUS
                + (SIMULATED_DRIVEN_CELSIUS - SIMULATED_AMBIENT_CELSIUS) * motor.load.abs();
            motor.temperature += (settles_at - motor.temperature) * SIMULATED_THERMAL_RATE;
        }

        self.motors
            .iter()
            .map(|(name, motor)| (name.clone(), motor.position))
            .collect()
    }
}

#[cfg(test)]
impl MotorBus for SimulatedBus {
    fn read_positions(&mut self) -> Result<BTreeMap<String, i64>, BusError> {
        Ok(self.step())
    }

    fn read_samples(&mut self) -> Result<BTreeMap<String, MotorSample>, BusError> {
        self.step();
        Ok(self
            .motors
            .iter()
            .map(|(name, motor)| {
                let travelled = motor.position - motor.previous_position;
                (
                    name.clone(),
                    MotorSample::new(
                        motor.position,
                        MotorMechanicalSample::new(
                            Some(EncoderCounts::degrees(travelled) * SIMULATED_READS_PER_SECOND),
                            Some(motor.load),
                        ),
                        MotorElectricalSample::new(
                            Some(SIMULATED_VOLTS),
                            Some(motor.temperature.round()),
                            Some(motor.load.abs() * SIMULATED_STALL_AMPS),
                        ),
                    ),
                )
            })
            .collect())
    }

    fn read_limits(&mut self) -> BTreeMap<String, MotorLimits> {
        self.motors
            .keys()
            .map(|name| (name.clone(), SimulatedMotor::limits()))
            .collect()
    }

    fn write_goal_positions(&mut self, goals: &BTreeMap<String, i64>) -> Result<(), BusError> {
        for name in goals.keys() {
            if !self.motors.contains_key(name) {
                return Err(BusError::UnknownMotor {
                    motor: name.clone(),
                });
            }
        }
        for (name, goal) in goals {
            let motor = self
                .motors
                .get_mut(name)
                .expect("every goal name was just checked");
            motor.goal = (*goal).clamp(0, SIMULATED_MAX_POSITION);
        }
        Ok(())
    }

    fn set_torque(&mut self, enabled: bool) -> Result<(), BusError> {
        if enabled {
            for motor in self.motors.values_mut() {
                motor.goal = motor.position;
            }
        }
        self.torque = enabled;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::robots::config::MotorDirection;

    fn specs() -> BTreeMap<String, MotorConfig> {
        BTreeMap::from([
            ("elbow_flex".to_owned(), spec(3)),
            ("shoulder_pan".to_owned(), spec(1)),
            ("shoulder_lift".to_owned(), spec(2)),
        ])
    }

    fn spec(id: u32) -> MotorConfig {
        MotorConfig::new(id, MotorDirection::Normal).with_model("sts3215")
    }

    #[test]
    fn a_fresh_bus_parks_every_motor_at_center_in_bus_order() {
        let mut bus = SimulatedBus::new(&specs(), false);

        assert_eq!(
            bus.motor_names(),
            ["shoulder_pan", "shoulder_lift", "elbow_flex"]
        );
        let positions = bus.read_positions().unwrap();
        assert_eq!(positions.len(), 3);
        assert!(positions.values().all(|value| *value == SIMULATED_CENTER));
    }

    #[test]
    fn a_driven_motor_ramps_toward_its_goal_instead_of_jumping() {
        let mut bus = SimulatedBus::new(&specs(), false);
        bus.set_torque(true).unwrap();
        bus.write_goal_positions(&BTreeMap::from([("shoulder_pan".to_owned(), 3000)]))
            .unwrap();

        let first = bus.read_positions().unwrap()["shoulder_pan"];
        assert_eq!(first, SIMULATED_CENTER + SIMULATED_STEP);

        for _ in 0..200 {
            bus.read_positions().unwrap();
        }
        assert_eq!(bus.read_positions().unwrap()["shoulder_pan"], 3000);
    }

    #[test]
    fn goals_are_clamped_to_the_encoder_span() {
        let mut bus = SimulatedBus::new(&specs(), false);
        bus.set_torque(true).unwrap();
        bus.write_goal_positions(&BTreeMap::from([
            ("shoulder_pan".to_owned(), -5_000),
            ("elbow_flex".to_owned(), 99_999),
        ]))
        .unwrap();

        for _ in 0..500 {
            bus.read_positions().unwrap();
        }
        let positions = bus.read_positions().unwrap();
        assert_eq!(positions["shoulder_pan"], 0);
        assert_eq!(positions["elbow_flex"], SIMULATED_MAX_POSITION);
    }

    #[test]
    fn an_unknown_motor_is_rejected_without_moving_anything() {
        let mut bus = SimulatedBus::new(&specs(), false);
        bus.set_torque(true).unwrap();

        let error = bus
            .write_goal_positions(&BTreeMap::from([
                ("shoulder_pan".to_owned(), 3000),
                ("nonexistent".to_owned(), 3000),
            ]))
            .expect_err("an unknown motor must fail the whole command");
        assert!(matches!(error, BusError::UnknownMotor { .. }));
        assert_eq!(
            bus.read_positions().unwrap()["shoulder_pan"],
            SIMULATED_CENTER,
            "no goal should have been applied"
        );
    }

    #[test]
    fn enabling_torque_holds_the_present_position() {
        let mut bus = SimulatedBus::new(&specs(), true);
        for _ in 0..50 {
            bus.read_positions().unwrap();
        }
        let before = bus.read_positions().unwrap();

        bus.set_torque(true).unwrap();
        let after = bus.read_positions().unwrap();

        assert_eq!(before, after, "energizing must not move the arm");
    }

    #[test]
    fn a_released_simulated_motor_rests_cool_and_a_driven_one_warms() {
        let mut bus = SimulatedBus::new(&specs(), false);
        let resting = bus.read_samples().unwrap();
        let cold = resting["shoulder_pan"];

        assert_eq!(cold.mechanical().load(), Some(0.0));
        assert_eq!(cold.electrical().current_amps(), Some(0.0));
        assert_eq!(cold.electrical().voltage_volts(), Some(SIMULATED_VOLTS));
        assert_eq!(
            cold.electrical().temperature_celsius(),
            Some(SIMULATED_AMBIENT_CELSIUS.round())
        );

        // Work the joint end to end rather than parking it on one target: a
        // motor that has arrived spends only its holding load, and barely
        // warming is the correct answer for a motor that is barely working.
        bus.set_torque(true).unwrap();
        for cycle in 0..4_000 {
            let goal = if (cycle / 100) % 2 == 0 { 3800 } else { 300 };
            bus.write_goal_positions(&BTreeMap::from([("shoulder_pan".to_owned(), goal)]))
                .unwrap();
            bus.read_samples().unwrap();
        }
        let hot = bus.read_samples().unwrap()["shoulder_pan"];

        // Load carries the direction the motor is pushing, so a joint on its
        // way back down reports a negative one. Effort is its magnitude.
        assert!(
            hot.mechanical().load().unwrap().abs() > SIMULATED_HOLDING_LOAD,
            "a motor under load reports more than its holding effort"
        );
        assert!(hot.electrical().current_amps().unwrap() > 0.0);
        assert!(
            hot.electrical().temperature_celsius().unwrap() > SIMULATED_AMBIENT_CELSIUS,
            "a working motor warms above ambient"
        );
        assert!(
            hot.electrical().temperature_celsius().unwrap() < SIMULATED_DRIVEN_CELSIUS,
            "and does not arrive there instantly"
        );
    }

    #[test]
    fn simulated_limits_are_reported_for_every_motor() {
        let limits = SimulatedBus::new(&specs(), false).read_limits();

        assert_eq!(limits.len(), 3);
        assert_eq!(limits["elbow_flex"].max_temperature_celsius(), Some(70.0));
    }

    #[test]
    fn idle_motors_sweep_only_when_drift_is_enabled() {
        let mut still = SimulatedBus::new(&specs(), false);
        let first = still.read_positions().unwrap();
        for _ in 0..100 {
            still.read_positions().unwrap();
        }
        assert_eq!(still.read_positions().unwrap(), first);

        let mut sweeping = SimulatedBus::new(&specs(), true);
        let mut minimum = i64::MAX;
        let mut maximum = i64::MIN;
        for _ in 0..(SIMULATED_DRIFT_PERIOD as usize + 1) {
            for position in sweeping.read_positions().unwrap().into_values() {
                minimum = minimum.min(position);
                maximum = maximum.max(position);
            }
        }
        assert!(
            maximum - minimum > SIMULATED_DRIFT_AMPLITUDE as i64,
            "a full sweep should cover both sides of center"
        );
        assert!(minimum >= 0 && maximum <= SIMULATED_MAX_POSITION);
    }
}
