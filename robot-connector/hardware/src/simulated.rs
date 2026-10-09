//! A motor bus the connector simulates instead of reaching over serial.
//!
//! Each motor is kinematic: under torque it travels toward its goal at a
//! bounded speed, and with torque released it either stays put or, on a
//! hand-guided arm, is swept through its travel the way an operator moves a
//! leader. The simulation advances only when it is read, so the control loop's
//! fixed rate is its clock and a test can step it exactly as far as it likes.

use std::collections::BTreeMap;

use spiral_robotics_common::robots::MotorLimits;
use spiral_robotics_common::robots::config::MotorConfig;

use crate::bus::{BusError, MotorBus, MotorElectricalSample, MotorMechanicalSample, MotorSample};

/// Encoder counts a driven simulated motor travels per read.
///
/// At the 60 Hz control rate this is about 125° per second, quick enough to
/// keep up with teleoperation and slow enough to watch.
const SIMULATED_STEP: i64 = 24;

/// The simulated motor's physical 12-bit encoder span.
const SIMULATED_MAX_POSITION: i64 = 4095;

/// Center of an uncalibrated simulated motor's travel, matching a 12-bit encoder.
const SIMULATED_CENTER: i64 = (SIMULATED_MAX_POSITION + 1) / 2;

/// How far a simulated hand sweeps a motor either side of its center.
const HAND_SWEEP_AMPLITUDE: f64 = 800.0;

/// Share of a calibrated motor's travel a simulated hand sweeps through, so
/// the hand never presses a joint into its hard stop.
const HAND_SWEEP_REACH: f64 = 0.8;

/// Reads it takes a simulated hand to complete one sweep.
const HAND_SWEEP_PERIOD: f64 = 420.0;

/// Supply a simulated bus reports, matching the 2S pack an SO-101 runs on.
const SIMULATED_VOLTS: f64 = 7.4;

/// Temperature a simulated motor reports.
const SIMULATED_CELSIUS: f64 = 28.0;

#[derive(Clone, Debug, PartialEq)]
struct SimulatedMotor {
    id: u32,
    position: i64,
    goal: i64,
    sweep: HandSweep,
}

impl SimulatedMotor {
    /// A motor parked at the middle of its travel, calibrated or not.
    fn new(spec: &MotorConfig, phase: f64) -> Self {
        let sweep = HandSweep::of(spec, phase);
        Self {
            id: spec.id(),
            position: sweep.center,
            goal: sweep.center,
            sweep,
        }
    }

    /// Limits a simulated motor claims for itself, in the shape a real one reports.
    fn limits() -> MotorLimits {
        MotorLimits::default()
            .with_max_temperature_celsius(Some(70.0))
            .with_voltage_range(Some(4.5), Some(9.0))
            .with_max_load(Some(1.0))
    }
}

/// Where a simulated hand holds one motor over time.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HandSweep {
    center: i64,
    amplitude: f64,
    /// Fraction of a sweep this motor starts ahead of the first one, so a
    /// guided arm looks like an arm rather than a metronome.
    phase: f64,
}

impl HandSweep {
    /// Sweep a calibrated motor inside its measured range; an uncalibrated
    /// one about the encoder's center, as a calibration sweep needs.
    fn of(spec: &MotorConfig, phase: f64) -> Self {
        let (center, amplitude) = match spec.calibration() {
            Some(range) => {
                let reach =
                    (range.center() - range.range_min()).min(range.range_max() - range.center());
                (
                    range.center(),
                    (reach as f64 * HAND_SWEEP_REACH).min(HAND_SWEEP_AMPLITUDE),
                )
            }
            None => (SIMULATED_CENTER, HAND_SWEEP_AMPLITUDE),
        };
        Self {
            center,
            amplitude,
            phase,
        }
    }

    fn position(self, reads: u64) -> i64 {
        let turns = reads as f64 / HAND_SWEEP_PERIOD + self.phase;
        let sweep = (turns * std::f64::consts::TAU).sin() * self.amplitude;
        (self.center as f64 + sweep).round() as i64
    }
}

/// An in-process stand-in for a Feetech bus.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SimulatedBus {
    motors: BTreeMap<String, SimulatedMotor>,
    torque: bool,
    hand_guided: bool,
    reads: u64,
}

