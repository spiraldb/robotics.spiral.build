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
pub(crate) struct EntityKey(u32);

impl EntityKey {
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index as u32)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// What the operator is doing with an entity.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
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
pub(crate) struct Entity {
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
    /// A camera's view: the edges of the pyramid its picture sees, filling
    /// the shape's box with the apex at the box's -Z face.
    Frustum,
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

    pub(crate) fn key(&self) -> EntityKey {
        self.key
    }

    pub(crate) fn condition(&self) -> Condition {
        self.state.condition
    }

    /// The entity's origin in world space, which is where a label anchors.
    pub(crate) fn origin(&self) -> Vec3 {
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

    pub(crate) fn is_frustum(&self) -> bool {
        self.shape.rendering == EntityRendering::Frustum
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

    /// A camera's view pyramid, `size` across its picture at `size.z` deep,
    /// placed so `transform` centres the pyramid's box: its apex, the camera,
    /// sits on the box's -Z face. The box is also what picking tests.
    pub(crate) fn frustum(size: Vec3, transform: Mat4, color: [f32; 4]) -> Self {
        Self {
            geometry: Geometry::Box { size },
            transform,
            color,
            rendering: EntityRendering::Frustum,
        }
    }
}

/// Everything to draw this frame.
#[derive(Clone, Debug, Default)]
pub(crate) struct Scene {
    entities: Vec<Entity>,
    grid_enabled: bool,
    grid_height: f32,
}

impl Scene {
    pub(crate) fn new(grid_enabled: bool, grid_height: f32) -> Self {
        Self {
            entities: Vec::new(),
            grid_enabled,
            grid_height,
        }
    }

    pub(crate) fn push(&mut self, entity: Entity) {
        self.entities.push(entity);
    }

    pub(crate) fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.entities.iter()
    }

    pub(crate) fn grid_enabled(&self) -> bool {
        self.grid_enabled
    }

    pub(crate) fn grid_height(&self) -> f32 {
        self.grid_height
    }

    /// Where a key's label belongs, in world space.
    ///
    /// The midpoint of everything sharing the key rather than the first shape's
    /// origin: a joint is several shells, and anchoring to whichever the
    /// description happened to list first would put the label on the edge of
    /// the part instead of on the part.
    pub(crate) fn anchor_point(&self, key: EntityKey) -> Option<Vec3> {
        let mut sum = Vec3::ZERO;
        let mut count = 0.0;
        for entity in self.entities.iter().filter(|entity| entity.key() == key) {
            sum += entity.origin();
            count += 1.0;
        }
        (count > 0.0).then(|| sum / count)
    }
}
