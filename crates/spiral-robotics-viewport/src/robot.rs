//! Arms, joints, and cameras, turned into a scene.
//!
//! This is the only module here that knows what any of those words mean, and it
//! owns both the neutral presentation pose and renderer-facing robot
//! description. What comes back is a [`Scene`] the layers below can draw
//! without learning any of it, plus the table that says which [`EntityKey`] was
//! which thing.
//!
//! Everything drawn comes from the robot description. Link lengths, joint axes,
//! and reach exist in the URDF file and nowhere else, including not here.

use std::collections::BTreeMap;
use std::fmt;

use crate::arm::{ArmPose, Joint};
use crate::scene::{Condition, Entity, EntityKey, EntityShape, Interaction, Scene};
use crate::urdf::{Geometry, Urdf};
use glam::{EulerRot, Mat4, Quat, Vec3};

/// How far apart two arms stand on the bench, in metres.
pub(crate) const BENCH_SPACING: f32 = 0.34;

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
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Target {
    /// One actuated joint, and every shell that moves with it.
    Joint { robot: String, joint: String },
    /// Structure ahead of the first joint — the base an arm stands on.
    Robot { robot: String },
    /// A camera mounted on this robot, drawn as the volume it can see.
    Camera { robot: String, role: String },
}

impl Target {
    pub fn robot(&self) -> &str {
        match self {
            Self::Joint { robot, .. } | Self::Robot { robot } | Self::Camera { robot, .. } => robot,
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
struct CameraPlacement {
    role: String,
    attachment: CameraPlacementAttachment,
    /// Horizontal field of view, in degrees.
    field_of_view_degrees: f32,
}

impl CameraPlacement {
    fn new(
        role: impl Into<String>,
        attachment: CameraPlacementAttachment,
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
struct CameraPlacementAttachment {
    /// Link the camera rides, or `None` for one fixed to the world.
    frame: Option<String>,
    /// Offset from that frame, in metres.
    translation: [f32; 3],
    /// Roll, pitch, and yaw from that frame, in radians.
    roll_pitch_yaw: [f32; 3],
}

impl CameraPlacementAttachment {
    fn new(frame: Option<String>, translation: [f32; 3], roll_pitch_yaw: [f32; 3]) -> Self {
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
struct ArmPlacement {
    id: String,
    accent: [f32; 4],
    pose: PoseState,
    cameras: Vec<CameraPlacement>,
}

impl ArmPlacement {
    /// Start an offline arm at rest.
    fn new(id: impl Into<String>, accent: [f32; 4]) -> Self {
        Self {
            id: id.into(),
            accent,
            pose: PoseState::default(),
            cameras: Vec::new(),
        }
    }

    fn with_live_pose(mut self, angles: ArmPose) -> Self {
        self.pose = PoseState { live: true, angles };
        self
    }

    /// Where this arm stands, centred with the others on the origin.
    fn base_transform(index: usize, count: usize) -> Mat4 {
        debug_assert!(count > 0 && index < count);
        Mat4::from_translation(Vec3::new(
            0.0,
            (index as f32 - count.saturating_sub(1) as f32 / 2.0) * BENCH_SPACING,
            0.0,
        ))
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
struct PoseState {
    live: bool,
    angles: ArmPose,
}

/// The appearance and optional live pose of one arm in a [`Frame`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArmPresentation {
    accent: [f32; 4],
    pose: Option<ArmPose>,
}

impl ArmPresentation {
    pub fn new(accent: [f32; 4], pose: Option<ArmPose>) -> Self {
        Self { accent, pose }
    }
}

/// A camera's rigid attachment to an optional robot link.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraAttachment {
    frame: Option<String>,
    translation: [f64; 3],
    roll_pitch_yaw: [f64; 3],
}

impl CameraAttachment {
    pub fn new(
        frame: Option<impl Into<String>>,
        translation: [f64; 3],
        roll_pitch_yaw: [f64; 3],
    ) -> Self {
        Self {
            frame: frame.map(Into::into),
            translation,
            roll_pitch_yaw,
        }
    }
}

/// A named camera and the mount information needed to draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraMount {
    role: String,
    attachment: CameraAttachment,
    field_of_view_degrees: f64,
}

impl CameraMount {
    pub fn new(
        role: impl Into<String>,
        attachment: CameraAttachment,
        field_of_view_degrees: f64,
    ) -> Self {
        Self {
            role: role.into(),
            attachment,
            field_of_view_degrees,
        }
    }
}

/// One complete, validated set of inputs for a viewport draw.
///
/// Arms are centered on the bench in insertion order. Configuration methods
/// reject malformed stable data; telemetry poses themselves stay permissive.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    grid_enabled: bool,
    placements: Vec<ArmPlacement>,
    interactions: BTreeMap<Target, Interaction>,
    conditions: BTreeMap<Target, Condition>,
}

impl Frame {
    pub fn new(grid_enabled: bool) -> Self {
        Self {
            grid_enabled,
            ..Self::default()
        }
    }

    /// Add one arm. `None` means the arm has no usable live pose and should be
    /// shown at the muted presentation rest pose.
    pub fn add_arm(
        &mut self,
        id: impl Into<String>,
        presentation: ArmPresentation,
    ) -> Result<&mut Self, FrameError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(FrameError::EmptyRobotId);
        }
        if self.placements.iter().any(|arm| arm.id == id) {
            return Err(FrameError::DuplicateRobot(id));
        }
        if !presentation
            .accent
            .into_iter()
            .all(|component| component.is_finite() && (0.0..=1.0).contains(&component))
        {
            return Err(FrameError::InvalidAccent(id));
        }

        let placement = match presentation.pose {
            Some(pose) => ArmPlacement::new(id, presentation.accent).with_live_pose(pose),
            None => ArmPlacement::new(id, presentation.accent),
        };
        self.placements.push(placement);
        Ok(self)
    }

    /// Attach a configured camera to an arm already in this frame.
    pub fn mount_camera(
        &mut self,
        robot: &str,
        mount: CameraMount,
    ) -> Result<&mut Self, FrameError> {
        let Some(placement) = self.placements.iter_mut().find(|arm| arm.id == robot) else {
            return Err(FrameError::UnknownRobot(robot.to_owned()));
        };
        let role = mount.role;
        if role.trim().is_empty() {
            return Err(FrameError::EmptyCameraRole {
                robot: robot.to_owned(),
            });
        }
        if placement.cameras.iter().any(|camera| camera.role == role) {
            return Err(FrameError::DuplicateCamera {
                robot: robot.to_owned(),
                role,
            });
        }
        if let Some(frame) = mount.attachment.frame.as_deref()
            && Urdf::so101().link_index(frame).is_none()
        {
            return Err(FrameError::UnknownCameraFrame {
                robot: robot.to_owned(),
                role,
                frame: frame.to_owned(),
            });
        }
        let translation = mount.attachment.translation.finite_f32().ok_or_else(|| {
            FrameError::InvalidCameraTransform {
                robot: robot.to_owned(),
                role: role.clone(),
            }
        })?;
        let roll_pitch_yaw = mount
            .attachment
            .roll_pitch_yaw
            .finite_f32()
            .ok_or_else(|| FrameError::InvalidCameraTransform {
                robot: robot.to_owned(),
                role: role.clone(),
            })?;
        let field_of_view_degrees = mount.field_of_view_degrees as f32;
        if !field_of_view_degrees.is_finite() || !(0.0..180.0).contains(&field_of_view_degrees) {
            return Err(FrameError::InvalidFieldOfView {
                robot: robot.to_owned(),
                role,
            });
        }

        placement.cameras.push(CameraPlacement::new(
            role,
            CameraPlacementAttachment::new(mount.attachment.frame, translation, roll_pitch_yaw),
            field_of_view_degrees,
        ));
        Ok(self)
    }

    /// Apply interaction to a target present in this frame.
    ///
    /// A robot target scopes the state to the whole arm; an exact joint or
    /// camera state is combined with that scope, with `Selected` winning over
    /// `Hovered`.
    pub fn set_interaction(&mut self, target: Target, interaction: Interaction) -> bool {
        if !self.contains(&target) {
            return false;
        }
        if interaction == Interaction::Resting {
            self.interactions.remove(&target);
        } else {
            self.interactions.insert(target, interaction);
        }
        true
    }

    /// Apply a hardware condition to a target present in this frame.
    pub fn set_condition(&mut self, target: Target, condition: Condition) -> bool {
        if !self.contains(&target) {
            return false;
        }
        if condition == Condition::Nominal {
            self.conditions.remove(&target);
        } else {
            self.conditions.insert(target, condition);
        }
        true
    }

    fn contains(&self, target: &Target) -> bool {
        let Some(placement) = self
            .placements
            .iter()
            .find(|placement| placement.id == target.robot())
        else {
            return false;
        };
        match target {
            Target::Robot { .. } => true,
            Target::Joint { joint, .. } => Joint::named(joint).is_some(),
            Target::Camera { role, .. } => {
                placement.cameras.iter().any(|camera| camera.role == *role)
            }
        }
    }

    fn state_for(&self, target: &Target) -> (Interaction, Condition) {
        let robot = Target::Robot {
            robot: target.robot().to_owned(),
        };
        let interaction = self
            .interactions
            .get(&robot)
            .into_iter()
            .chain(self.interactions.get(target))
            .copied()
            .max()
            .unwrap_or_default();
        let condition = self
            .conditions
            .get(&robot)
            .into_iter()
            .chain(self.conditions.get(target))
            .copied()
            .max()
            .unwrap_or_default();
        (interaction, condition)
    }
}

trait FiniteVector {
    fn finite_f32(self) -> Option<[f32; 3]>;
}

impl FiniteVector for [f64; 3] {
    fn finite_f32(self) -> Option<[f32; 3]> {
        let converted = self.map(|value| value as f32);
        converted
            .into_iter()
            .all(f32::is_finite)
            .then_some(converted)
    }
}

/// Why stable scene configuration was rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    EmptyRobotId,
    DuplicateRobot(String),
    InvalidAccent(String),
    UnknownRobot(String),
    EmptyCameraRole {
        robot: String,
    },
    DuplicateCamera {
        robot: String,
        role: String,
    },
    UnknownCameraFrame {
        robot: String,
        role: String,
        frame: String,
    },
    InvalidCameraTransform {
        robot: String,
        role: String,
    },
    InvalidFieldOfView {
        robot: String,
        role: String,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRobotId => f.write_str("a viewport robot must have an id"),
            Self::DuplicateRobot(robot) => write!(f, "viewport robot {robot:?} appears twice"),
            Self::InvalidAccent(robot) => {
                write!(f, "viewport robot {robot:?} has an invalid accent colour")
            }
            Self::UnknownRobot(robot) => write!(f, "viewport robot {robot:?} was not added"),
            Self::EmptyCameraRole { robot } => {
                write!(f, "a camera on viewport robot {robot:?} has no role")
            }
            Self::DuplicateCamera { robot, role } => {
                write!(f, "viewport camera {robot:?}/{role:?} appears twice")
            }
            Self::UnknownCameraFrame { robot, role, frame } => write!(
                f,
                "viewport camera {robot:?}/{role:?} names unknown frame {frame:?}"
            ),
            Self::InvalidCameraTransform { robot, role } => write!(
                f,
                "viewport camera {robot:?}/{role:?} has a non-finite transform"
            ),
            Self::InvalidFieldOfView { robot, role } => write!(
                f,
                "viewport camera {robot:?}/{role:?} needs a field of view between 0 and 180 degrees"
            ),
        }
    }
}

impl std::error::Error for FrameError {}

/// A scene and the table that says what is in it.
#[derive(Default)]
pub(crate) struct RobotScene {
    scene: Scene,
    /// Indexed by [`EntityKey`], so a pick is one lookup.
    entities: Vec<Target>,
    robot_origins: BTreeMap<String, Vec3>,
}

impl RobotScene {
    /// What a picked key refers to.
    pub(crate) fn target(&self, key: EntityKey) -> Option<&Target> {
        self.entities.get(key.index())
    }

