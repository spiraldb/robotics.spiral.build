//! Reading a robot description and posing it.
//!
//! The studio draws whatever this module parses, so the picture of the robot
//! comes from a description file rather than from code. Only the file shipped
//! today is SO-101 specific: everything here works on any URDF built from
//! primitive geometry, and teaching the studio a second arm means adding a
//! second file, not a second renderer.
//!
//! Only the subset a viewer needs is understood — the kinematic tree, link
//! visuals, and joint limits. Inertias, collision volumes, transmissions, and
//! Gazebo extensions are skipped rather than rejected, so a full-fidelity URDF
//! written for a simulator still loads and still draws.
//!
//! This is deliberately a tolerant visualization model, not a source of motor
//! limits or motion authorization. Missing joint positions rest at zero and
//! unusable visual dimensions collapse to zero so incomplete telemetry or
//! decoration cannot take down the scene; hardware policy must use validated
//! motor configuration and calibration instead.

use std::collections::BTreeMap;
use std::fmt;

use glam::{Mat4, Quat, Vec3};
use roxmltree::{Document, Node};

/// The SO-ARM101 description the studio ships with.
pub(crate) const SO101_URDF: &str = include_str!("../assets/so101.urdf");

/// A rigid transform, as written in a URDF `<origin>` element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pose {
    translation: Vec3,
    rotation: Quat,
}

impl Pose {
    pub(crate) const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
    };

    /// The equivalent homogeneous matrix.
    pub(crate) fn matrix(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rotation, self.translation)
    }

    #[cfg(test)]
    pub(crate) fn translation(&self) -> Vec3 {
        self.translation
    }
}

impl Default for Pose {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// A shape attached to a link.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Geometry {
    /// Full extents along each axis, centered on the visual's origin.
    Box {
        size: Vec3,
    },
    /// Centered on the visual's origin, extending along its local Z.
    Cylinder {
        radius: f32,
        length: f32,
    },
    Sphere {
        radius: f32,
    },
    /// Kept so a mesh-based description still parses; nothing draws it.
    Mesh,
}

/// One drawable shape on a link.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Visual {
    /// Placement relative to the link's own frame.
    origin: Pose,
    geometry: Geometry,
    /// The material's name, kept so a renderer can substitute its own colour.
    material: Option<String>,
    /// Resolved colour, from the visual itself or the named material.
    color: [f32; 4],
}

impl Visual {
    pub(crate) fn origin(&self) -> Pose {
        self.origin
    }

    pub(crate) fn geometry(&self) -> &Geometry {
        &self.geometry
    }

    pub(crate) fn material(&self) -> Option<&str> {
        self.material.as_deref()
    }

    pub(crate) fn color(&self) -> [f32; 4] {
        self.color
    }
}

/// A rigid body in the kinematic tree.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Link {
    name: String,
    visuals: Vec<Visual>,
}

impl Link {
    pub(crate) fn visuals(&self) -> &[Visual] {
        &self.visuals
    }
}

/// How a joint moves its child link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JointKind {
    /// Turns about its axis, between limits.
    Revolute,
    /// Turns about its axis without limits.
    Continuous,
    /// Slides along its axis.
    Prismatic,
    /// Does not move. Also covers `floating` and `planar`, which a viewer
    /// driven by named joint positions has no way to place.
    Fixed,
}

impl JointKind {
    /// True when a position value moves this joint.
    pub(crate) fn is_movable(self) -> bool {
        !matches!(self, Self::Fixed)
    }
}

/// A connection between two links.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Joint {
    name: String,
    kind: JointKind,
    /// Index into [`Urdf::links`].
    parent: usize,
    /// Index into [`Urdf::links`].
    child: usize,
    /// Placement of the child's frame in the parent's, before any motion.
    origin: Pose,
    axis: Vec3,
    /// Viewer travel limits in radians or metres, when the description states them.
    /// These are drawing bounds, never hardware command limits.
    limit: Option<(f32, f32)>,
}

impl Joint {
    /// Clamp `position` to this joint's limits, if it has any.
    fn clamp(&self, position: f32) -> f32 {
        match self.limit {
            Some((lower, upper)) if self.kind != JointKind::Continuous => {
                position.clamp(lower, upper)
            }
            _ => position,
        }
    }

