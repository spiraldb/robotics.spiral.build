//! Domain types shared by the `rerobot` command-line tools, the studio server,
//! and the WebAssembly user interface.
//!
//! Nothing in this crate touches a serial port, a camera, or the filesystem, so
//! the browser and the server agree on one definition of a joint angle, a
//! calibration, and every request and response they exchange.

pub mod api;
pub mod arm;
pub mod calibration;
