//! The SO-ARM101 joint model.
//!
//! Everything here is mapping-level knowledge: which motors an SO-101 has, how
//! a raw encoder count becomes an angle, and how a measured angle lands on the
//! joint of a robot description. The module deliberately holds no dynamics and
//! no inverse kinematics; the UI drives joints directly and only needs a
//! faithful picture of where they currently are.
//!
//! Where the links actually are is [`crate::urdf`]'s job — the shapes and the
//! lengths of an SO-101 are stated once, in its description file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::calibration::{Direction, MotorSpec};
use crate::urdf::Urdf;

/// Encoder counts in one full turn of an STS3215 output shaft.
pub const ENCODER_COUNTS_PER_TURN: i64 = 4096;

/// The role a motor plays in the arm's kinematic chain.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JointRole {
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
    pub name: &'static str,
    /// Feetech bus ID assigned by the SO-101 build instructions.
    pub id: u32,
    /// Where the motor sits in the kinematic chain.
    pub role: JointRole,
    /// Human-readable name for the UI.
    pub label: &'static str,
}

/// The Feetech servo model used throughout a stock SO-101 arm.
pub const SO101_MOTOR_MODEL: &str = "sts3215";

/// The motors of a stock SO-ARM101, in bus-ID order.
pub const SO101_JOINTS: [JointDefinition; 6] = [
    JointDefinition {
        name: "shoulder_pan",
        id: 1,
        role: JointRole::Pan,
        label: "Shoulder pan",
    },
    JointDefinition {
        name: "shoulder_lift",
        id: 2,
        role: JointRole::Lift,
        label: "Shoulder lift",
    },
    JointDefinition {
        name: "elbow_flex",
        id: 3,
        role: JointRole::Elbow,
        label: "Elbow flex",
    },
    JointDefinition {
        name: "wrist_flex",
        id: 4,
        role: JointRole::WristFlex,
        label: "Wrist flex",
    },
    JointDefinition {
        name: "wrist_roll",
        id: 5,
        role: JointRole::WristRoll,
        label: "Wrist roll",
    },
    JointDefinition {
        name: "gripper",
        id: 6,
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
                MotorSpec {
                    id: joint.id,
                    direction: Direction::Normal,
                    extra: BTreeMap::from([(
                        "model".to_owned(),
                        serde_json::Value::String(SO101_MOTOR_MODEL.to_owned()),
                    )]),
                },
            )
        })
        .collect()
}

/// Convert a signed encoder-count offset to degrees of shaft rotation.
pub fn counts_to_degrees(counts: i64) -> f64 {
    counts as f64 * 360.0 / ENCODER_COUNTS_PER_TURN as f64
}

/// Convert degrees of shaft rotation to a signed encoder-count offset.
pub fn degrees_to_counts(degrees: f64) -> i64 {
    if degrees.is_finite() {
        (degrees * ENCODER_COUNTS_PER_TURN as f64 / 360.0).round() as i64
    } else {
        0
    }
}

/// The link angles that correspond to every joint sitting at its calibrated
/// center.
///
/// Calibration records the midpoint of each joint's observed travel, which is
/// not a pose the robot knows anything about. Anchoring mid-travel to a natural
/// reaching pose keeps the drawing recognizable instead of showing a stick
/// pointing straight up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RestPose {
    /// Upper-arm tilt from vertical, positive toward the front of the robot.
    pub lift_deg: f64,
    /// Forearm angle relative to the upper arm.
    pub elbow_deg: f64,
    /// Wrist angle relative to the forearm.
    pub wrist_flex_deg: f64,
}

/// The rest pose used when drawing an SO-101.
pub const SO101_REST_POSE: RestPose = RestPose {
    lift_deg: 30.0,
    elbow_deg: 60.0,
    wrist_flex_deg: 30.0,
};

/// Place a description's joints from measured SO-101 angles.
///
/// The description's own zero is the mechanical one — every joint centered,
/// arm straight up — while a calibration's zero is the middle of whatever
/// travel the operator happened to sweep. [`SO101_REST_POSE`] is what bridges
/// the two, and applying it here keeps the description a statement about the
/// hardware rather than about how this studio calibrates it.
///
/// The gripper is the one joint given as a fraction rather than an angle, so
/// it is scaled to whatever opening the description says the jaw has; nothing
/// here needs to know that number.
pub fn so101_joint_positions(urdf: &Urdf, angles: &JointAngles) -> BTreeMap<String, f32> {
    let rest = SO101_REST_POSE;
    let open = urdf
        .joint("gripper")
        .and_then(|joint| joint.limit)
        .map_or(0.0, |(_, upper)| upper);

    [
        ("shoulder_pan", angles.pan_deg.to_radians() as f32),
        (
            "shoulder_lift",
            (rest.lift_deg + angles.lift_deg).to_radians() as f32,
        ),
        (
            "elbow_flex",
            (rest.elbow_deg + angles.elbow_deg).to_radians() as f32,
        ),
        (
            "wrist_flex",
            (rest.wrist_flex_deg + angles.wrist_flex_deg).to_radians() as f32,
        ),
        ("wrist_roll", angles.wrist_roll_deg.to_radians() as f32),
        ("gripper", angles.gripper.clamp(0.0, 1.0) as f32 * open),
    ]
    .into_iter()
    .map(|(name, position)| (name.to_owned(), position))
    .collect()
}