    /// The transform this joint contributes at `position`.
    fn motion(&self, position: f32) -> Mat4 {
        match self.kind {
            JointKind::Revolute | JointKind::Continuous => {
                Mat4::from_axis_angle(self.axis, position)
            }
            JointKind::Prismatic => Mat4::from_translation(self.axis * position),
            JointKind::Fixed => Mat4::IDENTITY,
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn kind(&self) -> JointKind {
        self.kind
    }

    pub(crate) fn parent(&self) -> usize {
        self.parent
    }

    pub(crate) fn child(&self) -> usize {
        self.child
    }

    #[cfg(test)]
    pub(crate) fn origin(&self) -> Pose {
        self.origin
    }

    pub(crate) fn limit(&self) -> Option<(f32, f32)> {
        self.limit
    }
}

/// A parsed robot description.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Urdf {
    name: String,
    links: Vec<Link>,
    joints: Vec<Joint>,
    /// Index of the only link no joint has as a child.
    root: usize,
    /// Joint indices leaving each link, so posing the tree is one walk down.
    children: Vec<Vec<usize>>,
}

impl Urdf {
    /// The validated description shipped with the crate.
    ///
    /// Its validity is a crate invariant covered by tests, not a runtime choice
    /// callers need to handle.
    pub(crate) fn so101() -> Self {
        Self::parse(SO101_URDF).expect("the viewport's bundled SO-101 description must be valid")
    }

    /// Parse a URDF document.
    pub(crate) fn parse(text: &str) -> Result<Self, UrdfError> {
        UrdfParser::parse(text)
    }

    /// The link every other link hangs from.
    pub(crate) fn root(&self) -> usize {
        self.root
    }

    pub(crate) fn link_index(&self, name: &str) -> Option<usize> {
        self.links.iter().position(|link| link.name == name)
    }

    pub(crate) fn joint(&self, name: &str) -> Option<&Joint> {
        self.joints.iter().find(|joint| joint.name == name)
    }

    /// The joint connecting two links, named rather than indexed.
    #[cfg(test)]
    pub(crate) fn joint_between(&self, parent: &str, child: &str) -> Option<&Joint> {
        let (parent, child) = (self.link_index(parent)?, self.link_index(child)?);
        self.joints
            .iter()
            .find(|joint| joint.parent == parent && joint.child == child)
    }

    /// Every joint a caller can supply a position for.
    #[cfg(test)]
    fn movable_joints(&self) -> impl Iterator<Item = &Joint> {
        self.joints.iter().filter(|joint| joint.kind.is_movable())
    }

    /// Place every link, given the joint positions to pose it at.
    ///
    /// The result is indexed like [`Urdf::links`]. Joints missing from
    /// `positions` stay at zero, so a description with more joints than the
    /// caller knows about still poses — the unknown ones simply do not move.
    /// Positions outside a joint's stated viewer limits are clamped, because a
    /// mismeasured angle should bend the drawing no further than the hardware
    /// could bend.
    pub(crate) fn pose(&self, positions: &BTreeMap<String, f32>) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.links.len()];
        let mut stack = vec![self.root];
        while let Some(link) = stack.pop() {
            for &index in &self.children[link] {
                let joint = &self.joints[index];
                let position = positions
                    .get(&joint.name)
                    .copied()
                    .filter(|value| value.is_finite())
                    .unwrap_or(0.0);
                world[joint.child] =
                    world[link] * joint.origin.matrix() * joint.motion(joint.clamp(position));
                stack.push(joint.child);
            }
        }
        world
    }

    /// Every visual in the description, paired with its world transform.
    ///
    /// `transforms` comes from [`Urdf::pose`]; a shorter slice simply yields
    /// fewer visuals rather than panicking, so a stale pose cannot crash a
    /// renderer mid-frame.
    #[cfg(test)]
    fn visuals<'a>(&'a self, transforms: &'a [Mat4]) -> impl Iterator<Item = (&'a Visual, Mat4)> {
        self.links
            .iter()
            .zip(transforms)
            .flat_map(|(link, &transform)| {
                link.visuals
                    .iter()
                    .map(move |visual| (visual, transform * visual.origin.matrix()))
            })
    }

    pub(crate) fn links(&self) -> &[Link] {
        &self.links
    }

    pub(crate) fn joints(&self) -> &[Joint] {
        &self.joints
    }
}

