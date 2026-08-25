//! Trusted backend for Spiral Robotics Studio.
//!
//! This crate is deliberately independent of the frontend crate. The thin Studio
//! launcher passes its root component into [`run`], so editing browser code
//! does not invalidate robotics, storage, or remote policy orchestration.

#![allow(async_fn_in_trait)]

mod adapters;
mod clock;
pub mod data;
mod http;
pub mod recording;
mod robotics;
pub mod robots;
pub mod rollout;
pub mod training;
pub mod workflow;

pub use http::server::run;
