//! Fullstack training and rollout commands.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_api::rollout::{RolloutConfig, RolloutSnapshot};
use spiral_robotics_api::training::{TrainingConfig, TrainingRun};

#[cfg(feature = "server")]
use dioxus::server::axum::Extension;
#[cfg(feature = "server")]
use spiral_robotics_backend::TrainingRolloutCommands;

/// Message-only command failure transported as Dioxus typed error details.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CommandError(String);

impl From<ServerFnError> for CommandError {
    fn from(error: ServerFnError) -> Self {
        match error {
            ServerFnError::ServerError { message, .. } => Self(message),
            error => Self(error.to_string()),
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(feature = "server")]
impl dioxus::fullstack::AsStatusCode for CommandError {
    fn as_status_code(&self) -> dioxus::fullstack::StatusCode {
        dioxus::fullstack::StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[server(
    endpoint = "/server-functions/training/start",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn start_training_run(
    config: TrainingConfig,
) -> Result<TrainingRun, CommandError> {
    commands
        .0
        .start_training_run(config)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "/server-functions/training/stop",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn stop_training_run(id: String) -> Result<(), CommandError> {
    commands
        .0
        .stop_training_run(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "/server-functions/training/delete",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn delete_training_run(id: String) -> Result<(), CommandError> {
    commands
        .0
        .delete_training_run(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "/server-functions/rollout/start",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn start_rollout(config: RolloutConfig) -> Result<RolloutSnapshot, CommandError> {
    commands
        .0
        .start_rollout(config)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "/server-functions/rollout/stop",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn stop_rollout() -> Result<(), CommandError> {
    commands
        .0
        .stop_rollout()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}
