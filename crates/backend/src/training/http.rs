//! HTTP endpoints for training.

use crate::http::contract::{ApiRoutes, Created};
use crate::http::delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject};

use axum::Json;
use spiral_robotics_common::training::{TrainingConfig, TrainingRun, TrainingSnapshot};

pub(crate) struct TrainingEndpoints;

impl TrainingEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.post("/training/runs", Self::start_training_run);
        routes.post("/training/runs/{id}/stop", Self::stop_training_run);
        routes.delete("/training/runs/{id}", Self::delete_training_run);
        routes.get("/training/runs", Self::fetch_training);
        routes.get("/training/runs/{id}", Self::fetch_run);
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
