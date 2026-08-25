//! The SO-ARM101 joint model.
//!
//! Everything here is mapping-level knowledge: which motors an SO-101 has and
//! how raw encoder counts become neutral joint-angle values. Rendering geometry,
//! robot-description parsing, and pose construction live in the viewport.

use std::collections::BTreeMap;

use crate::calibration::{MotorDirection, MotorSpec};

/// Encoder counts in one full turn of an STS3215 output shaft.
pub const ENCODER_COUNTS_PER_TURN: i64 = 4096;

/// The role a motor plays in the arm's kinematic chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JointRole {
    /// Rotates the whole arm about the vertical base axis.
    Pan,
    /// Raises and lowers the upper arm.
    Lift,
    /// Bends the forearm relative to the upper arm.
    Elbow,
    /// Bends the wrist relative to the forearm.
    WristFlex,
    /// Rolls the tool about the forearm axis.
    WristRoll,
    /// Opens and closes the gripper.
    Gripper,
}

/// One motor of a stock SO-ARM101 arm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JointDefinition {
    /// Motor name as it appears in the robot's JSON file.
    name: &'static str,
    /// Feetech bus ID assigned by the SO-101 build instructions.
    bus_id: u32,
    /// Where the motor sits in the kinematic chain.
    role: JointRole,
    /// Human-readable name for the UI.
    label: &'static str,
}

impl JointDefinition {
    pub const fn name(self) -> &'static str {
        self.name
    }

    pub const fn label(self) -> &'static str {
        self.label
    }
}

/// The Feetech servo model used throughout a stock SO-101 arm.
const SO101_MOTOR_MODEL: &str = "sts3215";

/// The motors of a stock SO-ARM101, in bus-ID order.
pub const SO101_JOINTS: [JointDefinition; 6] = [
    JointDefinition {
        name: "shoulder_pan",
        bus_id: 1,
        role: JointRole::Pan,
        label: "Shoulder pan",
    },
    JointDefinition {
        name: "shoulder_lift",
        bus_id: 2,
        role: JointRole::Lift,
        label: "Shoulder lift",
    },
    JointDefinition {
        name: "elbow_flex",
        bus_id: 3,
        role: JointRole::Elbow,
        label: "Elbow flex",
    },
    JointDefinition {
        name: "wrist_flex",
        bus_id: 4,
        role: JointRole::WristFlex,
        label: "Wrist flex",
    },
    JointDefinition {
        name: "wrist_roll",
        bus_id: 5,
        role: JointRole::WristRoll,
        label: "Wrist roll",
    },
    JointDefinition {
        name: "gripper",
        bus_id: 6,
        role: JointRole::Gripper,
        label: "Gripper",
    },
];

/// Look up a stock SO-101 joint by motor name.
pub fn so101_joint(name: &str) -> Option<&'static JointDefinition> {
    SO101_JOINTS.iter().find(|joint| joint.name == name)
}

/// Build the default motor map for a new SO-101 robot instance.
pub fn so101_motor_specs() -> BTreeMap<String, MotorSpec> {
    SO101_JOINTS
        .iter()
        .map(|joint| {
            (
                joint.name.to_owned(),
                MotorSpec::new(joint.bus_id, MotorDirection::Normal).with_model(SO101_MOTOR_MODEL),
            )
        })
        .collect()
}

/// Convert a signed encoder-count offset to degrees of shaft rotation.
pub fn counts_to_degrees(counts: i64) -> f64 {
    counts as f64 * 360.0 / ENCODER_COUNTS_PER_TURN as f64
}

