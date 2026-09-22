//! Saved recipes and judged runs; a run is one request the browser awaits.
use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, Project};
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    routing::{get, post},
};
use spiral_robotics_common::curation::progress::{ProgressScoreRequest, ProgressScoreResult};
use spiral_robotics_common::curation::run::{CurationOutcome, CurationRun};
use spiral_robotics_common::curation::{CurationSpec, CurationSummary};
pub(crate) struct CurationEndpoints;
impl CurationEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/curations", get(Self::list))
            .route("/curations/preview", post(Self::preview))
            .route("/curations/progress-score", post(Self::score_episode))
            .route("/curations/{id}", get(Self::load))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/curations/save", post(Self::save))
            .route("/curation-runs", post(Self::run))
    }
    async fn score_episode(
        Project(project): Project,
        ApiJson(request): ApiJson<ProgressScoreRequest>,
    ) -> Result<Json<ProgressScoreResult>, OperationError> {
        Ok(Json(project.curation().score_episode(request).await?))
    }
    async fn preview(
        Project(project): Project,
        ApiJson(curation): ApiJson<CurationSpec>,
    ) -> Result<Json<CurationOutcome>, OperationError> {
        Ok(Json(project.curation().preview(&curation).await?))
    }
    async fn list(Project(project): Project) -> Result<Json<Vec<CurationSummary>>, OperationError> {
        Ok(Json(project.curation().list().await?))
    }
    async fn load(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<CurationSpec>, OperationError> {
        Ok(Json(project.curation().load(&id).await?))
    }
    async fn save(
        Project(project): Project,
        ApiJson(curation): ApiJson<CurationSpec>,
    ) -> Result<Json<CurationSpec>, OperationError> {
        Ok(Json(project.curation().save(curation).await?))
    }
    async fn run(
        Project(project): Project,
        ApiJson(curation): ApiJson<CurationSpec>,
    ) -> Result<Json<CurationRun>, OperationError> {
        Ok(Json(project.curation().run(curation).await?))
    }
}
