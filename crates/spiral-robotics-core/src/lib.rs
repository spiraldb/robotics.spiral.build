//! Domain types shared by the Spiral Robotics command-line tools, the studio server,
//! and the WebAssembly user interface.
//!
//! Nothing in this crate touches a serial port, a camera, or the filesystem, so
//! the browser and the server agree on one definition of a joint angle, a
//! calibration, and every request and response they exchange.

pub mod api;
pub mod arm;
pub mod calibration;
pub mod spiral;
pub mod urdf;

/// Re-exported so every crate that poses a robot agrees on what a matrix is.
pub use glam;
