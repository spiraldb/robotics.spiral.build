//! Trusted backend for Spiral Robotics Studio.
//!
//! This crate is deliberately independent of the frontend crate. Its binary
//! starts [`StudioServer::run`], which serves the built browser assets
//! alongside the HTTP API; the desktop app runs the same server in its own
//! process through [`DesktopStudio`]. Browser rendering stays in the frontend
//! crate.

#![allow(async_fn_in_trait)]

mod account;
mod adapters;
mod clock;
mod curation;
mod data;
mod error;
mod http;
mod projects;
mod recording;
mod robotics;
mod robots;
mod rollout;
mod sync;
mod training;
mod transport;

pub(crate) use error::OperationError;
pub use http::server::{DesktopFiles, DesktopStudio, StudioServer};