/// Joint angles relative to each joint's calibrated center, in degrees.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct JointAngles {
    pub pan_deg: f64,
    pub lift_deg: f64,
    pub elbow_deg: f64,
    pub wrist_flex_deg: f64,
    pub wrist_roll_deg: f64,
    /// Gripper opening from `0.0` (closed) to `1.0` (fully open).
    pub gripper: f64,
}

impl JointAngles {
    /// Collect the angles of the SO-101 joints present in `degrees`.
    ///
    /// Joints that are missing, unknown, or not finite stay at their center, so
    /// a partially connected or partially calibrated arm still draws.
    pub fn from_named_degrees(degrees: &BTreeMap<String, f64>) -> Self {
        let mut angles = Self::default();
        for (name, value) in degrees {
            let Some(joint) = so101_joint(name) else {
                continue;
            };
            if !value.is_finite() {
                continue;
            }
            match joint.role {
                JointRole::Pan => angles.pan_deg = *value,
                JointRole::Lift => angles.lift_deg = *value,
                JointRole::Elbow => angles.elbow_deg = *value,
                JointRole::WristFlex => angles.wrist_flex_deg = *value,
                JointRole::WristRoll => angles.wrist_roll_deg = *value,
                JointRole::Gripper => {}
            }
        }
        angles
    }

    /// Set the gripper opening from a normalized `[-1.0, 1.0]` reading.
    pub fn with_gripper_normalized(mut self, normalized: f64) -> Self {
        self.gripper = if normalized.is_finite() {
            (normalized + 1.0) / 2.0
        } else {
            0.5
        }
        .clamp(0.0, 1.0);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_joints_are_unique_and_bus_ordered() {
        let mut ids = SO101_JOINTS
            .iter()
            .map(|joint| joint.id)
            .collect::<Vec<_>>();
        let sorted = ids.clone();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SO101_JOINTS.len(), "motor IDs must be unique");
        assert_eq!(ids, sorted, "joints must be listed in bus-ID order");

        let specs = so101_motor_specs();
        assert_eq!(specs.len(), SO101_JOINTS.len());
        assert_eq!(specs["gripper"].id, 6);
        assert_eq!(specs["gripper"].extra["model"], SO101_MOTOR_MODEL);
    }

    #[test]
    fn count_and_degree_conversions_round_trip() {
        assert_eq!(counts_to_degrees(ENCODER_COUNTS_PER_TURN), 360.0);
        assert_eq!(counts_to_degrees(-1024), -90.0);
        assert_eq!(degrees_to_counts(90.0), 1024);
        assert_eq!(degrees_to_counts(-360.0), -ENCODER_COUNTS_PER_TURN);
        assert_eq!(degrees_to_counts(f64::INFINITY), 0);

        for counts in [-2048, -7, 0, 3, 2048] {
            assert_eq!(degrees_to_counts(counts_to_degrees(counts)), counts);
        }
    }

    #[test]
    fn named_degrees_ignore_unknown_and_non_finite_joints() {
        let angles = JointAngles::from_named_degrees(&BTreeMap::from([
            ("shoulder_pan".to_owned(), 15.0),
            ("elbow_flex".to_owned(), -20.0),
            ("wrist_flex".to_owned(), f64::NAN),
            ("some_other_motor".to_owned(), 90.0),
        ]));

        assert_eq!(angles.pan_deg, 15.0);
        assert_eq!(angles.elbow_deg, -20.0);
        assert_eq!(angles.wrist_flex_deg, 0.0);
        assert_eq!(angles.lift_deg, 0.0);
    }

    #[test]
    fn gripper_opening_maps_normalized_readings_to_a_fraction() {
        assert_eq!(
            JointAngles::default().with_gripper_normalized(-1.0).gripper,
            0.0
        );
        assert_eq!(
            JointAngles::default().with_gripper_normalized(0.0).gripper,
            0.5
        );
        assert_eq!(
            JointAngles::default().with_gripper_normalized(1.0).gripper,
            1.0
        );
        assert_eq!(
            JointAngles::default().with_gripper_normalized(9.0).gripper,
            1.0
        );
        assert_eq!(
            JointAngles::default()
                .with_gripper_normalized(f64::NAN)
                .gripper,
            0.5
        );
    }