/// Why a description could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UrdfError {
    Xml(String),
    NotRobot(String),
    MissingAttribute {
        element: String,
        attribute: String,
    },
    BadNumbers {
        element: String,
        value: String,
    },
    UnknownJointType {
        joint: String,
        kind: String,
    },
    UnknownGeometry {
        link: String,
    },
    UnknownLink {
        joint: String,
        link: String,
    },
    DuplicateLink(String),
    /// A link is the child of more than one joint, so it has no single place.
    RepeatedChild {
        link: String,
        joint: String,
    },
    /// Every link is some joint's child, so the tree has no starting point.
    NoRoot,
    /// More than one link is nobody's child, so the description is not a tree.
    MultipleRoots(Vec<String>),
    /// Some links do not descend from the root.
    Detached(Vec<String>),
}

impl fmt::Display for UrdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => write!(f, "not valid XML: {error}"),
            Self::NotRobot(element) => {
                write!(f, "expected a <robot> document, found <{element}>")
            }
            Self::MissingAttribute { element, attribute } => {
                write!(f, "<{element}> is missing its {attribute} attribute")
            }
            Self::BadNumbers { element, value } => {
                write!(f, "<{element}> has an unreadable number: {value:?}")
            }
            Self::UnknownJointType { joint, kind } => {
                write!(f, "joint {joint} has an unknown type {kind:?}")
            }
            Self::UnknownGeometry { link } => {
                write!(f, "a visual on link {link} has no shape this build draws")
            }
            Self::UnknownLink { joint, link } => {
                write!(f, "joint {joint} names a link that does not exist: {link}")
            }
            Self::DuplicateLink(name) => write!(f, "two links are named {name}"),
            Self::RepeatedChild { link, joint } => {
                write!(
                    f,
                    "link {link} already has a parent joint, so {joint} cannot also move it"
                )
            }
            Self::NoRoot => write!(f, "every link has a parent, so there is no root link"),
            Self::MultipleRoots(names) => {
                write!(f, "several links have no parent: {}", names.join(", "))
            }
            Self::Detached(names) => {
                write!(
                    f,
                    "these links are not connected to the root: {}",
                    names.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for UrdfError {}

struct UrdfParser;

impl UrdfParser {
    fn parse(text: &str) -> Result<Urdf, UrdfError> {
        let document = Document::parse(text).map_err(|error| UrdfError::Xml(error.to_string()))?;
        let robot = document.root_element();
        if robot.tag_name().name() != "robot" {
            return Err(UrdfError::NotRobot(robot.tag_name().name().to_owned()));
        }

        // Materials declared at robot level are referenced by name from visuals
        // further down, so they are collected before any link is read.
        let mut palette = BTreeMap::new();
        for node in Self::children(robot, "material") {
            if let (Some(name), Some(color)) = (node.attribute("name"), Self::color_of(node)?) {
                palette.insert(name.to_owned(), color);
            }
        }

        let mut links = Vec::new();
        for node in Self::children(robot, "link") {
            let name = Self::required(node, "name")?.to_owned();
            if links.iter().any(|link: &Link| link.name == name) {
                return Err(UrdfError::DuplicateLink(name));
            }
            let mut visuals = Vec::new();
            for visual in Self::children(node, "visual") {
                visuals.push(Self::parse_visual(visual, &name, &palette)?);
            }
            links.push(Link { name, visuals });
        }

        let index_of = |joint: &str, link: &str| {
            links
                .iter()
                .position(|candidate| candidate.name == link)
                .ok_or_else(|| UrdfError::UnknownLink {
                    joint: joint.to_owned(),
                    link: link.to_owned(),
                })
        };

        let mut joints = Vec::new();
        for node in Self::children(robot, "joint") {
            let name = Self::required(node, "name")?.to_owned();
            let kind = match Self::required(node, "type")? {
                "revolute" => JointKind::Revolute,
                "continuous" => JointKind::Continuous,
                "prismatic" => JointKind::Prismatic,
                "fixed" | "floating" | "planar" => JointKind::Fixed,
                other => {
                    return Err(UrdfError::UnknownJointType {
                        joint: name,
                        kind: other.to_owned(),
                    });
                }
            };
            let parent = index_of(&name, Self::required(Self::child(node, "parent")?, "link")?)?;
            let child_link = index_of(&name, Self::required(Self::child(node, "child")?, "link")?)?;
            // A zero axis would make `Mat4::from_axis_angle` produce garbage, and a
            // description that omits the axis means the URDF default of +X.
            let axis = match Self::children(node, "axis").next() {
                Some(node) => Self::vec3(node, "xyz")?.normalize_or(Vec3::X),
                None => Vec3::X,
            };
            // Missing endpoints collapse this viewer-only joint to rest. Hardware
            // command limits come from validated motor configuration, not URDF.
            let limit = Self::children(node, "limit")
                .next()
                .map(|node| {
                    Ok::<_, UrdfError>((Self::number(node, "lower")?, Self::number(node, "upper")?))
                })
                .transpose()?
                .map(|(lower, upper)| (lower.unwrap_or(0.0), upper.unwrap_or(0.0)));

            joints.push(Joint {
                name,
                kind,
                parent,
                child: child_link,
                origin: Self::origin_of(node)?,
                axis,
                limit,
            });
        }

        let name = robot.attribute("name").unwrap_or("robot").to_owned();
        Urdf::assemble(name, links, joints)
    }

    /// Turn a flat list of links and joints into a tree, or say why it is not one.
    fn parse_visual(
        node: Node<'_, '_>,
        link: &str,
        palette: &BTreeMap<String, [f32; 4]>,
    ) -> Result<Visual, UrdfError> {
        let shape = Self::child(node, "geometry").map_err(|_| UrdfError::UnknownGeometry {
            link: link.to_owned(),
        })?;
        let geometry = Self::parse_geometry(shape, link)?;

        // A visual may name a material, restate one inline, or do both; an inline
        // colour is the more specific statement and wins.
        let material = Self::children(node, "material").next();
        let named = material.and_then(|node| node.attribute("name"));
        let color = match material.map(Self::color_of).transpose()?.flatten() {
            Some(color) => color,
            None => named
                .and_then(|name| palette.get(name).copied())
                .unwrap_or(DEFAULT_COLOR),
        };

        Ok(Visual {
            origin: Self::origin_of(node)?,
            geometry,
            material: named.map(str::to_owned),
            color,
        })
    }

    fn parse_geometry(node: Node<'_, '_>, link: &str) -> Result<Geometry, UrdfError> {
        // A missing visual dimension becomes zero so optional decoration cannot
        // break the scene. This tolerance is intentionally not configuration policy.
        for shape in node.children().filter(Node::is_element) {
            return match shape.tag_name().name() {
                "box" => Ok(Geometry::Box {
                    size: Self::vec3(shape, "size")?,
                }),
                "cylinder" => Ok(Geometry::Cylinder {
                    radius: Self::number(shape, "radius")?.unwrap_or(0.0),
                    length: Self::number(shape, "length")?.unwrap_or(0.0),
                }),
                "sphere" => Ok(Geometry::Sphere {
                    radius: Self::number(shape, "radius")?.unwrap_or(0.0),
                }),
                "mesh" => {
                    Self::required(shape, "filename")?;
                    if shape.attribute("scale").is_some() {
                        Self::vec3(shape, "scale")?;
                    }
                    Ok(Geometry::Mesh)
                }
                _ => continue,
            };
        }
        Err(UrdfError::UnknownGeometry {
            link: link.to_owned(),
        })
    }

    /// The `<origin>` of an element, defaulting to no offset and no rotation.
    fn origin_of(node: Node<'_, '_>) -> Result<Pose, UrdfError> {
        let Some(origin) = Self::children(node, "origin").next() else {
            return Ok(Pose::IDENTITY);
        };
        let translation = match origin.attribute("xyz") {
            Some(_) => Self::vec3(origin, "xyz")?,
            None => Vec3::ZERO,
        };
        let angles = match origin.attribute("rpy") {
            Some(_) => Self::vec3(origin, "rpy")?,
            None => Vec3::ZERO,
        };
        Ok(Pose {
            translation,
            // URDF states fixed-axis roll, pitch, and yaw, which compose as
            // Rz(yaw) * Ry(pitch) * Rx(roll).
            rotation: Quat::from_rotation_z(angles.z)
                * Quat::from_rotation_y(angles.y)
                * Quat::from_rotation_x(angles.x),
        })
    }

    fn color_of(node: Node<'_, '_>) -> Result<Option<[f32; 4]>, UrdfError> {
        let Some(color) = Self::children(node, "color").next() else {
            return Ok(None);
        };
        let Some(text) = color.attribute("rgba") else {
            return Ok(None);
        };
        let parts = Self::numbers(text).ok_or_else(|| UrdfError::BadNumbers {
            element: "color".to_owned(),
            value: text.to_owned(),
        })?;
        match parts.as_slice() {
            [r, g, b, a] => Ok(Some([*r, *g, *b, *a])),
            [r, g, b] => Ok(Some([*r, *g, *b, 1.0])),
            _ => Err(UrdfError::BadNumbers {
                element: "color".to_owned(),
                value: text.to_owned(),
            }),
        }
    }

    fn children<'a>(node: Node<'a, 'a>, name: &'a str) -> impl Iterator<Item = Node<'a, 'a>> {
        node.children()
            .filter(move |child| child.is_element() && child.tag_name().name() == name)
    }

    fn child<'a>(node: Node<'a, 'a>, name: &'a str) -> Result<Node<'a, 'a>, UrdfError> {
        Self::children(node, name)
            .next()
            .ok_or_else(|| UrdfError::MissingAttribute {
                element: node.tag_name().name().to_owned(),
                attribute: format!("<{name}> child"),
            })
    }

    fn required<'a>(node: Node<'a, 'a>, attribute: &str) -> Result<&'a str, UrdfError> {
        node.attribute(attribute)
            .ok_or_else(|| UrdfError::MissingAttribute {
                element: node.tag_name().name().to_owned(),
                attribute: attribute.to_owned(),
            })
    }

    fn vec3(node: Node<'_, '_>, attribute: &str) -> Result<Vec3, UrdfError> {
        let text = Self::required(node, attribute)?;
        match Self::numbers(text).as_deref() {
            Some([x, y, z]) => Ok(Vec3::new(*x, *y, *z)),
            _ => Err(UrdfError::BadNumbers {
                element: node.tag_name().name().to_owned(),
                value: text.to_owned(),
            }),
        }
    }

    fn number(node: Node<'_, '_>, attribute: &str) -> Result<Option<f32>, UrdfError> {
        let Some(text) = node.attribute(attribute) else {
            return Ok(None);
        };
        text.trim()
            .parse()
            .map(Some)
            .map_err(|_| UrdfError::BadNumbers {
                element: node.tag_name().name().to_owned(),
                value: text.to_owned(),
            })
    }

    /// Split a whitespace-separated list of numbers, or nothing if any is unreadable.
    fn numbers(text: &str) -> Option<Vec<f32>> {
        text.split_whitespace()
            .map(|part| part.parse().ok())
            .collect()
    }
}

