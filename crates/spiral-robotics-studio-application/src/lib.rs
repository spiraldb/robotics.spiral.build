#![allow(async_fn_in_trait)]

//! Framework-neutral application actions.
//!
//! Delivery adapters call these services with plain commands. The services
//! own cross-component ordering and compensation; Axum and Dioxus only map
//! requests and results.

pub mod calibration;
pub mod catalog_lease;
pub mod checkpoints;
pub mod clock;
pub mod control_session;
pub mod data;
pub mod device_discovery;
mod episode_arms;
pub mod episode_queries;
pub mod episode_seek;
pub mod episodes;
pub mod error;
pub mod mode_control;
pub mod policy_runtime;
pub mod recording;
pub mod robot_control;
pub mod robots;
pub mod rollout;
pub mod training;
pub mod training_execution;
pub mod training_runs;
