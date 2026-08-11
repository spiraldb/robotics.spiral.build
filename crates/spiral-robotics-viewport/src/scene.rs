//! What the bench is made of, with no idea what any of it is.
//!
//! An [`Entity`] is a shape somewhere in the world that the caller has given a
//! number to. This module never learns what the number means. That is the whole
//! point: picking can return an entity, hover can light one up, and a condition
//! can tint one, all without this crate holding an opinion about joints, arms,
//! or cameras — and so without the application's model of those things leaking
//! into the renderer one field at a time.

use spiral_robotics_core::glam::{Mat4, Vec3};
use spiral_robotics_core::urdf::Geometry;

/// A caller-assigned handle for one addressable thing in the scene.
///
/// Opaque on purpose. Several entities may share a key — every shell of one
/// joint does — and picking any of them returns that one key, so grouping falls
/// out of the numbering rather than out of a rule written in here.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntityKey(pub u32);

/// What the operator is doing with an entity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Interaction {
    #[default]
    Resting,
    Hovered,
    Selected,
}

/// What the hardware behind an entity is reporting about itself.
///
/// Supplied by the caller and rendered here. This crate does not decide what
/// counts as caution; it only knows how to show that something does.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Condition {
    #[default]
    Nominal,
    Caution,
    Fault,
}

/// One shape to draw, already placed in the world.
#[derive(Clone, Debug)]
pub struct Entity {
    pub key: EntityKey,
    pub geometry: Geometry,
    /// Where the shape's own frame sits in the world.
    pub transform: Mat4,
    pub color: [f32; 4],
    pub interaction: Interaction,
    pub condition: Condition,
    /// False for scenery: drawn, never picked, never lit by a hover.
    pub pickable: bool,
    /// Drawn as lines rather than a lit solid.
    ///
    /// A camera's field of view is a volume, not an object; drawing it shaded
    /// would put a solid grey wedge through the middle of the bench.
    pub wireframe: bool,
}

impl Entity {
    /// The entity's origin in world space, which is where a label anchors.
    pub fn origin(&self) -> Vec3 {
        self.transform.transform_point3(Vec3::ZERO)
    }
}

/// Everything to draw this frame.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub entities: Vec<Entity>,
    pub grid: bool,
}

impl Scene {
    /// The first entity carrying `key`, which is where its label anchors.
    pub fn entity(&self, key: EntityKey) -> Option<&Entity> {
        self.entities.iter().find(|entity| entity.key == key)
    }

    /// Where a key's label belongs, in world space.
    ///
    /// The midpoint of everything sharing the key rather than the first shape's
    /// origin: a joint is several shells, and anchoring to whichever the
    /// description happened to list first would put the label on the edge of
    /// the part instead of on the part.
    pub fn anchor_point(&self, key: EntityKey) -> Option<Vec3> {
        let mut sum = Vec3::ZERO;
        let mut count = 0.0;
        for entity in self.entities.iter().filter(|entity| entity.key == key) {
            sum += entity.origin();
            count += 1.0;
        }
        (count > 0.0).then(|| sum / count)
    }
}