impl Urdf {
    fn assemble(name: String, links: Vec<Link>, joints: Vec<Joint>) -> Result<Self, UrdfError> {
        let mut parent_joint = vec![None; links.len()];
        let mut children = vec![Vec::new(); links.len()];
        for (index, joint) in joints.iter().enumerate() {
            if parent_joint[joint.child].is_some() {
                return Err(UrdfError::RepeatedChild {
                    link: links[joint.child].name.clone(),
                    joint: joint.name.clone(),
                });
            }
            parent_joint[joint.child] = Some(index);
            children[joint.parent].push(index);
        }

        let roots = (0..links.len())
            .filter(|&index| parent_joint[index].is_none())
            .collect::<Vec<_>>();
        let root = match roots.as_slice() {
            [] => return Err(UrdfError::NoRoot),
            [only] => *only,
            many => {
                return Err(UrdfError::MultipleRoots(
                    many.iter()
                        .map(|&index| links[index].name.clone())
                        .collect(),
                ));
            }
        };

        // One root and one parent apiece still allows a separate cycle off to the
        // side, which would silently never be drawn. Walking the tree proves every
        // link is actually reachable.
        let mut reached = vec![false; links.len()];
        let mut stack = vec![root];
        reached[root] = true;
        while let Some(link) = stack.pop() {
            for &index in &children[link] {
                let child = joints[index].child;
                if !reached[child] {
                    reached[child] = true;
                    stack.push(child);
                }
            }
        }
        let detached = links
            .iter()
            .zip(&reached)
            .filter(|&(_, &seen)| !seen)
            .map(|(link, _)| link.name.clone())
            .collect::<Vec<_>>();
        if !detached.is_empty() {
            return Err(UrdfError::Detached(detached));
        }

        Ok(Self {
            name,
            links,
            joints,
            root,
            children,
        })
    }
}

