//! Arms, joints, and cameras, turned into a scene.
//!
//! This is the only module here that knows what any of those words mean, and it
//! knows the neutral joint values through `spiral-robotics-core` and owns the
//! renderer-facing robot description itself. What comes back is a [`Scene`] the layers
//! below can draw without learning any of it, plus the table that says which
//! [`EntityKey`] was which thing.
//!
//! Everything drawn comes from the robot description. Link lengths, joint axes,
//! and reach exist in the URDF file and nowhere else, including not here.

use std::collections::BTreeMap;

use glam::{EulerRot, Mat4, Quat, Vec3};
use spiral_robotics_core::arm::JointAngles;

use crate::scene::{Condition, Entity, EntityKey, Interaction, Scene};
use crate::urdf::{Geometry, Urdf};

/// How far apart two arms stand on the bench, in metres.
pub const BENCH_SPACING: f32 = 0.34;

/// The link angles that make a centered calibration recognizable on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RestPose {
    lift_deg: f64,
    elbow_deg: f64,
    wrist_flex_deg: f64,
}

const SO101_REST_POSE: RestPose = RestPose {
    lift_deg: 30.0,
    elbow_deg: 60.0,
    wrist_flex_deg: 30.0,
};

/// Place a description's joints from measured SO-101 angles.
///
/// The description's own zero is the mechanical one — every joint centered,
/// arm straight up — while a calibration's zero is the middle of whatever
/// travel the operator happened to sweep. This presentation mapping bridges
/// those coordinate systems without making the domain depend on the renderer.
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

/// One addressable physical thing on the bench.
///
/// The studio maps these onto its own selection type. They are stated here
/// because this is the layer that creates them, and a table the renderer cannot
/// see is what keeps the renderer from growing opinions about joints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RobotEntity {
    /// One actuated joint, and every shell that moves with it.
    Joint { robot: String, joint: String },
    /// Structure ahead of the first joint — the base an arm stands on.
    Structure { robot: String },
    /// A camera mounted on this robot, drawn as the volume it can see.
    Camera { robot: String, role: String },
}

impl RobotEntity {
    pub fn robot(&self) -> &str {
        match self {
            Self::Joint { robot, .. } | Self::Structure { robot } | Self::Camera { robot, .. } => {
                robot
            }
        }
    }
}

/// Where a camera is bolted, and how much it sees.
///
/// The frame is a link in the robot description, so a wrist camera rides the
/// wrist, or `None` for one standing on the bench watching the workspace.
/// Nothing is drawn for a camera without a mount: an unmounted camera has no
/// position, and drawing it at the origin would state one it does not have.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraMount {
    pub role: String,
    /// Link the camera rides, or `None` for one fixed to the world.
    pub frame: Option<String>,
    /// Offset from that frame, in metres.
    pub xyz: [f32; 3],
    /// Roll, pitch, and yaw from that frame, in radians.
    pub rpy: [f32; 3],
    /// Horizontal field of view, in degrees.
    pub fov_degrees: f32,
}

/// How far a drawn camera frustum reaches, in metres.
///
/// A frustum is an answer to "is this pointed at the workspace", not a claim
/// about focus distance, so it is drawn long enough to reach across the bench
/// and no longer.
const FRUSTUM_DEPTH: f32 = 0.28;

/// One arm, ready to be turned into shapes.
#[derive(Clone, Debug)]
pub struct ArmPlacement {
    pub id: String,
    pub accent: [f32; 4],
    /// False for an arm that is configured but not currently reporting, which
    /// is drawn at its rest pose and faded rather than left out — an empty
    /// bench says less than a greyed-out one.
    pub live: bool,
    pub angles: JointAngles,
    pub cameras: Vec<CameraMount>,
    /// This arm's index and the number of arms, which is how it finds its spot.
    pub index: usize,
    pub count: usize,
}

impl ArmPlacement {
    /// Where this arm stands, centred with the others on the origin.
    pub fn base(&self) -> Mat4 {
        Mat4::from_translation(Vec3::new(
            0.0,
            (self.index as f32 - self.count.saturating_sub(1) as f32 / 2.0) * BENCH_SPACING,
            0.0,
        ))
    }
}

/// A scene and the table that says what is in it.
pub struct RobotScene {
    pub scene: Scene,
    /// Indexed by [`EntityKey`], so a pick is one lookup.
    entities: Vec<RobotEntity>,
}

impl RobotScene {
    /// What a picked key refers to.
    pub fn entity(&self, key: EntityKey) -> Option<&RobotEntity> {
        self.entities.get(key.0 as usize)
    }
}

