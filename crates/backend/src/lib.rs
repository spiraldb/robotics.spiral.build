//! Trusted backend for Spiral Robotics Studio.
//!
//! This crate is deliberately independent of the frontend crate. Its binary
//! starts [`StudioServer::run`], which serves the built browser assets
//! alongside the HTTP API. Browser rendering stays in the frontend crate, and
//! robot hardware stays in the robot connector, which links itself to Studio;
//! browsers reach it through Studio.

#![allow(async_fn_in_trait)]
#![recursion_limit = "256"]

mod account;
mod clock;
mod curation;
mod data;
mod error;
mod held_runs;
mod http;
mod projects;
mod robot_connector;
mod sync;
mod training;
mod transport;

pub(crate) use error::OperationError;
pub use http::server::StudioServer;