/// What a visual is drawn in when the description never says.
const DEFAULT_COLOR: [f32; 4] = [0.7, 0.72, 0.75, 1.0];

#[cfg(test)]
mod tests {
    use crate::SO101_JOINTS;

    use super::*;

    type ErrorCase = (&'static str, fn(&UrdfError) -> bool);

    /// Two links on one revolute joint, enough to test the tree and the walk.
    const HINGE: &str = r#"
        <robot name="hinge">
          <material name="paint"><color rgba="0.1 0.2 0.3 1"/></material>
          <link name="base">
            <visual>
              <origin xyz="0 0 0.5"/>
              <geometry><box size="1 1 1"/></geometry>
              <material name="paint"/>
            </visual>
          </link>
          <link name="arm">
            <visual>
              <geometry><cylinder radius="0.1" length="2"/></geometry>
            </visual>
          </link>
          <joint name="hinge" type="revolute">
            <parent link="base"/>
            <child link="arm"/>
            <origin xyz="0 0 1"/>
            <axis xyz="0 1 0"/>
            <limit lower="-2" upper="2"/>
          </joint>
        </robot>
    "#;

    fn close(left: Vec3, right: Vec3) -> bool {
        (left - right).length() < 1e-5
    }

    #[test]
    fn a_description_parses_into_a_tree() {
        let urdf = Urdf::parse(HINGE).unwrap();

        assert_eq!(urdf.name, "hinge");
        assert_eq!(urdf.links.len(), 2);
        assert_eq!(urdf.root(), urdf.link_index("base").unwrap());
        assert_eq!(urdf.movable_joints().count(), 1);

        let joint = urdf.joint("hinge").unwrap();
        assert_eq!(joint.kind, JointKind::Revolute);
        assert_eq!(joint.axis, Vec3::Y);
        assert_eq!(joint.limit, Some((-2.0, 2.0)));
        assert_eq!(joint.origin.translation, Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn visuals_resolve_named_materials_and_keep_the_name() {
        let urdf = Urdf::parse(HINGE).unwrap();
        let painted = &urdf.links[urdf.link_index("base").unwrap()].visuals[0];

        assert_eq!(painted.material.as_deref(), Some("paint"));
        assert_eq!(painted.color, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(
            painted.geometry,
            Geometry::Box {
                size: Vec3::splat(1.0)
            }
        );

        // A visual with no material at all still draws, in the fallback colour.
        let bare = &urdf.links[urdf.link_index("arm").unwrap()].visuals[0];
        assert_eq!(bare.material, None);
        assert_eq!(bare.color, DEFAULT_COLOR);
    }

    #[test]
    fn posing_turns_the_child_about_the_joint_axis() {
        let urdf = Urdf::parse(HINGE).unwrap();
        let arm = urdf.link_index("arm").unwrap();

        // At rest the child sits exactly where its origin puts it.
        let rest = urdf.pose(&BTreeMap::new());
        assert!(close(
            rest[urdf.root()].transform_point3(Vec3::ZERO),
            Vec3::ZERO
        ));
        assert!(close(
            rest[arm].transform_point3(Vec3::ZERO),
            Vec3::new(0.0, 0.0, 1.0)
        ));

        // A quarter turn about +Y takes the child's +Z onto +X.
        let turned = urdf.pose(&BTreeMap::from([(
            "hinge".to_owned(),
            std::f32::consts::FRAC_PI_2,
        )]));
        assert!(close(
            turned[arm].transform_point3(Vec3::Z),
            Vec3::new(1.0, 0.0, 1.0)
        ));
    }

    #[test]
    fn positions_are_clamped_to_the_joint_limits() {
        let urdf = Urdf::parse(HINGE).unwrap();
        let arm = urdf.link_index("arm").unwrap();

        let past = urdf.pose(&BTreeMap::from([("hinge".to_owned(), 9.0)]));
        let at = urdf.pose(&BTreeMap::from([("hinge".to_owned(), 2.0)]));
        assert!(close(
            past[arm].transform_point3(Vec3::Z),
            at[arm].transform_point3(Vec3::Z)
        ));
    }

    #[test]
    fn unknown_and_unusable_positions_leave_a_joint_at_rest() {
        let urdf = Urdf::parse(HINGE).unwrap();
        let arm = urdf.link_index("arm").unwrap();
        let rest = urdf.pose(&BTreeMap::new())[arm];

        for positions in [
            BTreeMap::from([("some_other_joint".to_owned(), 1.0)]),
            BTreeMap::from([("hinge".to_owned(), f32::NAN)]),
        ] {
            assert_eq!(urdf.pose(&positions)[arm], rest);
        }
    }

    #[test]
    fn visuals_come_back_in_world_space() {
        let urdf = Urdf::parse(HINGE).unwrap();
        let transforms = urdf.pose(&BTreeMap::new());
        let placed = urdf.visuals(&transforms).collect::<Vec<_>>();

        assert_eq!(placed.len(), 2);
        // The base's own visual is offset half a metre up its link.
        assert!(close(
            placed[0].1.transform_point3(Vec3::ZERO),
            Vec3::new(0.0, 0.0, 0.5)
        ));
        // The arm's visual has no offset, so it lands on the joint.
        assert!(close(
            placed[1].1.transform_point3(Vec3::ZERO),
            Vec3::new(0.0, 0.0, 1.0)
        ));
    }

    #[test]
    fn a_stale_pose_yields_fewer_visuals_instead_of_panicking() {
        let urdf = Urdf::parse(HINGE).unwrap();
        assert_eq!(urdf.visuals(&[]).count(), 0);
        assert_eq!(urdf.visuals(&[Mat4::IDENTITY]).count(), 1);
    }

    #[test]
    fn roll_pitch_and_yaw_compose_the_way_urdf_says() {
        // A yaw of a quarter turn takes +X onto +Y.
        let yawed = Urdf::parse(
            r#"<robot name="r"><link name="a"><visual>
                 <origin xyz="0 0 0" rpy="0 0 1.5707963"/>
                 <geometry><sphere radius="1"/></geometry>
               </visual></link></robot>"#,
        )
        .unwrap();
        let origin = yawed.links[0].visuals[0].origin;
        assert!(close(origin.rotation * Vec3::X, Vec3::Y));

        // Roll then yaw: rolling +Z onto -Y, then yawing -Y onto +X.
        let both = Urdf::parse(
            r#"<robot name="r"><link name="a"><visual>
                 <origin rpy="1.5707963 0 1.5707963"/>
                 <geometry><sphere radius="1"/></geometry>
               </visual></link></robot>"#,
        )
        .unwrap();
        let origin = both.links[0].visuals[0].origin;
        assert!(close(origin.rotation * Vec3::Z, Vec3::X));
    }

    #[test]
    fn simulation_only_elements_are_skipped_rather_than_rejected() {
        let urdf = Urdf::parse(
            r#"<robot name="r">
                 <link name="a">
                   <inertial><mass value="2"/></inertial>
                   <collision><geometry><box size="1 1 1"/></geometry></collision>
                   <visual><geometry><sphere radius="0.5"/></geometry></visual>
                 </link>
                 <gazebo reference="a"><mu1>0.2</mu1></gazebo>
               </robot>"#,
        )
        .unwrap();

        assert_eq!(urdf.links.len(), 1);
        assert_eq!(urdf.links[0].visuals.len(), 1);
        assert_eq!(
            urdf.links[0].visuals[0].geometry,
            Geometry::Sphere { radius: 0.5 }
        );
    }

    #[test]
    fn a_mesh_parses_even_though_nothing_draws_it() {
        let urdf = Urdf::parse(
            r#"<robot name="r"><link name="a"><visual>
                 <geometry><mesh filename="arm.stl" scale="0.001 0.001 0.001"/></geometry>
               </visual></link></robot>"#,
        )
        .unwrap();

        assert_eq!(urdf.links[0].visuals[0].geometry, Geometry::Mesh);
    }

    #[test]
    fn descriptions_that_are_not_trees_are_refused() {
        let cases: [ErrorCase; 5] = [
            (
                r#"<robot name="r"><link name="a"/><link name="a"/></robot>"#,
                |error| matches!(error, UrdfError::DuplicateLink(name) if name == "a"),
            ),
            (
                r#"<robot name="r">
                     <link name="a"/><link name="b"/>
                     <joint name="j" type="fixed"><parent link="a"/><child link="b"/></joint>
                     <joint name="k" type="fixed"><parent link="a"/><child link="b"/></joint>
                   </robot>"#,
                |error| matches!(error, UrdfError::RepeatedChild { .. }),
            ),
            (
                r#"<robot name="r"><link name="a"/><link name="b"/></robot>"#,
                |error| matches!(error, UrdfError::MultipleRoots(names) if names.len() == 2),
            ),
            (
                r#"<robot name="r">
                     <link name="a"/>
                     <joint name="j" type="fixed"><parent link="a"/><child link="a"/></joint>
                   </robot>"#,
                |error| matches!(error, UrdfError::NoRoot),
            ),
            (
                r#"<robot name="r">
                     <link name="a"/><link name="b"/><link name="c"/>
                     <joint name="j" type="fixed"><parent link="c"/><child link="b"/></joint>
                     <joint name="k" type="fixed"><parent link="b"/><child link="c"/></joint>
                   </robot>"#,
                |error| matches!(error, UrdfError::Detached(names) if names.len() == 2),
            ),
        ];

        for (source, matches) in cases {
            let error = Urdf::parse(source).unwrap_err();
            assert!(matches(&error), "unexpected error for {source}: {error}");
        }
    }

    #[test]
    fn malformed_documents_report_what_is_wrong() {
        let cases: [ErrorCase; 5] = [
            ("<robot", |error| matches!(error, UrdfError::Xml(_))),
            (
                r#"<thing name="r"/>"#,
                |error| matches!(error, UrdfError::NotRobot(name) if name == "thing"),
            ),
            (
                r#"<robot name="r"><link/></robot>"#,
                |error| matches!(error, UrdfError::MissingAttribute { attribute, .. } if attribute == "name"),
            ),
            (
                r#"<robot name="r"><link name="a"><visual>
                     <geometry><box size="1 wide 1"/></geometry>
                   </visual></link></robot>"#,
                |error| matches!(error, UrdfError::BadNumbers { .. }),
            ),
            (
                r#"<robot name="r">
                     <link name="a"/><link name="b"/>
                     <joint name="j" type="wobbly"><parent link="a"/><child link="b"/></joint>
                   </robot>"#,
                |error| matches!(error, UrdfError::UnknownJointType { .. }),
            ),
        ];

        for (source, matches) in cases {
            let error = Urdf::parse(source).unwrap_err();
            assert!(matches(&error), "unexpected error for {source}: {error}");
            // Every error has to read as a sentence, since these reach the UI.
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn a_joint_with_no_axis_uses_the_urdf_default() {
        let urdf = Urdf::parse(
            r#"<robot name="r">
                 <link name="a"/><link name="b"/>
                 <joint name="j" type="revolute"><parent link="a"/><child link="b"/></joint>
               </robot>"#,
        )
        .unwrap();
        assert_eq!(urdf.joint("j").unwrap().axis, Vec3::X);
    }

    #[test]
    fn the_shipped_so101_description_is_the_arm_the_studio_drives() {
        let urdf = Urdf::parse(SO101_URDF).unwrap();

        // Every motor the studio knows about has to be a joint here, by name,
        // or a measured angle would have nothing to move.
        for joint in SO101_JOINTS {
            let found = urdf
                .joint(joint.name())
                .unwrap_or_else(|| panic!("{} is missing from the description", joint.name()));
            assert!(found.kind.is_movable(), "{} should move", joint.name());
            assert!(
                found.limit.is_some(),
                "{} should state its limits",
                joint.name()
            );
        }
        assert_eq!(urdf.movable_joints().count(), SO101_JOINTS.len());
        assert_eq!(urdf.links[urdf.root()].name, "base_link");

        // Nothing references a mesh, so the description draws with no assets.
        for link in &urdf.links {
            for visual in &link.visuals {
                assert!(
                    !matches!(visual.geometry, Geometry::Mesh),
                    "{} should not need a mesh file",
                    link.name
                );
            }
        }
    }

    #[test]
    fn the_shipped_arm_reaches_the_length_it_is_built_from() {
        let urdf = Urdf::parse(SO101_URDF).unwrap();
        let transforms = urdf.pose(&BTreeMap::new());
        let tip = urdf.link_index("gripper_link").unwrap();

        // Straight up at mechanical zero: base plate, shoulder, upper arm,
        // forearm, and the 30 mm to the roll axis.
        let expected = 0.018 + 0.037 + 0.117 + 0.135 + 0.030;
        let placed = transforms[tip].transform_point3(Vec3::ZERO);
        assert!(
            close(placed, Vec3::new(0.0, 0.0, expected)),
            "gripper sits at {placed:?}"
        );
    }
}