/// Build the bench from the description and what the arms are doing.
///
/// `interaction` and `condition` are asked about each entity rather than stored
/// on it: what is hovered changes every pointer move and what is hot changes
/// twenty times a second, and neither is something this crate should be told
/// twice.
pub fn build(
    urdf: &Urdf,
    placements: &[ArmPlacement],
    grid: bool,
    mut interaction: impl FnMut(&RobotEntity) -> Interaction,
    mut condition: impl FnMut(&RobotEntity) -> Condition,
) -> RobotScene {
    let mut entities = Vec::new();
    let mut scene = Scene {
        entities: Vec::new(),
        grid,
    };

    for placement in placements {
        let base = placement.base();
        let positions = so101_joint_positions(urdf, &placement.angles);
        let transforms = urdf.pose(&positions);

        for (link_index, link) in urdf.links.iter().enumerate() {
            let semantic = match controlling_joint(urdf, link_index) {
                Some(joint) => RobotEntity::Joint {
                    robot: placement.id.clone(),
                    joint,
                },
                None => RobotEntity::Structure {
                    robot: placement.id.clone(),
                },
            };
            let key = intern(&mut entities, semantic);
            let semantic = &entities[key.0 as usize];
            let (interaction, condition) = (interaction(semantic), condition(semantic));

            for visual in &link.visuals {
                let color = match visual.material.as_deref() {
                    Some("accent") => placement.accent,
                    _ => visual.color,
                };
                scene.entities.push(Entity {
                    key,
                    geometry: visual.geometry.clone(),
                    transform: base * transforms[link_index] * visual.origin.matrix(),
                    color: if placement.live { color } else { faded(color) },
                    interaction,
                    condition,
                    pickable: true,
                    wireframe: false,
                });
            }
        }

        for mount in &placement.cameras {
            let Some(frame) = mount_transform(urdf, &transforms, mount) else {
                continue;
            };
            let semantic = RobotEntity::Camera {
                robot: placement.id.clone(),
                role: mount.role.clone(),
            };
            let key = intern(&mut entities, semantic);
            let semantic = &entities[key.0 as usize];
            let (interaction, condition) = (interaction(semantic), condition(semantic));
            let spread = (mount.fov_degrees.to_radians() * 0.5).tan() * FRUSTUM_DEPTH;

            scene.entities.push(Entity {
                key,
                // A cone standing in for the volume the camera can see. Drawn
                // as lines, because what matters is where it points, and a
                // solid wedge would be an obstacle through the middle of the
                // bench rather than a note about coverage.
                geometry: Geometry::Cylinder {
                    radius: spread.max(0.01),
                    length: FRUSTUM_DEPTH,
                },
                transform: base
                    * frame
                    * Mat4::from_translation(Vec3::new(0.0, 0.0, FRUSTUM_DEPTH * 0.5)),
                color: if placement.live {
                    placement.accent
                } else {
                    faded(placement.accent)
                },
                interaction,
                condition,
                pickable: true,
                wireframe: true,
            });
        }
    }

    RobotScene { scene, entities }
}

/// Give `semantic` a key, reusing the one it already has.
///
/// Every shell of one joint therefore shares a key, which is what makes picking
/// any of them select the joint without the renderer knowing what a joint is.
fn intern(entities: &mut Vec<RobotEntity>, semantic: RobotEntity) -> EntityKey {
    match entities.iter().position(|held| *held == semantic) {
        Some(index) => EntityKey(index as u32),
        None => {
            entities.push(semantic);
            EntityKey((entities.len() - 1) as u32)
        }
    }
}

/// Where a mounted camera sits, relative to its arm's base.
fn mount_transform(urdf: &Urdf, transforms: &[Mat4], mount: &CameraMount) -> Option<Mat4> {
    let frame = match &mount.frame {
        Some(link) => *transforms.get(urdf.link_index(link)?)?,
        // A camera on the bench rather than on the arm still stands in the
        // arm's own space, so a two-arm workspace does not put both top
        // cameras in the same place.
        None => Mat4::IDENTITY,
    };
    Some(
        frame
            * Mat4::from_rotation_translation(
                Quat::from_euler(EulerRot::XYZ, mount.rpy[0], mount.rpy[1], mount.rpy[2]),
                Vec3::from_array(mount.xyz),
            ),
    )
}

