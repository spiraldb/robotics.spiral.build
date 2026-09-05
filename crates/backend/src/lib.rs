//! Trusted backend for Spiral Robotics Studio.
//!
//! This crate is deliberately independent of the frontend crate. The thin Studio
//! launcher passes its root component into [`StudioServer::run`], so editing browser code
//! does not invalidate robotics, storage, or remote policy orchestration.

#![allow(async_fn_in_trait)]

mod adapters;
mod clock;
mod data;
mod data_commands;
mod device_commands;
mod error;
mod http;
mod main_runtime;
mod project_admission;
mod project_session;
mod recording;
mod recording_commands;
mod recording_session;
mod robotics;
mod robots;
mod rollout;
mod sync;
mod training;
mod training_rollout;
mod workflow;
mod workflow_commands;

pub use data_commands::{DataCommands, DataFailure, DataSettingsChange, MAX_PROJECT_IMPORT_BYTES};
pub use device_commands::{DeviceCommands, DeviceFailure};
pub(crate) use error::OperationError;
pub use http::server::StudioServer;
pub use recording_commands::{RecordingCommands, RecordingFailure};
pub use training_rollout::{TrainingRolloutCommands, TrainingRolloutFailure};
pub use workflow_commands::{WorkflowCommands, WorkflowFailure};