    /// Where each pivot of the shipped description lands for `angles`.
    fn pivots(angles: &JointAngles) -> BTreeMap<&'static str, glam::Vec3> {
        let urdf = Urdf::parse(crate::urdf::SO101_URDF).unwrap();
        let transforms = urdf.pose(&so101_joint_positions(&urdf, angles));
        [
            "upper_arm_link",
            "forearm_link",
            "wrist_link",
            "gripper_link",
        ]
        .into_iter()
        .map(|name| {
            let index = urdf.link_index(name).unwrap();
            (name, transforms[index].transform_point3(glam::Vec3::ZERO))
        })
        .collect()
    }

    #[test]
    fn a_centered_arm_rests_in_a_reaching_pose() {
        let at = pivots(&JointAngles::default());

        // The rest pose puts the forearm horizontal and the tool angled down
        // and forward, which is what makes a mid-travel calibration read as a
        // recognizable arm rather than a stick pointing at the ceiling.
        assert!((at["wrist_link"].z - at["forearm_link"].z).abs() < 1e-6);
        assert!(at["forearm_link"].x > at["upper_arm_link"].x);
        assert!(at["gripper_link"].x > at["wrist_link"].x);
        assert!(at["gripper_link"].z < at["wrist_link"].z);
    }

    #[test]
    fn lifting_the_shoulder_raises_the_tool() {
        let lowered = pivots(&JointAngles {
            lift_deg: 20.0,
            ..JointAngles::default()
        });
        let raised = pivots(&JointAngles {
            lift_deg: -20.0,
            ..JointAngles::default()
        });

        assert!(raised["forearm_link"].z > lowered["forearm_link"].z);
    }

    #[test]
    fn panning_swings_the_arm_about_the_vertical_axis() {
        let ahead = pivots(&JointAngles::default());
        let turned = pivots(&JointAngles {
            pan_deg: 90.0,
            ..JointAngles::default()
        });

        // A quarter turn about the vertical takes the reach from +X onto +Y
        // without changing how far out or how high the arm is holding it.
        assert!(turned["gripper_link"].x.abs() < 1e-6);
        assert!((turned["gripper_link"].y - ahead["gripper_link"].x).abs() < 1e-6);
        assert!((turned["gripper_link"].z - ahead["gripper_link"].z).abs() < 1e-6);
    }

    #[test]
    fn link_lengths_come_from_the_description_for_every_pose() {
        let urdf = Urdf::parse(crate::urdf::SO101_URDF).unwrap();
        let reference = pivots(&JointAngles::default());
        let bent = pivots(&JointAngles {
            pan_deg: 45.0,
            lift_deg: -35.0,
            elbow_deg: 80.0,
            wrist_flex_deg: -70.0,
            wrist_roll_deg: 10.0,
            gripper: 0.25,
        });

        // Bending the arm moves the pivots but cannot stretch the links, and
        // the lengths it preserves are the ones the description states.
        for (child, parent) in [
            ("forearm_link", "upper_arm_link"),
            ("wrist_link", "forearm_link"),
            ("gripper_link", "wrist_link"),
        ] {
            let stated = urdf
                .joint_between(parent, child)
                .unwrap()
                .origin
                .translation
                .length();
            for placed in [&reference, &bent] {
                assert!(
                    ((placed[child] - placed[parent]).length() - stated).abs() < 1e-6,
                    "{parent} to {child} should stay {stated} m apart"
                );
            }
        }
    }

    #[test]
    fn the_gripper_fraction_opens_the_jaw_the_description_describes() {
        let urdf = Urdf::parse(crate::urdf::SO101_URDF).unwrap();
        let (_, open) = urdf.joint("gripper").unwrap().limit.unwrap();

        let jaw = |gripper| {
            so101_joint_positions(
                &urdf,
                &JointAngles {
                    gripper,
                    ..JointAngles::default()
                },
            )["gripper"]
        };

        assert_eq!(jaw(0.0), 0.0);
        assert_eq!(jaw(1.0), open);
        assert!((jaw(0.5) - open / 2.0).abs() < 1e-6);
        // A reading that escaped its calibrated range must not fold the jaw
        // through the other one.
        assert_eq!(jaw(4.0), open);
        assert_eq!(jaw(-1.0), 0.0);
    }

    #[test]
    fn every_motor_places_a_joint_of_the_description() {
        let urdf = Urdf::parse(crate::urdf::SO101_URDF).unwrap();
        let positions = so101_joint_positions(&urdf, &JointAngles::default());

        for joint in SO101_JOINTS {
            assert!(
                positions.contains_key(joint.name),
                "{} places nothing",
                joint.name
            );
        }
        assert_eq!(positions.len(), SO101_JOINTS.len());
    }
}
