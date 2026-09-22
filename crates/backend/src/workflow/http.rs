//! Documents and runs; no remote editor session.
use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, Project};
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
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/workflows", get(Self::list))
            .route("/workflows/{id}", get(Self::load))
            .route("/datasets/cameras", post(Self::cameras))
    }
    /// A run is a command: stopping the browser does not cancel an admitted query.
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/workflows/save", post(Self::save))
            .route("/workflow-runs", post(Self::run))
    }
    async fn list(Project(project): Project) -> Result<Json<Vec<WorkflowSummary>>, OperationError> {
        Ok(Json(project.workflow().list().await?))
    }
    async fn load(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<WorkflowDocument>, OperationError> {
        Ok(Json(project.workflow().load(&id).await?))
    }
    async fn save(
        Project(project): Project,
        ApiJson(document): ApiJson<WorkflowDocument>,
    ) -> Result<Json<WorkflowDocument>, OperationError> {
        Ok(Json(project.workflow().save(document).await?))
    }
    async fn run(
        Project(project): Project,
        ApiJson(workflow): ApiJson<WorkflowSpec>,
    ) -> Result<Json<WorkflowRun>, OperationError> {
        Ok(Json(project.workflow().run(workflow).await?))
    }
    async fn cameras(
        Project(project): Project,
        ApiJson(selection): ApiJson<DatasetSelection>,
    ) -> Result<Json<serde_json::Value>, OperationError> {
        Ok(Json(project.workflow().cameras(selection).await?))
    }
}
