//! The studio's 3D bench.
//!
//! The public boundary is intentionally smaller than the rendering pipeline.
//! The application supplies a validated [`Frame`] containing only the arms,
//! poses, mounts, and appearance needed for one draw. [`Viewport`] owns the
//! supplied robot descriptions, camera, drag state, composed geometry, GPU
//! surface, picking, and projection in a browser canvas; [`CameraRenderer`]
//! draws the same frame off screen, as a mounted camera sees it. Internal
//! scene keys and request/renderer types do not cross the crate boundary.
//!
//! The seam is: **this crate answers what is drawn and where it is on screen;
//! the application answers what the target means to the operator.**

mod arm;
mod camera;
mod canvas;
mod capture;
mod gesture;
mod model;
mod pick;
mod render;
mod robot;
mod scene;
mod urdf;
pub use model::RobotModel;

pub use arm::{Joint, SO101_JOINTS};
pub use capture::CameraRenderer;
pub use gesture::Command;
pub use pick::Viewport;
pub use render::ViewportError;
pub use robot::{ArmPresentation, CameraAttachment, CameraMount, Frame, FrameError, Target};
pub use scene::{Condition, Interaction};