/// Convert degrees of shaft rotation to a signed encoder-count offset.
///
/// Motion code must not turn an invalid angle into a plausible zero command,
/// so non-finite and out-of-range inputs are rejected explicitly.
#[cfg(test)]
fn degrees_to_counts(degrees: f64) -> Option<i64> {
    let counts = (degrees * ENCODER_COUNTS_PER_TURN as f64 / 360.0).round();
    // `i64::MAX as f64` rounds up to 2^63, so the positive bound must be
    // exclusive. The negative bound -2^63 is exactly representable.
    const I64_EXCLUSIVE_UPPER: f64 = 9_223_372_036_854_775_808.0;
    (counts.is_finite() && counts >= i64::MIN as f64 && counts < I64_EXCLUSIVE_UPPER)
        .then_some(counts as i64)
}

/// Joint angles relative to each joint's calibrated center.
///
/// This permissive display model is not a motion-command or configuration
/// boundary. Missing or unusable telemetry is intentionally drawn at a neutral
/// pose so a stale sensor cannot make the scene renderer fail.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct JointAngles {
    pan_degrees: f64,
    lift_degrees: f64,
    elbow_degrees: f64,
    wrist_flex_degrees: f64,
    wrist_roll_degrees: f64,
    /// Gripper opening from `0.0` (closed) to `1.0` (fully open).
    gripper_opening: f64,
}

impl JointAngles {
    pub const fn pan_degrees(self) -> f64 {
        self.pan_degrees
    }

    pub const fn lift_degrees(self) -> f64 {
        self.lift_degrees
    }

    pub const fn elbow_degrees(self) -> f64 {
        self.elbow_degrees
    }

    pub const fn wrist_flex_degrees(self) -> f64 {
        self.wrist_flex_degrees
    }

    pub const fn wrist_roll_degrees(self) -> f64 {
        self.wrist_roll_degrees
    }

    pub const fn gripper_opening(self) -> f64 {
        self.gripper_opening
    }

    pub fn with_pan_degrees(mut self, degrees: f64) -> Self {
        self.pan_degrees = finite_or_neutral(degrees);
        self
    }

    pub fn with_lift_degrees(mut self, degrees: f64) -> Self {
        self.lift_degrees = finite_or_neutral(degrees);
        self
    }

    pub fn with_elbow_degrees(mut self, degrees: f64) -> Self {
        self.elbow_degrees = finite_or_neutral(degrees);
        self
    }

    pub fn with_wrist_flex_degrees(mut self, degrees: f64) -> Self {
        self.wrist_flex_degrees = finite_or_neutral(degrees);
        self
    }

    pub fn with_wrist_roll_degrees(mut self, degrees: f64) -> Self {
        self.wrist_roll_degrees = finite_or_neutral(degrees);
        self
    }

    /// Set the gripper opening from a normalized `[-1.0, 1.0]` reading.
    ///
    /// The SO-101 jaw closes as its encoder counts rise, so the *low* end of the
    /// calibrated travel is the open jaw and the normalized reading runs
    /// backwards against the opening this struct reports: `-1.0` is fully open
    /// and `+1.0` is fully closed. Getting this backwards is invisible during
    /// teleoperation — leader and follower invert together, so the arms still
    /// mirror correctly — and only shows up as an inspector percentage and a
    /// drawn jaw that contradict the hardware on the bench.
    ///
    /// Non-finite input becomes half-open. This permissive observation mapping is
    /// not authorization to substitute a motor command; command normalization
    /// uses the strict calibration APIs instead.
    pub fn with_gripper_normalized(mut self, normalized: f64) -> Self {
        self.gripper_opening = if normalized.is_finite() {
            (1.0 - normalized) / 2.0
        } else {
            0.5
        }
        .clamp(0.0, 1.0);
        self
    }
}

