//! Native server runtime for Spiral Robotics Studio.
//!
//! This crate is deliberately independent of the UI crate. The thin Studio
//! launcher passes its root component into [`run`], so editing browser code
//! does not invalidate the robotics, storage, training, and inference server.

pub mod backend;

pub use backend::run;
