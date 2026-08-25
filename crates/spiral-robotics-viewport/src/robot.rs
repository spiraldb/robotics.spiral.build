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

use crate::scene::{Condition, Entity, EntityKey, EntityShape, Interaction, Scene};
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
    role: String,
    attachment: CameraAttachment,
    /// Horizontal field of view, in degrees.
    field_of_view_degrees: f32,
}

impl CameraMount {
    pub fn new(
        role: impl Into<String>,
        attachment: CameraAttachment,
        field_of_view_degrees: f32,
    ) -> Self {
        Self {
            role: role.into(),
            attachment,
            field_of_view_degrees,
        }
    }

    /// Where this camera sits relative to its arm's base.
    fn transform(&self, urdf: &Urdf, link_transforms: &[Mat4]) -> Option<Mat4> {
        let attachment_frame = match &self.attachment.frame {
            Some(link_name) => *link_transforms.get(urdf.link_index(link_name)?)?,
            // A camera on the bench rather than on the arm still stands in
            // the arm's own space, so a two-arm workspace does not put both
            // top cameras in the same place.
            None => Mat4::IDENTITY,
        };
        Some(
            attachment_frame
                * Mat4::from_rotation_translation(
                    Quat::from_euler(
                        EulerRot::XYZ,
                        self.attachment.roll_pitch_yaw[0],
                        self.attachment.roll_pitch_yaw[1],
                        self.attachment.roll_pitch_yaw[2],
                    ),
                    Vec3::from_array(self.attachment.translation),
                ),
        )
    }
}

/// A camera's rigid attachment to an optional robot link.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraAttachment {
    /// Link the camera rides, or `None` for one fixed to the world.
    frame: Option<String>,
    /// Offset from that frame, in metres.
    translation: [f32; 3],
    /// Roll, pitch, and yaw from that frame, in radians.
    roll_pitch_yaw: [f32; 3],
}

impl CameraAttachment {
    pub fn new(frame: Option<String>, translation: [f32; 3], roll_pitch_yaw: [f32; 3]) -> Self {
        Self {
            frame,
            translation,
            roll_pitch_yaw,
        }
    }
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
    id: String,
    accent: [f32; 4],
    pose: ArmPose,
    cameras: Vec<CameraMount>,
    bench_position: BenchPosition,
}

impl ArmPlacement {
    /// Start an offline arm at rest in its assigned bench position.
    pub fn new(id: impl Into<String>, accent: [f32; 4], bench_position: BenchPosition) -> Self {
        Self {
            id: id.into(),
            accent,
            pose: ArmPose::default(),
            cameras: Vec::new(),
            bench_position,
        }
    }

    pub fn with_live_pose(mut self, angles: JointAngles) -> Self {
        self.pose = ArmPose { live: true, angles };
        self
    }

    pub fn with_cameras(mut self, cameras: Vec<CameraMount>) -> Self {
        self.cameras = cameras;
        self
    }

    /// Where this arm stands, centred with the others on the origin.
    fn base_transform(&self) -> Mat4 {
        Mat4::from_translation(self.bench_position.translation())
    }

    fn display_color(&self, color: [f32; 4]) -> [f32; 4] {
        if self.pose.live {
            color
        } else {
            const OFFLINE_TINT: [f32; 3] = [0.82, 0.82, 0.80];
            let mix = |value: f32, target: f32| value + (target - value) * 0.55;
            [
                mix(color[0], OFFLINE_TINT[0]),
                mix(color[1], OFFLINE_TINT[1]),
                mix(color[2], OFFLINE_TINT[2]),
                color[3],
            ]
        }
    }
}

#[derive(Clone, Debug, Default)]
struct ArmPose {
    live: bool,
    angles: JointAngles,
}

/// An arm's validated position in the centered row on the bench.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BenchPosition {
    index: usize,
    count: usize,
}

