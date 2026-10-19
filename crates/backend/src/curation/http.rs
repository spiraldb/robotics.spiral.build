//! Saved curations: list, load by identity, and a revision-guarded save and
//! delete; the fill runs that write the answers a view reads and then
//! publish it, which Publish and a Score progress preview start and poll;
//! which episodes a column of answers already holds, and which columns a
//! dataset holds; and the published views and dropping one.
use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, Project};
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use spiral_robotics_common::curation::{
    AnswerColumn, CurationDeletion, CurationDocument, CurationSummary, FillRequest, FillStatus,
    FilledEpisodesRequest, MAX_CURATION_PAYLOAD_BYTES, PublishedView, UnpublishRequest,
};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use std::collections::BTreeSet;

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
            .route("/curated-views", get(Self::published))
            .route("/curation-answers/filled", post(Self::filled))
            .route("/curation-answers/columns", post(Self::answer_columns))
            .route("/curation-fills", get(Self::publications))
            .route("/curation-fills/{id}", get(Self::fill_status))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        // A payload reaches 32 MiB, far past the default JSON body bound.
        Router::new()
            .route(
                "/curations/save",
                post(Self::save).layer(DefaultBodyLimit::max(MAX_SAVE_BODY_BYTES)),
            )
            .route("/curations/delete", post(Self::delete))
            .route("/curations/unpublish", post(Self::unpublish))
            .route("/curation-fills", post(Self::start_fill))
            .route("/curation-fills/{id}/stop", post(Self::stop_fill))
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
    async fn start_fill(
        Project(project): Project,
        ApiJson(request): ApiJson<FillRequest>,
    ) -> Result<Json<FillStatus>, OperationError> {
        Ok(Json(project.curation().fill(request)?))
    }
    async fn filled(
        Project(project): Project,
        ApiJson(request): ApiJson<FilledEpisodesRequest>,
    ) -> Result<Json<BTreeSet<EpisodeId>>, OperationError> {
        Ok(Json(project.curation().filled(request).await?))
    }
    async fn answer_columns(
        Project(project): Project,
        ApiJson(dataset): ApiJson<DatasetSelection>,
    ) -> Result<Json<BTreeSet<AnswerColumn>>, OperationError> {
        Ok(Json(project.curation().answer_columns(dataset).await?))
    }
    async fn published(
        Project(project): Project,
    ) -> Result<Json<Vec<PublishedView>>, OperationError> {
        Ok(Json(project.curation().published().await?))
    }
    /// Every run that publishes a view, as Data shows them.
    async fn publications(Project(project): Project) -> Json<Vec<FillStatus>> {
        Json(project.curation().fills().publications())
    }
    async fn fill_status(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<FillStatus>, OperationError> {
        Ok(Json(project.curation().fills().status(&id)?))
    }
    async fn stop_fill(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        project.curation().fills().stop(&id)?;
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
