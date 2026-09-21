//! Documents, planning, runs, and model transport; no remote editor session.
use super::commands::WorkflowCommands;
use crate::http::{
    contract::ApiRoutes,
    delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject},
};
use axum::Json;
use spiral_robotics_common::{
    datasets::DatasetSelection,
    workflow::{
        WorkflowSpec, WorkflowSummary,
        document::{WorkflowDocument, WorkflowPlan, WorkflowRun},
    },
};
pub(crate) struct WorkflowEndpoints;
impl WorkflowEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/workflows", Self::list);
        routes.get("/workflows/{id}", Self::load);
        routes.post("/workflows/save", Self::save);
        routes.post("/workflows/plan", Self::plan);
        routes.get("/workflow-runs", Self::runs);
        routes.post("/workflow-runs", Self::start);
        routes.get("/workflow-runs/{id}", Self::run);
        routes.get("/models/capabilities", Self::capabilities);
        routes.post("/datasets/cameras", Self::cameras);
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
    async fn plan(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
        ApiJson(workflow): ApiJson<WorkflowSpec>,
    ) -> Result<Json<WorkflowPlan>, HttpFailure> {
        Ok(Json(commands.plan(workflow).await?))
    }
    async fn runs(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
    ) -> Result<Json<Vec<WorkflowRun>>, HttpFailure> {
        Ok(Json(commands.runs()?))
    }
    async fn start(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
        ApiJson(workflow): ApiJson<WorkflowSpec>,
    ) -> Result<Json<WorkflowRun>, HttpFailure> {
        Ok(Json(commands.start(workflow)?))
    }
    async fn run(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<WorkflowRun>, HttpFailure> {
        Ok(Json(commands.run(&id)?))
    }
    async fn capabilities(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
    ) -> Result<Json<spiral_robotics_common::relations::SpiralQuerySnapshot>, HttpFailure> {
        Ok(Json(commands.capabilities().await?))
    }
    async fn cameras(
        ScopedProject(commands): ScopedProject<WorkflowCommands>,
        ApiJson(selection): ApiJson<DatasetSelection>,
    ) -> Result<Json<serde_json::Value>, HttpFailure> {
        Ok(Json(commands.cameras(selection).await?))
    }
}
