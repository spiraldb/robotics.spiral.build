//! Shared building blocks for the Spiral Robotics command-line tools and server.
//!
//! Hardware lives here: protocol-neutral motor-bus contracts, concrete motor
//! adapters, camera capture, and serial discovery. Policy code targets
//! [`bus::MotorBus`]; driver-specific protocols and errors remain behind their
//! adapters.
//! Everything hardware-free — calibration, the SO-101 joint model, and the
//! studio's wire types — lives in [`spiral_robotics_core`] so the WebAssembly UI
//! can share it.

pub mod bus;
pub mod calibration_codec;
pub mod cameras;
pub mod feetech;
pub mod robot_config;
pub mod serial_ports;

pub use spiral_robotics_core::calibration;
