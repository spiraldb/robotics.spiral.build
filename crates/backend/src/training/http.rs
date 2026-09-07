//! HTTP endpoints for training.

use crate::http::contract::{ApiRoutes, Created};
use crate::http::delivery::{ApiJson, ApiPath, ApiQuery, HttpFailure, ScopedProject};

use axum::Json;
use spiral_robotics_common::training::{
    TrainingConfig, TrainingRun, TrainingSelection, TrainingSnapshot,
};

pub(crate) struct TrainingEndpoints;

impl TrainingEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.post("/training/runs", Self::start_training_run);
        routes.post("/training/runs/{id}/stop", Self::stop_training_run);
        routes.delete("/training/runs/{id}", Self::delete_training_run);
        routes.get("/training", Self::fetch_training);
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
        ApiQuery(selection): ApiQuery<TrainingSelection>,
    ) -> Result<Json<TrainingSnapshot>, HttpFailure> {
        let selected = selection.into_selected();
        let result = commands.snapshot(selected)?;
        Ok(Json(result))
    }
}
