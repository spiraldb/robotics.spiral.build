//! One interface over real and simulated motor buses.
//!
//! The studio's control loop is written against [`MotorBus`] so that every
//! screen — setup, calibration, joint control, teleoperation — can be exercised
//! without an arm on the desk. [`SimulatedBus`] is deterministic: it advances
//! only when it is read, so a test can step it exactly as far as it likes.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::calibration::MotorSpec;
use crate::feetech::{
    FeetechBus, FeetechError, MAX_GOAL_POSITION, MotorFaults, MotorLimits, MotorSample,
};

/// Something went wrong talking to a motor bus.
#[derive(Debug)]
pub enum BusError {
    /// A real Feetech bus reported a protocol or transport failure.
    Feetech(FeetechError),
    /// A command named a motor the bus does not have.
    UnknownMotor { motor: String },
}

impl fmt::Display for BusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Feetech(source) => source.fmt(formatter),
            Self::UnknownMotor { motor } => {
                write!(formatter, "motor {motor:?} is not configured on this bus")
            }
        }
    }
}

impl Error for BusError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Feetech(source) => Some(source),
            Self::UnknownMotor { .. } => None,
        }
    }
}

impl From<FeetechError> for BusError {
    fn from(source: FeetechError) -> Self {
        Self::Feetech(source)
    }
}

/// A bank of position-controlled motors addressed by name.
///
/// Positions are raw encoder counts in the coordinate system the motors report;
/// converting to and from calibrated units is the caller's job, because only
/// the caller knows the calibration.
pub trait MotorBus: Send {
    /// Motor names on this bus, in bus-ID order.
    fn motor_names(&self) -> Vec<String>;

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

impl MotorBus for FeetechBus {
    fn motor_names(&self) -> Vec<String> {
        Self::motor_names(self)
    }

    fn read_positions(&mut self) -> Result<BTreeMap<String, i64>, BusError> {
        Ok(Self::read_positions(self)?)
    }

    fn read_samples(&mut self) -> Result<BTreeMap<String, MotorSample>, BusError> {
        Ok(Self::read_samples(self)?)
    }

    fn read_limits(&mut self) -> BTreeMap<String, MotorLimits> {
        Self::read_limits(self)
    }

    fn write_goal_positions(&mut self, goals: &BTreeMap<String, i64>) -> Result<(), BusError> {
        Ok(Self::write_goal_positions(self, goals)?)
    }

