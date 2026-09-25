//! Saved curations: list, load by identity, and one revision-guarded save;
//! and publishing an output as a catalog view, or dropping it.
use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, Project};
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use spiral_robotics_common::curation::{
    CurationDocument, CurationSummary, MAX_CURATION_PAYLOAD_BYTES, PublishRequest, UnpublishRequest,
};

/// The body carries the payload as a JSON string, so escaping and the
/// surrounding fields both add bytes to it. Bounding the request at twice the
/// payload limit keeps every document the contract allows acceptable; one past
/// that limit is still reported as invalid input, not as an oversized request.
const MAX_SAVE_BODY_BYTES: usize = 2 * MAX_CURATION_PAYLOAD_BYTES;

pub(crate) struct CurationEndpoints;
impl CurationEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/curations", get(Self::list))
            .route("/curations/{id}", get(Self::load))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        // A payload reaches 32 MiB, far past the default JSON body bound.
        Router::new()
            .route(
                "/curations/save",
                post(Self::save).layer(DefaultBodyLimit::max(MAX_SAVE_BODY_BYTES)),
            )
            .route("/curations/publish", post(Self::publish))
            .route("/curations/unpublish", post(Self::unpublish))
    }
    async fn list(Project(project): Project) -> Result<Json<Vec<CurationSummary>>, OperationError> {
        Ok(Json(project.curation().list().await?))
    }
    async fn load(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<CurationDocument>, OperationError> {
        Ok(Json(project.curation().load(&id).await?))
    }
    async fn save(
        Project(project): Project,
        ApiJson(document): ApiJson<CurationDocument>,
    ) -> Result<Json<CurationDocument>, OperationError> {
        Ok(Json(project.curation().save(document).await?))
    }
    async fn publish(
        Project(project): Project,
        ApiJson(request): ApiJson<PublishRequest>,
    ) -> Result<Json<()>, OperationError> {
        project.curation().publish(request).await?;
        Ok(Json(()))
    }
    async fn unpublish(
        Project(project): Project,
        ApiJson(request): ApiJson<UnpublishRequest>,
    ) -> Result<Json<()>, OperationError> {
        project.curation().unpublish(request).await?;
        Ok(Json(()))
    }
}

#[cfg(test)]
mod tests;
