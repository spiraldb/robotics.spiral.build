//! Trusted backend for Spiral Robotics Studio.
//!
//! This crate is deliberately independent of the frontend crate. The thin Studio
//! launcher passes its root component into [`StudioServer::run`], so editing browser code
//! does not invalidate robotics, storage, or remote policy orchestration.

#![allow(async_fn_in_trait)]

mod adapters;
mod clock;
mod data;
mod error;
mod http;
mod recording;
mod recording_session;
mod robotics;
mod robots;
mod rollout;
mod sync;
#[cfg(test)]
mod test_database;
mod training;
mod workflow;

pub(crate) use error::OperationError;
pub use http::server::StudioServer;