/// The nearest movable joint above `link`, which is what moves that link.
fn controlling_joint(urdf: &Urdf, mut link: usize) -> Option<String> {
    while link != urdf.root() {
        let joint = urdf.joints.iter().find(|joint| joint.child == link)?;
        if joint.kind.is_movable() {
            return Some(joint.name.clone());
        }
        link = joint.parent;
    }
    None
}

/// Wash a colour out toward the background, marking an arm that is not
/// reporting so a stale pose is never mistaken for a live one.
fn faded(color: [f32; 4]) -> [f32; 4] {
    const TOWARD: [f32; 3] = [0.82, 0.82, 0.80];
    let mix = |value: f32, target: f32| value + (target - value) * 0.55;
    [
        mix(color[0], TOWARD[0]),
        mix(color[1], TOWARD[1]),
        mix(color[2], TOWARD[2]),
        color[3],
    ]
}

/// Where the gripper tip sits for `angles`, in millimetres from the arm's base.
///
/// The description carries a tool frame for exactly this, so no length written
/// here has to agree with one written there.
pub fn tool_position(urdf: &Urdf, angles: &JointAngles) -> Option<Vec3> {
    let index = urdf.link_index("tool_link")?;
    let transforms = urdf.pose(&so101_joint_positions(urdf, angles));
    Some(transforms.get(index)?.transform_point3(Vec3::ZERO) * 1000.0)
}

#[cfg(test)]
mod tests {
    use spiral_robotics_core::arm::SO101_JOINTS;

    use super::*;
    use crate::urdf::SO101_URDF;

    #[test]
    fn every_shell_of_one_joint_shares_its_key() {
        let mut entities = Vec::new();
        let joint = |name: &str| RobotEntity::Joint {
            robot: "follower".to_owned(),
            joint: name.to_owned(),
        };

        let first = intern(&mut entities, joint("elbow_flex"));
        let again = intern(&mut entities, joint("elbow_flex"));
        let other = intern(&mut entities, joint("wrist_flex"));

        assert_eq!(first, again, "picking any shell selects the one joint");
        assert_ne!(first, other);
        assert_eq!(entities.len(), 2);
    }

    #[test]
    fn arms_stand_in_a_row_centred_on_the_origin() {
        let placement = |index, count| ArmPlacement {
            id: "arm".to_owned(),
            accent: [0.0; 4],
            live: true,
            angles: JointAngles::default(),
            cameras: Vec::new(),
            index,
            count,
        };

        let alone = placement(0, 1).base().transform_point3(Vec3::ZERO);
        assert_eq!(alone, Vec3::ZERO);

        let left = placement(0, 2).base().transform_point3(Vec3::ZERO);
        let right = placement(1, 2).base().transform_point3(Vec3::ZERO);
        assert!(
            (left.y + right.y).abs() < 1e-6,
            "a pair straddles the origin"
        );
        assert!((right.y - left.y - BENCH_SPACING).abs() < 1e-6);
    }

    /// Where each pivot of the shipped description lands for `angles`.
    fn pivots(angles: &JointAngles) -> BTreeMap<&'static str, Vec3> {
        let urdf = Urdf::parse(SO101_URDF).unwrap();
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
            (name, transforms[index].transform_point3(Vec3::ZERO))
        })
        .collect()
    }

    #[test]
    fn a_centered_arm_rests_in_a_reaching_pose() {
        let at = pivots(&JointAngles::default());

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

        assert!(turned["gripper_link"].x.abs() < 1e-6);
        assert!((turned["gripper_link"].y - ahead["gripper_link"].x).abs() < 1e-6);
        assert!((turned["gripper_link"].z - ahead["gripper_link"].z).abs() < 1e-6);
    }

    #[test]
    fn link_lengths_come_from_the_description_for_every_pose() {
        let urdf = Urdf::parse(SO101_URDF).unwrap();
        let reference = pivots(&JointAngles::default());
        let bent = pivots(&JointAngles {
            pan_deg: 45.0,
            lift_deg: -35.0,
            elbow_deg: 80.0,
            wrist_flex_deg: -70.0,
            wrist_roll_deg: 10.0,
            gripper: 0.25,
        });

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
    fn the_gripper_fraction_opens_the_described_jaw() {
        let urdf = Urdf::parse(SO101_URDF).unwrap();
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
        assert_eq!(jaw(4.0), open);
        assert_eq!(jaw(-1.0), 0.0);
    }

    #[test]
    fn every_motor_places_a_joint_of_the_description() {
        let urdf = Urdf::parse(SO101_URDF).unwrap();
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
