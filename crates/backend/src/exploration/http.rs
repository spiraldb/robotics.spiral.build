//! Saved explorations: list, load by identity, and one revision-guarded save.
use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, Project};
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use spiral_robotics_common::exploration::{
    ExplorationDocument, ExplorationSummary, MAX_EXPLORATION_PAYLOAD_BYTES,
};

/// The body carries the payload as a JSON string, so escaping and the
/// surrounding fields both add bytes to it. Bounding the request at twice the
/// payload limit keeps every document the contract allows acceptable; one past
/// that limit is still reported as invalid input, not as an oversized request.
const MAX_SAVE_BODY_BYTES: usize = 2 * MAX_EXPLORATION_PAYLOAD_BYTES;

pub(crate) struct ExplorationEndpoints;
impl ExplorationEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/explorations", get(Self::list))
            .route("/explorations/{id}", get(Self::load))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        // A payload reaches 32 MiB, far past the default JSON body bound.
        Router::new().route(
            "/explorations/save",
            post(Self::save).layer(DefaultBodyLimit::max(MAX_SAVE_BODY_BYTES)),
        )
    }
    async fn list(
        Project(project): Project,
    ) -> Result<Json<Vec<ExplorationSummary>>, OperationError> {
        Ok(Json(project.explorations().list().await?))
    }
    async fn load(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<ExplorationDocument>, OperationError> {
        Ok(Json(project.explorations().load(&id).await?))
    }
    async fn save(
        Project(project): Project,
        ApiJson(document): ApiJson<ExplorationDocument>,
    ) -> Result<Json<ExplorationDocument>, OperationError> {
        Ok(Json(project.explorations().save(document).await?))
    }
}

#[cfg(test)]
mod tests;
