//! Saved recipes and judged runs; a run is one request the browser awaits.
use super::commands::CurationCommands;
use crate::http::delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject};
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    routing::{get, post},
};
use spiral_robotics_common::curation::{CurationSpec, CurationSummary, run::CurationRun};
pub(crate) struct CurationEndpoints;
impl CurationEndpoints {
    pub(crate) fn router() -> Router<StudioContext> {
        Router::new()
            .route("/curations", get(Self::list))
            .route("/curations/preview", post(Self::preview))
            .route("/curations/progress-score", post(Self::score_episode))
            .route("/curations/{id}", get(Self::load))
            .route("/curations/save", post(Self::save))
            .route("/curation-runs", post(Self::run))
    }
    async fn score_episode(
        ScopedProject(commands): ScopedProject<CurationCommands>,
        ApiJson(request): ApiJson<spiral_robotics_common::curation::progress::ProgressScoreRequest>,
    ) -> Result<Json<spiral_robotics_common::curation::progress::ProgressScoreResult>, HttpFailure>
    {
        Ok(Json(commands.score_episode(request).await?))
    }
    async fn preview(
        ScopedProject(commands): ScopedProject<CurationCommands>,
        ApiJson(curation): ApiJson<CurationSpec>,
    ) -> Result<Json<spiral_robotics_common::curation::run::CurationOutcome>, HttpFailure> {
        Ok(Json(commands.preview(curation).await?))
    }
    async fn list(
        ScopedProject(commands): ScopedProject<CurationCommands>,
    ) -> Result<Json<Vec<CurationSummary>>, HttpFailure> {
        Ok(Json(commands.list().await?))
    }
    async fn load(
        ScopedProject(commands): ScopedProject<CurationCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<CurationSpec>, HttpFailure> {
        Ok(Json(commands.load(&id).await?))
    }
    async fn save(
        ScopedProject(commands): ScopedProject<CurationCommands>,
        ApiJson(curation): ApiJson<CurationSpec>,
    ) -> Result<Json<CurationSpec>, HttpFailure> {
        Ok(Json(commands.save(curation).await?))
    }
    async fn run(
        ScopedProject(commands): ScopedProject<CurationCommands>,
        ApiJson(curation): ApiJson<CurationSpec>,
    ) -> Result<Json<CurationRun>, HttpFailure> {
        Ok(Json(commands.run(curation).await?))
    }
}
