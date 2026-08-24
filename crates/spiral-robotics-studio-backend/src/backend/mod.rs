//! Server-only Studio runtime, grouped by domain boundary.

mod adapters;
mod http;
mod recording;
mod robotics;
pub mod robots;
mod rollout;
mod storage;
mod training;
mod workflow;

pub use http::server::run;