impl BenchPosition {
    pub fn new(index: usize, count: usize) -> Self {
        let count = count.max(1);
        Self {
            index: index.min(count - 1),
            count,
        }
    }

    pub fn translation(self) -> Vec3 {
        Vec3::new(
            0.0,
            (self.index as f32 - self.count.saturating_sub(1) as f32 / 2.0) * BENCH_SPACING,
            0.0,
        )
    }
}

/// A scene and the table that says what is in it.
pub struct RobotScene {
    scene: Scene,
    /// Indexed by [`EntityKey`], so a pick is one lookup.
    entities: Vec<RobotEntity>,
}

impl RobotScene {
    /// What a picked key refers to.
    pub fn entity(&self, key: EntityKey) -> Option<&RobotEntity> {
        self.entities.get(key.index())
    }

    pub fn scene(&self) -> &Scene {
        &self.scene
    }
}

/// How application state should decorate the otherwise neutral robot scene.
pub struct SceneAppearance<InteractionFn, ConditionFn> {
    grid_enabled: bool,
    interaction_for: InteractionFn,
    condition_for: ConditionFn,
}

impl<InteractionFn, ConditionFn> SceneAppearance<InteractionFn, ConditionFn> {
    pub fn new(
        grid_enabled: bool,
        interaction_for: InteractionFn,
        condition_for: ConditionFn,
    ) -> Self {
        Self {
            grid_enabled,
            interaction_for,
            condition_for,
        }
    }
}

/// Build the bench from the description and what the arms are doing.
///
/// `interaction` and `condition` are asked about each entity rather than stored
/// on it: what is hovered changes every pointer move and what is hot changes
/// twenty times a second, and neither is something this crate should be told
/// twice.
impl RobotScene {
    pub fn compose<InteractionFn, ConditionFn>(
        urdf: &Urdf,
        placements: &[ArmPlacement],
        mut appearance: SceneAppearance<InteractionFn, ConditionFn>,
    ) -> Self
    where
        InteractionFn: FnMut(&RobotEntity) -> Interaction,
        ConditionFn: FnMut(&RobotEntity) -> Condition,
    {
        let mut entity_catalog = EntityCatalog::default();
        let mut scene = Scene::new(appearance.grid_enabled);

        for placement in placements {
            let base_transform = placement.base_transform();
            let joint_positions = urdf.so101_joint_positions(&placement.pose.angles);
            let link_transforms = urdf.pose(&joint_positions);

            for (link_index, link) in urdf.links().iter().enumerate() {
                let robot_entity = match urdf.controlling_joint(link_index) {
                    Some(joint) => RobotEntity::Joint {
                        robot: placement.id.clone(),
                        joint,
                    },
                    None => RobotEntity::Structure {
                        robot: placement.id.clone(),
                    },
                };
                let key = entity_catalog.key_for(robot_entity);
                let robot_entity = entity_catalog.entity(key);
                let state = (
                    (appearance.interaction_for)(robot_entity),
                    (appearance.condition_for)(robot_entity),
                );

                for visual in link.visuals() {
                    let color = match visual.material() {
                        Some("accent") => placement.accent,
                        _ => visual.color(),
                    };
                    scene.push(Entity::new(
                        key,
                        EntityShape::solid(
                            visual.geometry().clone(),
                            base_transform * link_transforms[link_index] * visual.origin().matrix(),
                            placement.display_color(color),
                        ),
                        state,
                    ));
                }
            }

            for camera_mount in &placement.cameras {
                let Some(camera_transform) = camera_mount.transform(urdf, &link_transforms) else {
                    continue;
                };
                let robot_entity = RobotEntity::Camera {
                    robot: placement.id.clone(),
                    role: camera_mount.role.clone(),
                };
                let key = entity_catalog.key_for(robot_entity);
                let robot_entity = entity_catalog.entity(key);
                let state = (
                    (appearance.interaction_for)(robot_entity),
                    (appearance.condition_for)(robot_entity),
                );
                let frustum_radius =
                    (camera_mount.field_of_view_degrees.to_radians() * 0.5).tan() * FRUSTUM_DEPTH;

                scene.push(Entity::new(
                    key,
                    EntityShape::wireframe(
                        // A cone standing in for the volume the camera can see. Drawn
                        // as lines, because what matters is where it points, and a
                        // solid wedge would be an obstacle through the middle of the
                        // bench rather than a note about coverage.
                        Geometry::Cylinder {
                            radius: frustum_radius.max(0.01),
                            length: FRUSTUM_DEPTH,
                        },
                        base_transform
                            * camera_transform
                            * Mat4::from_translation(Vec3::new(0.0, 0.0, FRUSTUM_DEPTH * 0.5)),
                        placement.display_color(placement.accent),
                    ),
                    state,
                ));
            }
        }

        Self {
            scene,
            entities: entity_catalog.entities,
        }
    }
}

