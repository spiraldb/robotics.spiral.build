//! Saved recipes and judged runs; the browser polls runs like Workflow's.
use super::commands::CurationCommands;
use crate::http::{
    contract::ApiRoutes,
    delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject},
};
use axum::Json;
use spiral_robotics_common::curation::{CurationSpec, CurationSummary, run::CurationRun};
pub(crate) struct CurationEndpoints;
impl CurationEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/curations", Self::list);
        routes.post("/curations/preview", Self::preview);
        routes.post("/curations/progress-score", Self::score_episode);
        routes.get("/curations/{id}", Self::load);
        routes.post("/curations/save", Self::save);
        routes.get("/curation-runs", Self::runs);
        routes.post("/curation-runs", Self::start);
        routes.get("/curation-runs/{id}", Self::run);
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
    async fn runs(
        ScopedProject(commands): ScopedProject<CurationCommands>,
    ) -> Result<Json<Vec<CurationRun>>, HttpFailure> {
        Ok(Json(commands.runs()?))
    }
    async fn start(
        ScopedProject(commands): ScopedProject<CurationCommands>,
        ApiJson(curation): ApiJson<CurationSpec>,
    ) -> Result<Json<CurationRun>, HttpFailure> {
        Ok(Json(commands.start(curation)?))
    }
    async fn run(
        ScopedProject(commands): ScopedProject<CurationCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<CurationRun>, HttpFailure> {
        Ok(Json(commands.run(&id)?))
    }
}
