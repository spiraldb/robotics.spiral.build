//! Trusted backend for Spiral Robotics Studio.
//!
//! This crate is deliberately independent of the frontend crate. The thin Studio
//! launcher passes its root component into [`run`], so editing browser code
//! does not invalidate robotics, storage, or remote policy orchestration.

#![allow(async_fn_in_trait)]

mod adapters;
mod clock;
mod data;
mod http;
mod recording;
mod robotics;
mod robots;
mod rollout;
mod training;
mod workflow;

pub use http::server::run;
