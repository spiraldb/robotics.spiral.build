//! Saved curations: list, load by identity, and a revision-guarded save and
//! delete;
//! publishing an output as a catalog view, or dropping it; and the preview
//! runs a node's Run starts, polls, stops, and reads answers from.
use super::preview::PreviewAnswer;
use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, ArrowResponse, Project};
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use spiral_robotics_common::curation::{
    CurationDeletion, CurationDocument, CurationSummary, MAX_CURATION_PAYLOAD_BYTES, PreviewQuery,
    PreviewRunStatus, PublishRequest, UnpublishRequest,
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
            .route("/curation-previews/{id}", get(Self::preview_status))
            .route(
                "/curation-previews/{id}/answers/{query}",
                get(Self::preview_answer),
            )
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        // A payload reaches 32 MiB, far past the default JSON body bound.
        Router::new()
            .route(
                "/curations/save",
                post(Self::save).layer(DefaultBodyLimit::max(MAX_SAVE_BODY_BYTES)),
            )
            .route("/curations/delete", post(Self::delete))
            .route("/curations/publish", post(Self::publish))
            .route("/curations/unpublish", post(Self::unpublish))
            .route("/curation-previews", post(Self::start_preview))
            .route("/curation-previews/{id}/stop", post(Self::stop_preview))
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
    async fn delete(
        Project(project): Project,
        ApiJson(request): ApiJson<CurationDeletion>,
    ) -> Result<Json<()>, OperationError> {
        project.curation().delete(request).await?;
        Ok(Json(()))
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
    async fn start_preview(
        Project(project): Project,
        ApiJson(queries): ApiJson<Vec<PreviewQuery>>,
    ) -> Result<Json<PreviewRunStatus>, OperationError> {
        Ok(Json(project.curation().previews().start(queries)?))
    }
    async fn preview_status(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<PreviewRunStatus>, OperationError> {
        Ok(Json(project.curation().previews().status(&id)?))
    }
    async fn stop_preview(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        project.curation().previews().stop(&id)?;
        Ok(Json(()))
    }
    /// One query's answer, as its own endpoint serves it: rows as JSON, a
    /// stream as Arrow IPC.
    async fn preview_answer(
        Project(project): Project,
        ApiPath((id, query)): ApiPath<(String, usize)>,
    ) -> Result<Response, OperationError> {
        Ok(match project.curation().previews().answer(&id, query)? {
            PreviewAnswer::Rows(rows) => Json(rows).into_response(),
            PreviewAnswer::Arrow(stream) => ArrowResponse(stream).into_response(),
        })
    }
}

#[cfg(test)]
mod tests;
