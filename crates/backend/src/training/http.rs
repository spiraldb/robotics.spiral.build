//! HTTP endpoints for training.

use crate::http::delivery::{ApiJson, ApiPath, Created, HttpFailure, ScopedProject};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    routing::{get, post},
};
use spiral_robotics_common::training::{TrainingConfig, TrainingRun, TrainingSnapshot};

pub(crate) struct TrainingEndpoints;

impl TrainingEndpoints {
    pub(crate) fn router() -> Router<StudioContext> {
        Router::new()
            .route(
                "/training/runs",
                get(Self::fetch_training).post(Self::start_training_run),
            )
            .route(
                "/training/runs/{id}",
                get(Self::fetch_run).delete(Self::delete_training_run),
            )
            .route("/training/runs/{id}/stop", post(Self::stop_training_run))
    }
    async fn start_training_run(
        ScopedProject(commands): ScopedProject<crate::training::commands::TrainingCommands>,
        ApiJson(config): ApiJson<TrainingConfig>,
    ) -> Result<Created<TrainingRun>, HttpFailure> {
        let result = commands.start_training_run(config).await?;
        Ok(Created(result))
    }
    async fn stop_training_run(
        ScopedProject(commands): ScopedProject<crate::training::commands::TrainingCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.stop_training_run(id).await?;
        Ok(Json(()))
    }
    async fn delete_training_run(
        ScopedProject(commands): ScopedProject<crate::training::commands::TrainingCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.delete_training_run(id).await?;
        Ok(Json(()))
    }

    async fn fetch_training(
        ScopedProject(commands): ScopedProject<crate::training::commands::TrainingCommands>,
    ) -> Result<Json<TrainingSnapshot>, HttpFailure> {
        let result = commands.snapshot().await?;
        Ok(Json(result))
    }
    async fn fetch_run(
        ScopedProject(commands): ScopedProject<crate::training::commands::TrainingCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<TrainingRun>, HttpFailure> {
        Ok(Json(commands.run(id).await?))
    }
}