/// Give `semantic` a key, reusing the one it already has.
///
/// Every shell of one joint therefore shares a key, which is what makes picking
/// any of them select the joint without the renderer knowing what a joint is.
#[derive(Default)]
struct EntityCatalog {
    entities: Vec<RobotEntity>,
}

impl EntityCatalog {
    fn key_for(&mut self, robot_entity: RobotEntity) -> EntityKey {
        match self
            .entities
            .iter()
            .position(|existing_entity| *existing_entity == robot_entity)
        {
            Some(index) => EntityKey::from_index(index),
            None => {
                self.entities.push(robot_entity);
                EntityKey::from_index(self.entities.len() - 1)
            }
        }
    }

    fn entity(&self, key: EntityKey) -> &RobotEntity {
        &self.entities[key.index()]
    }
}

impl Urdf {
    /// Place this description's joints from measured SO-101 angles.
    fn so101_joint_positions(&self, angles: &JointAngles) -> BTreeMap<String, f32> {
        let rest_pose = SO101_REST_POSE;
        let maximum_gripper_position = self
            .joint("gripper")
            .and_then(|joint| joint.limit())
            .map_or(0.0, |(_, upper)| upper);

        [
            ("shoulder_pan", angles.pan_degrees().to_radians() as f32),
            (
                "shoulder_lift",
                (rest_pose.lift_deg + angles.lift_degrees()).to_radians() as f32,
            ),
            (
                "elbow_flex",
                (rest_pose.elbow_deg + angles.elbow_degrees()).to_radians() as f32,
            ),
            (
                "wrist_flex",
                (rest_pose.wrist_flex_deg + angles.wrist_flex_degrees()).to_radians() as f32,
            ),
            (
                "wrist_roll",
                angles.wrist_roll_degrees().to_radians() as f32,
            ),
            (
                "gripper",
                angles.gripper_opening().clamp(0.0, 1.0) as f32 * maximum_gripper_position,
            ),
        ]
        .into_iter()
        .map(|(joint_name, position)| (joint_name.to_owned(), position))
        .collect()
    }

    /// The nearest movable joint above `link_index`, which moves that link.
    fn controlling_joint(&self, mut link_index: usize) -> Option<String> {
        while link_index != self.root() {
            let joint = self
                .joints()
                .iter()
                .find(|joint| joint.child() == link_index)?;
            if joint.kind().is_movable() {
                return Some(joint.name().to_owned());
            }
            link_index = joint.parent();
        }
        None
    }

