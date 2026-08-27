//! SO-ARM101 values used to present telemetry in the viewport.
//!
//! Hardware owns bus IDs, encoder units, calibration, and motion. This module
//! owns only display order, labels, and the permissive pose assembled from
//! already-calibrated telemetry.

use std::collections::BTreeMap;

/// One displayed joint of the stock SO-ARM101.
///
/// The enum is the presentation crate's stable vocabulary for the arm. It
/// cannot contain a misspelled or unsupported joint, and its declaration order
/// is the canonical order used by the Studio's readouts and timelines.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Joint {
    /// Rotates the whole arm about the vertical base axis.
    ShoulderPan,
    /// Raises and lowers the upper arm.
    ShoulderLift,
    /// Bends the forearm relative to the upper arm.
    ElbowFlex,
    /// Bends the wrist relative to the forearm.
    WristFlex,
    /// Rolls the tool about the forearm axis.
    WristRoll,
    /// Opens and closes the gripper.
    Gripper,
}

impl Joint {
    /// Look up a supported joint by the name used in telemetry and the URDF.
    pub fn named(name: &str) -> Option<Self> {
        SO101_JOINTS.into_iter().find(|joint| joint.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::ShoulderPan => "shoulder_pan",
            Self::ShoulderLift => "shoulder_lift",
            Self::ElbowFlex => "elbow_flex",
            Self::WristFlex => "wrist_flex",
            Self::WristRoll => "wrist_roll",
            Self::Gripper => "gripper",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ShoulderPan => "Shoulder pan",
            Self::ShoulderLift => "Shoulder lift",
            Self::ElbowFlex => "Elbow flex",
            Self::WristFlex => "Wrist flex",
            Self::WristRoll => "Wrist roll",
            Self::Gripper => "Gripper",
        }
    }
}

impl std::fmt::Display for Joint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl<'a> TryFrom<&'a str> for Joint {
    type Error = &'a str;

    fn try_from(name: &'a str) -> Result<Self, Self::Error> {
        Self::named(name).ok_or(name)
    }
}

/// The joints of a stock SO-ARM101 in the Studio's canonical display order.
pub const SO101_JOINTS: [Joint; 6] = [
    Joint::ShoulderPan,
    Joint::ShoulderLift,
    Joint::ElbowFlex,
    Joint::WristFlex,
    Joint::WristRoll,
    Joint::Gripper,
];

/// Joint angles relative to each joint's calibrated center.
///
/// This permissive display model is not a motion-command or configuration
/// boundary. Missing or unusable telemetry is intentionally drawn at a neutral
/// pose so a stale sensor cannot make the scene renderer fail.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArmPose {
    pan_degrees: f64,
    lift_degrees: f64,
    elbow_degrees: f64,
    wrist_flex_degrees: f64,
    wrist_roll_degrees: f64,
    /// Gripper opening from `0.0` (closed) to `1.0` (fully open).
    gripper_opening: f64,
}

impl ArmPose {
    fn finite_or_neutral(degrees: f64) -> f64 {
        if degrees.is_finite() { degrees } else { 0.0 }
    }

    pub(crate) const fn pan_degrees(self) -> f64 {
        self.pan_degrees
    }

    pub(crate) const fn lift_degrees(self) -> f64 {
        self.lift_degrees
    }

    pub(crate) const fn elbow_degrees(self) -> f64 {
        self.elbow_degrees
    }

    pub(crate) const fn wrist_flex_degrees(self) -> f64 {
        self.wrist_flex_degrees
    }

    pub(crate) const fn wrist_roll_degrees(self) -> f64 {
        self.wrist_roll_degrees
    }

    pub const fn gripper_opening(self) -> f64 {
        self.gripper_opening
    }

