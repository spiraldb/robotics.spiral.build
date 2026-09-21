//! SO-ARM101 values used to present telemetry in the viewport.
//!
//! Hardware owns bus IDs, encoder units, calibration, and motion. This module
//! owns only display order and labels for the stock arm's readouts.

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
