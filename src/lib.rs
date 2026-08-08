//! Shared building blocks for the `rerobot` command-line tools and server.
//!
//! Hardware lives here: motor buses, camera capture, and serial discovery.
//! Everything hardware-free — calibration, the SO-101 joint model, and the
//! studio's wire types — lives in [`rerobot_core`] so the WebAssembly UI can
//! share it.

pub mod bus;
pub mod cameras;
pub mod feetech;
pub mod robot_config;
pub mod serial_ports;

pub use rerobot_core::{api, arm, calibration};
