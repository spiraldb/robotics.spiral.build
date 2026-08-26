//! SO-ARM101 values used to present telemetry in the viewport.
//!
//! Hardware owns bus IDs, encoder units, calibration, and motion. This module
//! owns only display order, labels, and the permissive pose assembled from
//! already-calibrated telemetry.

use std::collections::BTreeMap;

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

/// One displayed joint of a stock SO-ARM101 arm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JointDefinition {
    /// Motor name as it appears in the robot's JSON file.
    name: &'static str,
    /// Where the motor sits in the kinematic chain.
    role: JointRole,
    /// Human-readable name for the UI.
    label: &'static str,
}

impl JointDefinition {
    /// Look up a stock SO-101 joint by motor name.
    pub fn so101(name: &str) -> Option<&'static Self> {
        SO101_JOINTS.iter().find(|joint| joint.name == name)
    }

    pub const fn name(self) -> &'static str {
        self.name
    }

    pub const fn label(self) -> &'static str {
        self.label
    }
}

/// The joints of a stock SO-ARM101 in the Studio's canonical display order.
pub const SO101_JOINTS: [JointDefinition; 6] = [
    JointDefinition {
        name: "shoulder_pan",
        role: JointRole::Pan,
        label: "Shoulder pan",
    },
    JointDefinition {
        name: "shoulder_lift",
        role: JointRole::Lift,
        label: "Shoulder lift",
    },
    JointDefinition {
        name: "elbow_flex",
        role: JointRole::Elbow,
        label: "Elbow flex",
    },
    JointDefinition {
        name: "wrist_flex",
        role: JointRole::WristFlex,
        label: "Wrist flex",
    },
    JointDefinition {
        name: "wrist_roll",
        role: JointRole::WristRoll,
        label: "Wrist roll",
    },
    JointDefinition {
        name: "gripper",
        role: JointRole::Gripper,
        label: "Gripper",
    },
];

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
    fn finite_or_neutral(degrees: f64) -> f64 {
        if degrees.is_finite() { degrees } else { 0.0 }
    }

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
        self.pan_degrees = Self::finite_or_neutral(degrees);
        self
    }

    pub fn with_lift_degrees(mut self, degrees: f64) -> Self {
        self.lift_degrees = Self::finite_or_neutral(degrees);
        self
    }

    pub fn with_elbow_degrees(mut self, degrees: f64) -> Self {
        self.elbow_degrees = Self::finite_or_neutral(degrees);
        self
    }

    pub fn with_wrist_flex_degrees(mut self, degrees: f64) -> Self {
        self.wrist_flex_degrees = Self::finite_or_neutral(degrees);
        self
    }

    pub fn with_wrist_roll_degrees(mut self, degrees: f64) -> Self {
        self.wrist_roll_degrees = Self::finite_or_neutral(degrees);
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
    /// uses the hardware-owned calibration policy instead.
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
            let Some(joint) = JointDefinition::so101(name) else {
                continue;
            };
            match joint.role {
                JointRole::Pan => angles.pan_degrees = Self::finite_or_neutral(*degrees),
                JointRole::Lift => angles.lift_degrees = Self::finite_or_neutral(*degrees),
                JointRole::Elbow => angles.elbow_degrees = Self::finite_or_neutral(*degrees),
                JointRole::WristFlex => {
                    angles.wrist_flex_degrees = Self::finite_or_neutral(*degrees);
                }
                JointRole::WristRoll => {
                    angles.wrist_roll_degrees = Self::finite_or_neutral(*degrees);
                }
                JointRole::Gripper => {}
            }
        }
        angles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_joint_names_are_unique() {
        let mut names = SO101_JOINTS
            .iter()
            .map(|joint| joint.name)
            .collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SO101_JOINTS.len());
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
