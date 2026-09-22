//! Documents and runs; no remote editor session.
use super::commands::WorkflowCommands;
use crate::http::delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject};
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    routing::{get, post},
};
use spiral_robotics_common::{
    datasets::DatasetSelection,
    workflow::{
        WorkflowSpec, WorkflowSummary,
        document::{WorkflowDocument, WorkflowRun},
    },
};
pub(crate) struct WorkflowEndpoints;
impl WorkflowEndpoints {
    pub(crate) fn router() -> Router<StudioContext> {
        Router::new()
            .route("/workflows", get(Self::list))
            .route("/workflows/{id}", get(Self::load))
            .route("/workflows/save", post(Self::save))
            .route("/workflow-runs", post(Self::run))
            .route("/datasets/cameras", post(Self::cameras))
    }
    async fn list(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
    ) -> Result<Json<Vec<WorkflowSummary>>, HttpFailure> {
        Ok(Json(commands.list().await?))
    }
    async fn load(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<WorkflowDocument>, HttpFailure> {
        Ok(Json(commands.load(&id).await?))
    }
    async fn save(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
        ApiJson(document): ApiJson<WorkflowDocument>,
    ) -> Result<Json<WorkflowDocument>, HttpFailure> {
        Ok(Json(commands.save(document).await?))
    }
    async fn run(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
        ApiJson(workflow): ApiJson<WorkflowSpec>,
    ) -> Result<Json<WorkflowRun>, HttpFailure> {
        Ok(Json(commands.run(workflow).await?))
    }
    async fn cameras(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
        ApiJson(selection): ApiJson<DatasetSelection>,
    ) -> Result<Json<serde_json::Value>, HttpFailure> {
        Ok(Json(commands.cameras(selection).await?))
    }
}
