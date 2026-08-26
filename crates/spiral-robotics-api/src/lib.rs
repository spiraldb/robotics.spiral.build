//! Frontend/backend wire protocol for Spiral Robotics Studio.
//!
//! This crate contains serializable request, response, and workflow document
//! values together with contract-level validation, including the conservative
//! SQL gates shared by those contracts. Database schemas and native backend
//! types belong elsewhere.

pub mod catalog;
pub mod datasets;
pub mod identity;
pub mod recording;
pub mod relations;
pub mod robots;
pub mod rollout;
pub mod settings;
pub mod sql;
pub mod status;
pub mod training;
pub mod workflow;
