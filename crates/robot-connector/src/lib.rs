//! The robot connector: an HTTP endpoint that runs next to the arms it drives.
//!
//! It owns everything physical — serial buses, cameras, calibration, the
//! fixed-rate control loop — along with the takes recorded from it and the
//! policies rolled out on it. Studio is its browser frontend: the browser talks
//! to this server directly, carries finished recordings to Studio, and writes
//! the policies Studio trained here. The connector never reaches Spiral.

#![allow(async_fn_in_trait)]

mod clock;
mod context;
mod error;
mod http;
mod policies;
mod recording;
mod robotics;
mod robots;
mod rollout;
mod simulation;
mod sync;

pub(crate) use error::OperationError;
pub use http::server::RobotConnectorServer;
