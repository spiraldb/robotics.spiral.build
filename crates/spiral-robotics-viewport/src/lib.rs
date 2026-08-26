//! The studio's 3D bench.
//!
//! The public boundary is intentionally smaller than the rendering pipeline.
//! The application supplies a validated [`Frame`] containing only the arms,
//! poses, mounts, and appearance needed for one draw. [`Viewport`] owns the
//! bundled robot description, camera, drag state, composed geometry, WebGL,
//! picking, and projection. Internal scene keys and request/renderer types do
//! not cross the crate boundary.
//!
//! The seam is: **this crate answers what is drawn and where it is on screen;
//! the application answers what the target means to the operator.**

mod arm;
mod camera;
mod gesture;
mod pick;
mod render;
mod robot;
mod scene;
mod urdf;

pub use arm::{ArmPose, Joint, SO101_JOINTS};
pub use gesture::Command;
pub use pick::{Viewport, ViewportError};
pub use robot::{Frame, FrameError, Target};
pub use scene::{Condition, Interaction};

/// Where the SO-101 tool tip sits for a display pose, in millimetres from the
/// arm base.
pub fn tool_position(pose: &ArmPose) -> Option<[f32; 3]> {
    urdf::so101()
        .so101_tool_position(pose)
        .map(|position| position.to_array())
}