    fn set_torque(&mut self, enabled: bool) -> Result<(), BusError> {
        Ok(Self::set_torque(self, enabled)?)
    }
}

/// Encoder counts a simulated motor travels between two reads while driven.
///
/// At the studio's control rate this is roughly a quarter turn per second,
/// slow enough to watch and fast enough to feel responsive.
const SIMULATED_STEP: i64 = 24;

/// Center of a simulated motor's travel, matching a 12-bit encoder.
const SIMULATED_CENTER: i64 = (MAX_GOAL_POSITION + 1) / 2;

/// How far an idle simulated motor sweeps either side of center.
const SIMULATED_DRIFT_AMPLITUDE: f64 = 800.0;

/// Reads it takes an idle simulated motor to complete one sweep.
const SIMULATED_DRIFT_PERIOD: f64 = 420.0;

/// Supply a simulated bus reports, matching the 2S pack an SO-101 runs on.
const SIMULATED_VOLTS: f64 = 7.4;

/// Reads per second the simulation assumes when it reports a rate.
///
/// The bus advances per read rather than per second — that is what makes it
/// steppable from a test — so it has to be told what one read is worth before
/// it can answer in degrees per second. This matches the studio's control rate
/// and affects nothing but simulated readings.
const SIMULATED_READS_PER_SECOND: f64 = 60.0;

/// Temperature a simulated motor rests at.
const SIMULATED_AMBIENT_CELSIUS: f64 = 28.0;

/// Temperature a simulated motor climbs toward while it is holding torque.
const SIMULATED_DRIVEN_CELSIUS: f64 = 52.0;

/// Fraction of the remaining thermal gap a simulated motor closes per read.
///
/// At the control rate this is a climb of roughly nine degrees in the first
/// minute under load — slow enough to read as a real motor warming up, fast
/// enough that the studio's rise-rate readout has something to say during a
/// session someone is actually watching.
const SIMULATED_THERMAL_RATE: f64 = 0.0001;

/// Current a simulated motor draws at full effort, in amps.
const SIMULATED_STALL_AMPS: f64 = 1.4;

/// Effort a simulated motor spends merely holding its position.
const SIMULATED_HOLDING_LOAD: f64 = 0.08;

/// Position error at which a simulated motor is working at its rated torque.
const SIMULATED_FULL_LOAD_ERROR: f64 = 400.0;

/// Limits a simulated motor claims for itself, in the shape a real one reports.
fn simulated_limits() -> MotorLimits {
    MotorLimits {
        max_temperature_celsius: Some(70.0),
        min_voltage_volts: Some(4.5),
        max_voltage_volts: Some(9.0),
        max_load: Some(1.0),
    }
}

#[derive(Clone, Debug, PartialEq)]
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

/// An in-process stand-in for a Feetech bus.
///
/// Undriven motors sweep slowly through their travel so that an operator can
/// see the UI, the calibration sweep, and teleoperation work end to end without
/// hardware. A motor under torque ignores the sweep and tracks its goal.
#[derive(Clone, Debug, PartialEq)]
pub struct SimulatedBus {
    motors: BTreeMap<String, SimulatedMotor>,
    torque: bool,
    drift: bool,
    reads: u64,
}

impl SimulatedBus {
    /// Create a simulated bus for `specs`, with every motor parked at center.
    ///
    /// When `drift` is set, motors that are not being driven sweep through
    /// their travel on every read.
    pub fn new(specs: &BTreeMap<String, MotorSpec>, drift: bool) -> Self {
        let count = specs.len().max(1) as f64;
        let motors = specs
            .iter()
            .enumerate()
            .map(|(index, (name, spec))| {
                (
                    name.clone(),
                    SimulatedMotor {
                        id: spec.id,
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

impl MotorBus for SimulatedBus {
    fn motor_names(&self) -> Vec<String> {
        let mut names = self.motors.keys().cloned().collect::<Vec<_>>();
        names.sort_by_key(|name| self.motors[name].id);
        names
    }

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
                    MotorSample {
                        position: motor.position,
                        speed_degrees_per_second: Some(
                            spiral_robotics_core::arm::counts_to_degrees(travelled)
                                * SIMULATED_READS_PER_SECOND,
                        ),
                        load: Some(motor.load),
                        voltage_volts: Some(SIMULATED_VOLTS),
                        temperature_celsius: Some(motor.temperature.round()),
                        current_amps: Some(motor.load.abs() * SIMULATED_STALL_AMPS),
                        moving: Some(travelled != 0),
                        faults: MotorFaults::default(),
                    },
                )
            })
            .collect())
    }

    fn read_limits(&mut self) -> BTreeMap<String, MotorLimits> {
        self.motors
            .keys()
            .map(|name| (name.clone(), simulated_limits()))
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
            motor.goal = (*goal).clamp(0, MAX_GOAL_POSITION);
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
    use serde_json::json;

    use super::*;
    use crate::calibration::Direction;

    fn specs() -> BTreeMap<String, MotorSpec> {
        BTreeMap::from([
            ("elbow_flex".to_owned(), spec(3)),
            ("shoulder_pan".to_owned(), spec(1)),
            ("shoulder_lift".to_owned(), spec(2)),
        ])
    }

    fn spec(id: u32) -> MotorSpec {
        MotorSpec {
            id,
            direction: Direction::Normal,
            extra: BTreeMap::from([("model".to_owned(), json!("sts3215"))]),
        }
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
        assert_eq!(positions["elbow_flex"], MAX_GOAL_POSITION);
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

        assert_eq!(cold.load, Some(0.0));
        assert_eq!(cold.current_amps, Some(0.0));
        assert_eq!(cold.voltage_volts, Some(SIMULATED_VOLTS));
        assert_eq!(
            cold.temperature_celsius,
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
            hot.load.unwrap().abs() > SIMULATED_HOLDING_LOAD,
            "a motor under load reports more than its holding effort"
        );
        assert!(hot.current_amps.unwrap() > 0.0);
        assert!(
            hot.temperature_celsius.unwrap() > SIMULATED_AMBIENT_CELSIUS,
            "a working motor warms above ambient"
        );
        assert!(
            hot.temperature_celsius.unwrap() < SIMULATED_DRIVEN_CELSIUS,
            "and does not arrive there instantly"
        );
    }

    #[test]
    fn simulated_limits_are_reported_for_every_motor() {
        let limits = SimulatedBus::new(&specs(), false).read_limits();

        assert_eq!(limits.len(), 3);
        assert_eq!(limits["elbow_flex"].max_temperature_celsius, Some(70.0));
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
        assert!(minimum >= 0 && maximum <= MAX_GOAL_POSITION);
    }
}
