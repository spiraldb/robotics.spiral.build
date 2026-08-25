//! Physical I/O and fixed-rate control for Spiral Robotics.
//!
//! This crate owns protocol-neutral motor-bus contracts, concrete motor
//! adapters, camera capture, serial discovery, and the single control thread
//! that coordinates connected arms. Framework-free robot and calibration
//! policy remains in `spiral-robotics-core`; delivery lives in the backend and
//! persistence lives in `spiral-robotics-storage`.

pub mod bus;
pub mod cameras;
mod control;
mod control_policy;
pub mod feetech;
pub mod serial_ports;

pub use control::*;
pub use control_policy::*;
