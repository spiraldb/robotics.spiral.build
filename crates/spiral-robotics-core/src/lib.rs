//! Domain types shared by the Spiral Robotics command-line tools and adapters.
//!
//! Nothing in this crate touches a serial port, a camera, or the filesystem, so
//! every delivery mechanism agrees on one definition of a joint angle and a
//! calibration without making domain policy depend on delivery contracts.

pub mod arm;
pub mod calibration;
pub mod episode;
pub mod identity;
pub mod operation;
pub mod query;
pub mod workflow;
