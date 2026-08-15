//! Framework-neutral application actions.
//!
//! Delivery adapters call these services with plain commands. The services
//! own cross-component ordering and compensation; Axum and Dioxus only map
//! requests and results.

pub(crate) mod calibration;
pub(crate) mod catalog_lease;
pub(crate) mod checkpoints;
pub(crate) mod clock;
pub(crate) mod control_session;
pub(crate) mod data;
pub(crate) mod device_discovery;
mod episode_arms;
pub(crate) mod episode_queries;
pub(crate) mod episode_seek;
pub(crate) mod episodes;
pub(crate) mod error;
pub(crate) mod mode_control;
pub(crate) mod policy_runtime;
pub(crate) mod recording;
pub(crate) mod robot_control;
pub(crate) mod robots;
pub(crate) mod rollout;
pub(crate) mod training;
pub(crate) mod training_execution;
pub(crate) mod training_population;
pub(crate) mod training_runs;