    pub(crate) fn scene(&self) -> &Scene {
        &self.scene
    }

    pub(crate) fn key(&self, target: &Target) -> Option<EntityKey> {
        self.entities
            .iter()
            .position(|candidate| candidate == target)
            .map(EntityKey::from_index)
    }

    pub(crate) fn robot_origin(&self, robot: &str) -> Option<Vec3> {
        self.robot_origins.get(robot).copied()
    }
}

impl RobotScene {
    pub(crate) fn compose(urdf: &Urdf, frame: &Frame) -> Self {
        let mut entity_catalog = EntityCatalog::default();
        let mut scene = Scene::new(frame.grid_enabled);
        let mut robot_origins = BTreeMap::new();

        for (placement_index, placement) in frame.placements.iter().enumerate() {
            let base_transform =
                ArmPlacement::base_transform(placement_index, frame.placements.len());
            robot_origins.insert(
                placement.id.clone(),
                base_transform.transform_point3(Vec3::ZERO),
            );
            let joint_positions = urdf.so101_joint_positions(&placement.pose.angles);
            let link_transforms = urdf.pose(&joint_positions);

            for (link_index, link) in urdf.links().iter().enumerate() {
                let target = match urdf.controlling_joint(link_index) {
                    Some(joint) => Target::Joint {
                        robot: placement.id.clone(),
                        joint,
                    },
                    None => Target::Robot {
                        robot: placement.id.clone(),
                    },
                };
                let key = entity_catalog.key_for(target);
                let state = frame.state_for(entity_catalog.target(key));

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
                let target = Target::Camera {
                    robot: placement.id.clone(),
                    role: camera_mount.role.clone(),
                };
                let key = entity_catalog.key_for(target);
                let state = frame.state_for(entity_catalog.target(key));
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
            robot_origins,
        }
    }
}

/// Give `semantic` a key, reusing the one it already has.
///
/// Every shell of one joint therefore shares a key, which is what makes picking
/// any of them select the joint without the renderer knowing what a joint is.
#[derive(Default)]
struct EntityCatalog {
    entities: Vec<Target>,
}

impl EntityCatalog {
    fn key_for(&mut self, target: Target) -> EntityKey {
        match self
            .entities
            .iter()
            .position(|existing_target| *existing_target == target)
        {
            Some(index) => EntityKey::from_index(index),
            None => {
                self.entities.push(target);
                EntityKey::from_index(self.entities.len() - 1)
            }
        }
    }