impl From<&BTreeMap<String, f64>> for JointAngles {
    /// Collect the angles of the SO-101 joints present in a named map.
    ///
    /// Joints that are missing, unknown, or not finite stay at their center, so
    /// a partially connected or partially calibrated arm remains representable.
    fn from(named_degrees: &BTreeMap<String, f64>) -> Self {
        let mut angles = Self::default();
        for (name, degrees) in named_degrees {
            let Some(joint) = so101_joint(name) else {
                continue;
            };
            match joint.role {
                JointRole::Pan => angles.pan_degrees = finite_or_neutral(*degrees),
                JointRole::Lift => angles.lift_degrees = finite_or_neutral(*degrees),
                JointRole::Elbow => angles.elbow_degrees = finite_or_neutral(*degrees),
                JointRole::WristFlex => angles.wrist_flex_degrees = finite_or_neutral(*degrees),
                JointRole::WristRoll => angles.wrist_roll_degrees = finite_or_neutral(*degrees),
                JointRole::Gripper => {}
            }
        }
        angles
    }
}

fn finite_or_neutral(degrees: f64) -> f64 {
    if degrees.is_finite() { degrees } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_joints_are_unique_and_bus_ordered() {
        let mut ids = SO101_JOINTS
            .iter()
            .map(|joint| joint.bus_id)
            .collect::<Vec<_>>();
        let sorted = ids.clone();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SO101_JOINTS.len(), "motor IDs must be unique");
        assert_eq!(ids, sorted, "joints must be listed in bus-ID order");

        let specs = so101_motor_specs();
        assert_eq!(specs.len(), SO101_JOINTS.len());
        assert_eq!(specs["gripper"].id(), 6);
        assert_eq!(specs["gripper"].model(), Some(SO101_MOTOR_MODEL));
    }

    #[test]
    fn count_and_degree_conversions_round_trip() {
        assert_eq!(counts_to_degrees(ENCODER_COUNTS_PER_TURN), 360.0);
        assert_eq!(counts_to_degrees(-1024), -90.0);
        assert_eq!(degrees_to_counts(90.0), Some(1024));
        assert_eq!(degrees_to_counts(-360.0), Some(-ENCODER_COUNTS_PER_TURN));
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
            assert_eq!(degrees_to_counts(invalid), None);
        }

        for counts in [-2048, -7, 0, 3, 2048] {
            assert_eq!(degrees_to_counts(counts_to_degrees(counts)), Some(counts));
        }
    }

    #[test]
    fn named_degrees_ignore_unknown_and_non_finite_joints() {
        let angles = JointAngles::from(&BTreeMap::from([
            ("shoulder_pan".to_owned(), 15.0),
            ("elbow_flex".to_owned(), -20.0),
            ("wrist_flex".to_owned(), f64::NAN),
            ("some_other_motor".to_owned(), 90.0),
        ]));

        assert_eq!(angles.pan_degrees(), 15.0);
        assert_eq!(angles.elbow_degrees(), -20.0);
        assert_eq!(angles.wrist_flex_degrees(), 0.0);
        assert_eq!(angles.lift_degrees(), 0.0);
    }

    #[test]
    fn gripper_opening_maps_normalized_readings_to_a_fraction() {
        // The bottom of the calibrated travel is the open jaw: on the SO-101 the
        // encoder counts up as the gripper closes. Checked against hardware —
        // the wrist camera showed the follower's jaws wide open while it read
        // `raw 1590` against a `1551..3030` travel, normalized `-0.947`.
        assert_eq!(
            JointAngles::default()
                .with_gripper_normalized(-1.0)
                .gripper_opening(),
            1.0
        );
        assert_eq!(
            JointAngles::default()
                .with_gripper_normalized(0.0)
                .gripper_opening(),
            0.5
        );
        assert_eq!(
            JointAngles::default()
                .with_gripper_normalized(1.0)
                .gripper_opening(),
            0.0
        );
        // Readings past the calibrated ends clamp rather than reporting a jaw
        // wider or tighter than the hardware has.
        assert_eq!(
            JointAngles::default()
                .with_gripper_normalized(9.0)
                .gripper_opening(),
            0.0
        );
        assert_eq!(
            JointAngles::default()
                .with_gripper_normalized(-9.0)
                .gripper_opening(),
            1.0
        );
        assert_eq!(
            JointAngles::default()
                .with_gripper_normalized(f64::NAN)
                .gripper_opening(),
            0.5
        );
    }
}
