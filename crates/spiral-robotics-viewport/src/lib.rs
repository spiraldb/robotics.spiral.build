//! The studio's 3D bench.
//!
//! This crate exists to hold a boundary still. The viewport used to be two
//! modules inside the studio that reached straight into application state —
//! reading the selected robot, its telemetry, and its calibration to decide
//! what to draw. Every new feature made that reach a little longer. Here it
//! cannot happen at all: nothing in this crate can see the studio, so the only
//! way in is through the types below.
//!
//! It is deliberately two layers.
//!
//! [`scene`], [`camera`], [`render`], [`pick`], and [`gesture`] know nothing
//! about robots. They deal in an [`EntityKey`] — an opaque number the caller
//! assigns — plus geometry, transforms, colour, and state. Picking returns a
//! key; it has no opinion about what the key means. [`Viewport::anchor`] turns
//! a point in the world into a point on the canvas, which is what lets the
//! application draw its own labels over the scene without this crate ever
//! learning what a label says.
//!
//! [`robot`] is the layer that does know what an arm is. Together with [`urdf`],
//! it owns the renderer-facing description, parser, and pose construction. It
//! accepts neutral measured joint angles from `spiral-robotics-core`, turns
//! them and any mounted cameras into a [`Scene`], and hands back the table that
//! says which key is which joint.
//!
//! The seam between them is the useful one: **this crate answers where a thing
//! is on screen, and the application answers what should be said about it.**

mod camera;
mod gesture;
mod pick;
mod render;
mod robot;
mod scene;
mod urdf;

/// Re-export the viewport's matrix types so callers use its presentation
/// geometry without depending on the domain crate for rendering details.
pub use glam;

pub use camera::Camera;
pub use gesture::{Drag, DragUpdate, Gesture};
pub use pick::{PickRequest, SceneFrame, Viewport};
pub use robot::{
    ArmPlacement, BENCH_SPACING, BenchPosition, CameraAttachment, CameraMount, RobotEntity,
    RobotScene, SceneAppearance,
};
pub use scene::{Condition, Entity, EntityKey, Interaction, Scene};
pub use urdf::{SO101_URDF, Urdf, UrdfError};