    fn target(&self, key: EntityKey) -> &Target {
        &self.entities[key.index()]
    }
}

impl Urdf {
    /// Place this description's joints from measured SO-101 angles.
    fn so101_joint_positions(&self, angles: &ArmPose) -> BTreeMap<String, f32> {
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
    pub(crate) fn so101_tool_position(&self, angles: &ArmPose) -> Option<Vec3> {
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
    use crate::SO101_JOINTS;

    use super::*;
    use crate::urdf::SO101_URDF;

    #[test]
    fn every_shell_of_one_joint_shares_its_key() {
        let mut entity_catalog = EntityCatalog::default();
        let joint = |name: &str| Target::Joint {
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
        let alone = ArmPlacement::base_transform(0, 1).transform_point3(Vec3::ZERO);
        assert_eq!(alone, Vec3::ZERO);

        let left = ArmPlacement::base_transform(0, 2).transform_point3(Vec3::ZERO);
        let right = ArmPlacement::base_transform(1, 2).transform_point3(Vec3::ZERO);
        assert!(
            (left.y + right.y).abs() < 1e-6,
            "a pair straddles the origin"
        );
        assert!((right.y - left.y - BENCH_SPACING).abs() < 1e-6);
    }

    #[test]
    fn frame_rejects_malformed_stable_configuration() {
        let mut frame = Frame::new(true);
        assert_eq!(
            frame
                .add_arm("", ArmPresentation::new([0.2, 0.3, 0.4, 1.0], None))
                .unwrap_err(),
            FrameError::EmptyRobotId
        );
        frame
            .add_arm("follower", ArmPresentation::new([0.2, 0.3, 0.4, 1.0], None))
            .unwrap();
        assert_eq!(
            frame
                .add_arm("follower", ArmPresentation::new([0.2, 0.3, 0.4, 1.0], None),)
                .unwrap_err(),
            FrameError::DuplicateRobot("follower".to_owned())
        );
        assert!(matches!(
            frame.mount_camera(
                "follower",
                CameraMount::new(
                    "wrist",
                    CameraAttachment::new(Some("not_a_link"), [0.0; 3], [0.0; 3]),
                    62.0,
                ),
            ),
            Err(FrameError::UnknownCameraFrame { .. })
        ));
        assert!(matches!(
            frame.mount_camera(
                "follower",
                CameraMount::new(
                    "wrist",
                    CameraAttachment::new(None::<String>, [f64::NAN, 0.0, 0.0], [0.0; 3],),
                    62.0,
                ),
            ),
            Err(FrameError::InvalidCameraTransform { .. })
        ));
        assert!(matches!(
            frame.mount_camera(
                "follower",
                CameraMount::new(
                    "wrist",
                    CameraAttachment::new(None::<String>, [0.0; 3], [0.0; 3]),
                    180.0,
                ),
            ),
            Err(FrameError::InvalidFieldOfView { .. })
        ));
    }

    #[test]
    fn frame_only_records_appearance_for_targets_it_contains() {
        let mut frame = Frame::new(false);
        frame
            .add_arm("follower", ArmPresentation::new([0.2, 0.3, 0.4, 1.0], None))
            .unwrap();
        let robot = Target::Robot {
            robot: "follower".to_owned(),
        };
        let elbow = Target::Joint {
            robot: "follower".to_owned(),
            joint: "elbow_flex".to_owned(),
        };
        let unknown = Target::Joint {
            robot: "follower".to_owned(),
            joint: "made_up".to_owned(),
        };

        assert!(frame.set_interaction(robot, Interaction::Selected));
        assert!(frame.set_interaction(elbow.clone(), Interaction::Hovered));
        assert!(frame.set_condition(elbow.clone(), Condition::Fault));
        assert!(!frame.set_condition(unknown, Condition::Fault));
        assert_eq!(
            frame.state_for(&elbow),
            (Interaction::Selected, Condition::Fault),
            "whole-arm selection scopes over the exact hovered joint"
        );
    }

    #[test]
    fn every_composed_shape_resolves_to_a_semantic_target() {
        let mut frame = Frame::new(false);
        frame
            .add_arm(
                "follower",
                ArmPresentation::new([0.2, 0.3, 0.4, 1.0], Some(ArmPose::default())),
            )
            .unwrap();
        let scene = RobotScene::compose(&Urdf::so101(), &frame);

        assert!(scene.scene().entities().next().is_some());
        for entity in scene.scene().entities() {
            let target = scene
                .target(entity.key())
                .expect("every drawn shape must have a target");
            assert_eq!(scene.key(target), Some(entity.key()));
            assert!(scene.scene().anchor_point(entity.key()).is_some());
        }
        assert_eq!(scene.robot_origin("follower"), Some(Vec3::ZERO));
    }

    /// Where each pivot of the shipped description lands for `angles`.
    fn pivots(angles: &ArmPose) -> BTreeMap<&'static str, Vec3> {
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
        let at = pivots(&ArmPose::default());

        assert!((at["wrist_link"].z - at["forearm_link"].z).abs() < 1e-6);
        assert!(at["forearm_link"].x > at["upper_arm_link"].x);
        assert!(at["gripper_link"].x > at["wrist_link"].x);
        assert!(at["gripper_link"].z < at["wrist_link"].z);
    }

    #[test]
    fn lifting_the_shoulder_raises_the_tool() {
        let lowered = pivots(&ArmPose::default().with_lift_degrees(20.0));
        let raised = pivots(&ArmPose::default().with_lift_degrees(-20.0));

        assert!(raised["forearm_link"].z > lowered["forearm_link"].z);
    }

    #[test]
    fn panning_swings_the_arm_about_the_vertical_axis() {
        let ahead = pivots(&ArmPose::default());
        let turned = pivots(&ArmPose::default().with_pan_degrees(90.0));

        assert!(turned["gripper_link"].x.abs() < 1e-6);
        assert!((turned["gripper_link"].y - ahead["gripper_link"].x).abs() < 1e-6);
        assert!((turned["gripper_link"].z - ahead["gripper_link"].z).abs() < 1e-6);
    }

    #[test]
    fn link_lengths_come_from_the_description_for_every_pose() {
        let urdf = Urdf::parse(SO101_URDF).unwrap();
        let reference = pivots(&ArmPose::default());
        let bent = pivots(
            &ArmPose::default()
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
                &ArmPose::default().with_gripper_normalized(1.0 - 2.0 * gripper_opening),
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
        let positions = urdf.so101_joint_positions(&ArmPose::default());

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
