//! The SO-ARM101 joint model and the planar geometry the web UI draws.
//!
//! Everything here is display- and mapping-level knowledge: which motors an
//! SO-101 has, how a raw encoder count becomes an angle, and where the links
//! land once the angles are known. The module deliberately holds no dynamics
//! and no inverse kinematics; the UI drives joints directly and only needs a
//! faithful picture of where they currently are.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::calibration::{Direction, MotorSpec};

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

/// Nominal link lengths, in millimetres, used to draw the arm.
///
/// These are approximate SO-101 proportions chosen so the rendered arm matches
/// the real one closely enough to read a pose at a glance. They affect the
/// picture only: no command sent to a motor depends on them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArmGeometry {
    /// Mounting plate to the shoulder-lift pivot.
    pub base_height_mm: f64,
    /// Horizontal offset of the shoulder-lift pivot from the pan axis.
    pub shoulder_offset_mm: f64,
    /// Shoulder-lift pivot to the elbow pivot.
    pub upper_arm_mm: f64,
    /// Elbow pivot to the wrist pivot.
    pub forearm_mm: f64,
    /// Wrist pivot to the gripper tip.
    pub wrist_mm: f64,
    /// Full opening of the gripper jaws.
    pub gripper_span_mm: f64,
}

/// Nominal geometry of a stock SO-ARM101.
pub const SO101_GEOMETRY: ArmGeometry = ArmGeometry {
    base_height_mm: 55.0,
    shoulder_offset_mm: 0.0,
    upper_arm_mm: 117.0,
    forearm_mm: 135.0,
    wrist_mm: 75.0,
    gripper_span_mm: 45.0,
};

/// The side-view link angles that correspond to every joint sitting at its
/// calibrated center.
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

/// A point in the arm's vertical working plane, in millimetres.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Point {
    /// Horizontal distance in front of the pan axis.
    pub x: f64,
    /// Height above the mounting plate.
    pub y: f64,
}

/// Where every link of the arm sits for a given set of joint angles.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ArmPose {
    /// Pan axis at the mounting plate.
    pub base: Point,
    /// Shoulder-lift pivot.
    pub shoulder: Point,
    /// Elbow pivot.
    pub elbow: Point,
    /// Wrist pivot.
    pub wrist: Point,
    /// Gripper tip.
    pub tool: Point,
    /// Absolute tool direction in the side view, degrees from vertical.
    pub tool_deg: f64,
    /// Rotation of the whole arm about the base, degrees.
    pub pan_deg: f64,
    /// Roll of the tool about the forearm axis, degrees.
    pub wrist_roll_deg: f64,
    /// Gripper opening from `0.0` (closed) to `1.0` (fully open).
    pub gripper: f64,
}

impl ArmPose {
    /// Solve the side-view link positions for `angles`.
    pub fn solve(geometry: &ArmGeometry, rest: &RestPose, angles: &JointAngles) -> Self {
        let lift = rest.lift_deg + angles.lift_deg;
        let elbow = rest.elbow_deg + angles.elbow_deg;
        let wrist = rest.wrist_flex_deg + angles.wrist_flex_deg;

        // Link angles accumulate down the chain: each joint bends its link
        // relative to the one before it.
        let upper_arm_deg = lift;
        let forearm_deg = upper_arm_deg + elbow;
        let tool_deg = forearm_deg + wrist;

        let base = Point { x: 0.0, y: 0.0 };
        let shoulder = Point {
            x: geometry.shoulder_offset_mm,
            y: geometry.base_height_mm,
        };
        let elbow_point = extend(shoulder, upper_arm_deg, geometry.upper_arm_mm);
        let wrist_point = extend(elbow_point, forearm_deg, geometry.forearm_mm);
        let tool = extend(wrist_point, tool_deg, geometry.wrist_mm);

        Self {
            base,
            shoulder,
            elbow: elbow_point,
            wrist: wrist_point,
            tool,
            tool_deg,
            pan_deg: angles.pan_deg,
            wrist_roll_deg: angles.wrist_roll_deg,
            gripper: angles.gripper.clamp(0.0, 1.0),
        }
    }

    /// Solve using the stock SO-101 geometry and rest pose.
    pub fn solve_so101(angles: &JointAngles) -> Self {
        Self::solve(&SO101_GEOMETRY, &SO101_REST_POSE, angles)
    }

    /// Horizontal distance from the pan axis to the gripper tip.
    pub fn reach_mm(&self) -> f64 {
        self.tool.x
    }
}

/// Advance `from` by `length` in the direction `degrees` measured from vertical.
fn extend(from: Point, degrees: f64, length: f64) -> Point {
    let radians = degrees.to_radians();
    Point {
        x: from.x + length * radians.sin(),
        y: from.y + length * radians.cos(),
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

    #[test]
    fn a_centered_arm_rests_in_a_reaching_pose() {
        let pose = ArmPose::solve_so101(&JointAngles::default());

        assert_eq!(pose.base, Point { x: 0.0, y: 0.0 });
        assert_eq!(pose.shoulder.y, SO101_GEOMETRY.base_height_mm);
        // Rest angles put the forearm horizontal and the tool angled downward.
        assert!((pose.wrist.y - pose.elbow.y).abs() < 1e-9);
        assert!(pose.tool.x > pose.wrist.x);
        assert!(pose.tool.y < pose.wrist.y);
        assert_eq!(pose.tool_deg, 120.0);
    }

    #[test]
    fn link_lengths_are_preserved_for_every_pose() {
        let angles = JointAngles {
            pan_deg: 45.0,
            lift_deg: -35.0,
            elbow_deg: 80.0,
            wrist_flex_deg: -70.0,
            wrist_roll_deg: 10.0,
            gripper: 0.25,
        };
        let pose = ArmPose::solve_so101(&angles);

        let distance = |a: Point, b: Point| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
        assert!((distance(pose.shoulder, pose.elbow) - SO101_GEOMETRY.upper_arm_mm).abs() < 1e-9);
        assert!((distance(pose.elbow, pose.wrist) - SO101_GEOMETRY.forearm_mm).abs() < 1e-9);
        assert!((distance(pose.wrist, pose.tool) - SO101_GEOMETRY.wrist_mm).abs() < 1e-9);
        assert_eq!(pose.pan_deg, 45.0);
        assert_eq!(pose.reach_mm(), pose.tool.x);
    }

    #[test]
    fn lifting_the_shoulder_raises_the_tool() {
        let lowered = ArmPose::solve_so101(&JointAngles {
            lift_deg: 20.0,
            ..JointAngles::default()
        });
        let raised = ArmPose::solve_so101(&JointAngles {
            lift_deg: -20.0,
            ..JointAngles::default()
        });

        assert!(raised.elbow.y > lowered.elbow.y);
    }
}