    /// Where the gripper tip sits, in millimetres from the arm's base.
    pub fn so101_tool_position(&self, angles: &JointAngles) -> Option<Vec3> {
        let tool_link_index = self.link_index("tool_link")?;
        let link_transforms = self.pose(&self.so101_joint_positions(angles));
        Some(
            link_transforms
                .get(tool_link_index)?
                .transform_point3(Vec3::ZERO)
                * 1000.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_core::arm::SO101_JOINTS;

    use super::*;
    use crate::urdf::SO101_URDF;

    #[test]
    fn every_shell_of_one_joint_shares_its_key() {
        let mut entity_catalog = EntityCatalog::default();
        let joint = |name: &str| RobotEntity::Joint {
            robot: "follower".to_owned(),
            joint: name.to_owned(),
        };

        let first = entity_catalog.key_for(joint("elbow_flex"));
        let again = entity_catalog.key_for(joint("elbow_flex"));
        let other = entity_catalog.key_for(joint("wrist_flex"));

        assert_eq!(first, again, "picking any shell selects the one joint");
        assert_ne!(first, other);
        assert_eq!(entity_catalog.entities.len(), 2);
    }

    #[test]
    fn arms_stand_in_a_row_centred_on_the_origin() {
        let placement = |index, count| {
            ArmPlacement::new("arm", [0.0; 4], BenchPosition::new(index, count))
                .with_live_pose(JointAngles::default())
        };

        let alone = placement(0, 1)
            .base_transform()
            .transform_point3(Vec3::ZERO);
        assert_eq!(alone, Vec3::ZERO);

        let left = placement(0, 2)
            .base_transform()
            .transform_point3(Vec3::ZERO);
        let right = placement(1, 2)
            .base_transform()
            .transform_point3(Vec3::ZERO);
        assert!(
            (left.y + right.y).abs() < 1e-6,
            "a pair straddles the origin"
        );
        assert!((right.y - left.y - BENCH_SPACING).abs() < 1e-6);
    }

    /// Where each pivot of the shipped description lands for `angles`.
    fn pivots(angles: &JointAngles) -> BTreeMap<&'static str, Vec3> {
        let urdf = Urdf::parse(SO101_URDF).unwrap();
        let transforms = urdf.pose(&urdf.so101_joint_positions(angles));
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
        let lowered = pivots(&JointAngles::default().with_lift_degrees(20.0));
        let raised = pivots(&JointAngles::default().with_lift_degrees(-20.0));

        assert!(raised["forearm_link"].z > lowered["forearm_link"].z);
    }

    #[test]
    fn panning_swings_the_arm_about_the_vertical_axis() {
        let ahead = pivots(&JointAngles::default());
        let turned = pivots(&JointAngles::default().with_pan_degrees(90.0));

        assert!(turned["gripper_link"].x.abs() < 1e-6);
        assert!((turned["gripper_link"].y - ahead["gripper_link"].x).abs() < 1e-6);
        assert!((turned["gripper_link"].z - ahead["gripper_link"].z).abs() < 1e-6);
    }

    #[test]
    fn link_lengths_come_from_the_description_for_every_pose() {
        let urdf = Urdf::parse(SO101_URDF).unwrap();
        let reference = pivots(&JointAngles::default());
        let bent = pivots(
            &JointAngles::default()
                .with_pan_degrees(45.0)
                .with_lift_degrees(-35.0)
                .with_elbow_degrees(80.0)
                .with_wrist_flex_degrees(-70.0)
                .with_wrist_roll_degrees(10.0)
                .with_gripper_normalized(0.5),
        );

        for (child, parent) in [
            ("forearm_link", "upper_arm_link"),
            ("wrist_link", "forearm_link"),
            ("gripper_link", "wrist_link"),
        ] {
            let stated = urdf
                .joint_between(parent, child)
                .unwrap()
                .origin()
                .translation()
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
        let (_, open) = urdf.joint("gripper").unwrap().limit().unwrap();
        let jaw = |gripper_opening: f64| {
            urdf.so101_joint_positions(
                &JointAngles::default().with_gripper_normalized(1.0 - 2.0 * gripper_opening),
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
        let positions = urdf.so101_joint_positions(&JointAngles::default());

        for joint in SO101_JOINTS {
            assert!(
                positions.contains_key(joint.name()),
                "{} places nothing",
                joint.name()
            );
        }
        assert_eq!(positions.len(), SO101_JOINTS.len());
    }
}
