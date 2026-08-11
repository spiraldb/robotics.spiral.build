//! Server-only Studio runtime, grouped by domain boundary.

pub mod http;
pub mod recording;
pub mod robotics;
pub mod robots;
pub mod storage;
pub mod training;

pub use http::server::run;
