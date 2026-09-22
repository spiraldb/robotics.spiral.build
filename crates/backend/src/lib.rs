//! Trusted backend for Spiral Robotics Studio.
//!
//! This crate is deliberately independent of the frontend crate. Its binary
//! starts [`StudioServer::run`], which serves the built browser assets
//! alongside the HTTP API. Browser rendering stays in the frontend crate.

#![allow(async_fn_in_trait)]

mod adapters;
mod clock;
mod curation;
mod data;
mod error;
mod exploration;
mod http;
mod project_admission;
mod project_session;
mod recording;
mod recording_session;
mod robotics;
mod robots;
mod rollout;
mod sync;
mod training;
mod transport;
mod workflow;

pub(crate) use error::BackendCommandError;
pub(crate) use error::OperationError;
pub use http::server::StudioServer;
