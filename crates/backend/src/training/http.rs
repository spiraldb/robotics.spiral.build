//! HTTP endpoints for training.

use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, Created, Project};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    routing::{delete, get, post},
};
use spiral_robotics_common::training::{
    TrainingCheckpoint, TrainingConfig, TrainingRun, TrainingSnapshot,
};

pub(crate) struct TrainingEndpoints;

impl TrainingEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/training/runs", get(Self::fetch_training))
            .route("/training/runs/{id}", get(Self::fetch_run))
            .route("/training/checkpoints", get(Self::fetch_checkpoints))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/training/runs", post(Self::start_training_run))
            .route("/training/runs/{id}", delete(Self::delete_training_run))
            .route("/training/runs/{id}/stop", post(Self::stop_training_run))
    }
    async fn start_training_run(
        Project(project): Project,
        ApiJson(config): ApiJson<TrainingConfig>,
    ) -> Result<Created<TrainingRun>, OperationError> {
        Ok(Created(project.training().start(config).await?))
    }
    async fn stop_training_run(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        project.training().stop(&id).await?;
        Ok(Json(()))
    }
    async fn delete_training_run(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        let active_rollout = project.rollout().active_run_id();
        project
            .training()
            .delete(&id, active_rollout.as_deref())
            .await?;
        Ok(Json(()))
    }
    async fn fetch_training(
        Project(project): Project,
    ) -> Result<Json<TrainingSnapshot>, OperationError> {
        Ok(Json(project.training().catalog().await?))
    }
    async fn fetch_checkpoints(
        Project(project): Project,
    ) -> Result<Json<Vec<TrainingCheckpoint>>, OperationError> {
        Ok(Json(project.training().checkpoints().await?))
    }
    async fn fetch_run(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<TrainingRun>, OperationError> {
        Ok(Json(project.training().run(&id).await?))
    }
}
