//! Server-only Studio runtime, grouped by domain boundary.

mod adapters;
mod application;
mod http;
mod recording;
mod robotics;
mod robots;
mod rollout;
mod storage;
mod training;

pub use http::server::run;
