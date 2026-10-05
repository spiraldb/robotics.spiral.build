//! Dependency-minimal contracts shared across Spiral Robotics layers.
//!
//! Every value here belongs to one of two categories: (A) a concept consumed
//! by at least three workspace crates, or (B) a wire contract of the Studio or
//! robot connector HTTP APIs (including values embedded in those contracts).
//! Storage identities, database schemas, hardware-only state, and native
//! server types belong to their owning crates.

pub mod account;
pub mod command_failure;
pub mod curation;
pub mod datasets;
pub mod identity;
pub mod project;
pub mod providers;
pub mod recording;
pub mod recording_signals;
pub mod relations;
pub mod robot_connector;
pub mod robots;
pub mod rollout;
pub mod settings;
pub mod sql;
pub mod training;
pub mod workspace;