    #[cfg(test)]
    pub(crate) fn with_pan_degrees(mut self, degrees: f64) -> Self {
        self.pan_degrees = Self::finite_or_neutral(degrees);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_lift_degrees(mut self, degrees: f64) -> Self {
        self.lift_degrees = Self::finite_or_neutral(degrees);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_elbow_degrees(mut self, degrees: f64) -> Self {
        self.elbow_degrees = Self::finite_or_neutral(degrees);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_wrist_flex_degrees(mut self, degrees: f64) -> Self {
        self.wrist_flex_degrees = Self::finite_or_neutral(degrees);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_wrist_roll_degrees(mut self, degrees: f64) -> Self {
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
    pub(crate) fn with_gripper_normalized(mut self, normalized: f64) -> Self {
        self.gripper_opening = if normalized.is_finite() {
            (1.0 - normalized) / 2.0
        } else {
            0.5
        }
        .clamp(0.0, 1.0);
        self
    }

    /// Build a display pose from calibrated telemetry.
    ///
    /// Missing and unknown joints remain neutral. Non-finite observations also
    /// become neutral, while the normalized gripper reading is clamped to its
    /// representable travel. This is intentionally tolerant because telemetry
    /// is observation, not configuration or motion authority.
    pub fn from_degrees(named_degrees: &BTreeMap<String, f64>, gripper_normalized: f64) -> Self {
        Self::from(named_degrees).with_gripper_normalized(gripper_normalized)
    }
    #[cfg(any(test, feature = "test-support"))]
    /// The calibrated-center angle for a rotary joint. The gripper is linear
    /// presentation state and is read through [`Self::gripper_opening`].
    pub const fn degrees(self, joint: Joint) -> Option<f64> {
        match joint {
            Joint::ShoulderPan => Some(self.pan_degrees),
            Joint::ShoulderLift => Some(self.lift_degrees),
            Joint::ElbowFlex => Some(self.elbow_degrees),
            Joint::WristFlex => Some(self.wrist_flex_degrees),
            Joint::WristRoll => Some(self.wrist_roll_degrees),
            Joint::Gripper => None,
        }
    }
}

impl From<&BTreeMap<String, f64>> for ArmPose {
    /// Collect the angles of the SO-101 joints present in a named map.
    ///
    /// Joints that are missing, unknown, or not finite stay at their center, so
    /// a partially connected or partially calibrated arm remains representable.
    fn from(named_degrees: &BTreeMap<String, f64>) -> Self {
        let mut angles = Self::default();
        for (name, degrees) in named_degrees {
            let Some(joint) = Joint::named(name) else {
                continue;
            };
            match joint {
                Joint::ShoulderPan => angles.pan_degrees = Self::finite_or_neutral(*degrees),
                Joint::ShoulderLift => angles.lift_degrees = Self::finite_or_neutral(*degrees),
                Joint::ElbowFlex => angles.elbow_degrees = Self::finite_or_neutral(*degrees),
                Joint::WristFlex => {
                    angles.wrist_flex_degrees = Self::finite_or_neutral(*degrees);
                }
                Joint::WristRoll => {
                    angles.wrist_roll_degrees = Self::finite_or_neutral(*degrees);
                }
                Joint::Gripper => {}
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
            .map(|joint| joint.name())
            .collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SO101_JOINTS.len());
    }

    #[test]
    fn named_degrees_ignore_unknown_and_non_finite_joints() {
        let angles = ArmPose::from(&BTreeMap::from([
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
            ArmPose::default()
                .with_gripper_normalized(-1.0)
                .gripper_opening(),
            1.0
        );
        assert_eq!(
            ArmPose::default()
                .with_gripper_normalized(0.0)
                .gripper_opening(),
            0.5
        );
        assert_eq!(
            ArmPose::default()
                .with_gripper_normalized(1.0)
                .gripper_opening(),
            0.0
        );
        // Readings past the calibrated ends clamp rather than reporting a jaw
        // wider or tighter than the hardware has.
        assert_eq!(
            ArmPose::default()
                .with_gripper_normalized(9.0)
                .gripper_opening(),
            0.0
        );
        assert_eq!(
            ArmPose::default()
                .with_gripper_normalized(-9.0)
                .gripper_opening(),
            1.0
        );
        assert_eq!(
            ArmPose::default()
                .with_gripper_normalized(f64::NAN)
                .gripper_opening(),
            0.5
        );
    }
}
