//! Rollout endpoints.
use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, Project};
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    routing::{get, post},
};
use spiral_robotics_common::rollout::{RolloutConfig, RolloutSnapshot};
pub(crate) struct RolloutEndpoints;
impl RolloutEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new().route("/rollout", get(Self::fetch_rollout))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/rollout", post(Self::start_rollout))
            .route("/rollout/{id}/stop", post(Self::stop_rollout))
    }
    async fn start_rollout(
        Project(project): Project,
        ApiJson(config): ApiJson<RolloutConfig>,
    ) -> Result<Json<RolloutSnapshot>, OperationError> {
        let config = project.training().admit_rollout(config).await?;
        let snapshot = project
            .rollout()
            .start(config)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(snapshot))
    }
    async fn stop_rollout(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        project
            .rollout()
            .stop(&id)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(()))
    }
    /// The current rollout session, or an idle snapshot before the first one.
    async fn fetch_rollout(Project(project): Project) -> Json<RolloutSnapshot> {
        Json(project.rollout().snapshot())
    }
}
