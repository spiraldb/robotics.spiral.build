//! Dependency-minimal contracts shared across Spiral Robotics layers.
//!
//! Every value here belongs to one of two categories: (A) a concept consumed
//! by at least three workspace crates, or (B) a frontend/backend wire contract
//! used by a server function (including values embedded
//! in those contracts). Storage identities, database schemas, hardware-only
//! state, and native backend types belong to their owning crates.

pub mod build;
pub mod command_failure;
pub mod credentials;
pub mod curation;
pub mod datasets;
pub mod identity;
pub mod project;
pub mod recording;
pub mod recording_signals;
pub mod relations;
pub mod robots;
pub mod rollout;
pub mod runs;
pub mod settings;
pub mod sql;
pub mod status;
pub mod training;
pub mod workflow;
