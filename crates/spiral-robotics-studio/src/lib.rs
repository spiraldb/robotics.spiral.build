//! The Spiral Robotics Studio interface, compiled by DX for the browser and SSR.
//!
//! The native runtime lives in `spiral-robotics-studio-backend`, keeping UI
//! edits from invalidating robotics, storage, training, and inference code.

mod frontend;

pub use frontend::App;