impl SimulatedBus {
    /// Create a simulated bus for `specs`, with every motor parked mid-travel.
    pub(crate) fn new(specs: &BTreeMap<String, MotorConfig>, hand_guided: bool) -> Self {
        let count = specs.len().max(1) as f64;
        let motors = specs
            .iter()
            .enumerate()
            .map(|(index, (name, spec))| {
                (
                    name.clone(),
                    SimulatedMotor::new(spec, index as f64 / count),
                )
            })
            .collect();

        Self {
            motors,
            torque: false,
            hand_guided,
            reads: 0,
        }
    }

    #[cfg(test)]
    fn motor_names(&self) -> Vec<String> {
        let mut names = self.motors.keys().cloned().collect::<Vec<_>>();
        names.sort_by_key(|name| self.motors[name].id);
        names
    }

    /// Advance the simulation by one step and return the new positions.
    fn step(&mut self) -> BTreeMap<String, i64> {
        self.reads = self.reads.wrapping_add(1);
        let reads = self.reads;
        let (torque, hand_guided) = (self.torque, self.hand_guided);

        for motor in self.motors.values_mut() {
            if torque {
                let error = motor.goal - motor.position;
                motor.position += error.clamp(-SIMULATED_STEP, SIMULATED_STEP);
            } else if hand_guided {
                motor.position = motor.sweep.position(reads);
                // A released motor is back-driven, so its goal follows along;
                // enabling torque must not yank it back to an old target.
                motor.goal = motor.position;
            }
        }

        self.motors
            .iter()
            .map(|(name, motor)| (name.clone(), motor.position))
            .collect()
    }
}

impl MotorBus for SimulatedBus {
    fn read_positions(&mut self) -> Result<BTreeMap<String, i64>, BusError> {
        Ok(self.step())
    }

    fn read_samples(&mut self) -> Result<BTreeMap<String, MotorSample>, BusError> {
        let electrical =
            MotorElectricalSample::new(Some(SIMULATED_VOLTS), Some(SIMULATED_CELSIUS), None);
        Ok(self
            .step()
            .into_iter()
            .map(|(name, position)| {
                let sample =
                    MotorSample::new(position, MotorMechanicalSample::default(), electrical);
                (name, sample)
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
        if let Some(motor) = goals.keys().find(|name| !self.motors.contains_key(*name)) {
            return Err(BusError::UnknownMotor {
                motor: motor.clone(),
            });
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
    use spiral_robotics_common::robots::config::{MotorDirection, MotorRange};

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
    fn simulated_limits_are_reported_for_every_motor() {
        let limits = SimulatedBus::new(&specs(), false).read_limits();

        assert_eq!(limits.len(), 3);
        assert_eq!(limits["elbow_flex"].max_temperature_celsius(), Some(70.0));
    }

    #[test]
    fn released_motors_move_only_when_a_hand_guides_them() {
        let mut still = SimulatedBus::new(&specs(), false);
        let first = still.read_positions().unwrap();
        for _ in 0..100 {
            still.read_positions().unwrap();
        }
        assert_eq!(still.read_positions().unwrap(), first);

        let mut guided = SimulatedBus::new(&specs(), true);
        let (minimum, maximum) = guided.sweep_extent();
        assert!(
            maximum - minimum > HAND_SWEEP_AMPLITUDE as i64,
            "a full sweep should cover both sides of center"
        );
        assert!(minimum >= 0 && maximum <= SIMULATED_MAX_POSITION);
    }

    #[test]
    fn a_hand_keeps_a_calibrated_motor_inside_its_measured_range() {
        let range = MotorRange::new(1900, 2000, 3000).unwrap();
        let specs = BTreeMap::from([("gripper".to_owned(), spec(6).with_calibration(range))]);
        let mut guided = SimulatedBus::new(&specs, true);
        let (minimum, maximum) = guided.sweep_extent();
        assert_eq!((minimum, maximum), (1920, 2080));
    }

    impl SimulatedBus {
        /// The lowest and highest positions seen across one full hand sweep.
        fn sweep_extent(&mut self) -> (i64, i64) {
            let mut minimum = i64::MAX;
            let mut maximum = i64::MIN;
            for _ in 0..(HAND_SWEEP_PERIOD as usize + 1) {
                for position in self.read_positions().unwrap().into_values() {
                    minimum = minimum.min(position);
                    maximum = maximum.max(position);
                }
            }
            (minimum, maximum)
        }
    }
}
