//! Frontend/backend wire protocol for Spiral Robotics Studio.
//!
//! This crate contains serializable request, response, and workflow document
//! values. Database schemas and native backend types belong elsewhere.

pub mod catalog;
pub mod datasets;
pub mod recording;
pub mod relations;
pub mod robots;
pub mod rollout;
pub mod settings;
pub mod status;
pub mod training;
pub mod workflow;
