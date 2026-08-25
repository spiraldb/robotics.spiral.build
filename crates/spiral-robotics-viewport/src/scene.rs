//! What the bench is made of, with no idea what any of it is.
//!
//! An [`Entity`] is a shape somewhere in the world that the caller has given a
//! number to. This module never learns what the number means. That is the whole
//! point: picking can return an entity, hover can light one up, and a condition
//! can tint one, all without this crate holding an opinion about joints, arms,
//! or cameras — and so without the application's model of those things leaking
//! into the renderer one field at a time.

use glam::{Mat4, Vec3};

use crate::urdf::Geometry;

/// A caller-assigned handle for one addressable thing in the scene.
///
/// Opaque on purpose. Several entities may share a key — every shell of one
/// joint does — and picking any of them returns that one key, so grouping falls
/// out of the numbering rather than out of a rule written in here.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntityKey(u32);

impl EntityKey {
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index as u32)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

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
    key: EntityKey,
    shape: EntityShape,
    state: EntityState,
}

#[derive(Clone, Copy, Debug)]
struct EntityState {
    interaction: Interaction,
    condition: Condition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntityRendering {
    Solid,
    Wireframe,
}

impl Entity {
    pub(crate) fn new(
        key: EntityKey,
        shape: EntityShape,
        (interaction, condition): (Interaction, Condition),
    ) -> Self {
        Self {
            key,
            shape,
            state: EntityState {
                interaction,
                condition,
            },
        }
    }

    pub fn key(&self) -> EntityKey {
        self.key
    }

    pub fn condition(&self) -> Condition {
        self.state.condition
    }

    /// The entity's origin in world space, which is where a label anchors.
    pub fn origin(&self) -> Vec3 {
        self.shape.transform.transform_point3(Vec3::ZERO)
    }

    pub(crate) fn geometry(&self) -> &Geometry {
        &self.shape.geometry
    }

    pub(crate) fn transform(&self) -> Mat4 {
        self.shape.transform
    }

    pub(crate) fn color(&self) -> [f32; 4] {
        self.shape.color
    }

    pub(crate) fn interaction(&self) -> Interaction {
        self.state.interaction
    }

    pub(crate) fn is_pickable(&self) -> bool {
        true
    }

    pub(crate) fn is_wireframe(&self) -> bool {
        self.shape.rendering == EntityRendering::Wireframe
    }
}

/// Geometry and presentation that must agree for one placed shape.
#[derive(Clone, Debug)]
pub(crate) struct EntityShape {
    geometry: Geometry,
    transform: Mat4,
    color: [f32; 4],
    rendering: EntityRendering,
}

impl EntityShape {
    pub(crate) fn solid(geometry: Geometry, transform: Mat4, color: [f32; 4]) -> Self {
        Self {
            geometry,
            transform,
            color,
            rendering: EntityRendering::Solid,
        }
    }

    pub(crate) fn wireframe(geometry: Geometry, transform: Mat4, color: [f32; 4]) -> Self {
        Self {
            geometry,
            transform,
            color,
            rendering: EntityRendering::Wireframe,
        }
    }
}

/// Everything to draw this frame.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    entities: Vec<Entity>,
    grid_enabled: bool,
}

impl Scene {
    pub(crate) fn new(grid_enabled: bool) -> Self {
        Self {
            entities: Vec::new(),
            grid_enabled,
        }
    }

    pub(crate) fn push(&mut self, entity: Entity) {
        self.entities.push(entity);
    }

    pub fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.entities.iter()
    }

    pub(crate) fn grid_enabled(&self) -> bool {
        self.grid_enabled
    }

    /// The first entity carrying `key`, which is where its label anchors.
    pub fn entity(&self, key: EntityKey) -> Option<&Entity> {
        self.entities.iter().find(|entity| entity.key() == key)
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
        for entity in self.entities.iter().filter(|entity| entity.key() == key) {
            sum += entity.origin();
            count += 1.0;
        }
        (count > 0.0).then(|| sum / count)
    }
}
