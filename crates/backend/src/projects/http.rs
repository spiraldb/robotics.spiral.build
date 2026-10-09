//! HTTP endpoints for workspaces and their projects.

use crate::OperationError;
use crate::http::delivery::{ApiBytes, ApiJson, ApiPath, Project, SignedIn};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    routing::{delete, get, post},
};
use spiral_robotics_common::datasets::ProjectImportSummary;
use spiral_robotics_common::workspace::{NewWorkspace, Workspace};
use spiral_robotics_storage::bundle::ProjectBundle;

pub(crate) struct WorkspaceEndpoints;

impl WorkspaceEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new().route("/workspaces", get(Self::workspaces))
    }

    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/workspaces", post(Self::create))
            .route("/workspaces/{project_id}", delete(Self::delete))
    }

    async fn workspaces(
        State(state): State<StudioContext>,
        SignedIn(session): SignedIn,
    ) -> Result<Json<Vec<Workspace>>, OperationError> {
        Ok(Json(state.projects().workspaces(&session).await?))
    }

    async fn create(
        State(state): State<StudioContext>,
        SignedIn(session): SignedIn,
        ApiJson(request): ApiJson<NewWorkspace>,
    ) -> Result<Json<Workspace>, OperationError> {
        Ok(Json(
            state
                .projects()
                .create_workspace(&session, &request)
                .await?,
        ))
    }

    /// The workspace's robot connector token goes with it, and so does the
    /// connector that linked with it.
    async fn delete(
        State(state): State<StudioContext>,
        SignedIn(session): SignedIn,
        ApiPath(project_id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        state
            .projects()
            .delete_workspace(&session, &project_id)
            .await?;
        state.robot_connectors().disconnect(&project_id);
        Ok(Json(()))
    }
}

pub(crate) struct ProjectEndpoints;

impl ProjectEndpoints {
    /// A bundle reaches 256 MiB, far past the default body bound.
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new().route(
            "/project/import",
            post(Self::import_project).layer(DefaultBodyLimit::max(ProjectBundle::MAX_BYTES)),
        )
    }

    async fn import_project(
        Project(project): Project,
        ApiBytes(bundle): ApiBytes,
    ) -> Result<Json<ProjectImportSummary>, OperationError> {
        if bundle.is_empty() {
            return Err(OperationError::invalid_input(
                "choose a .spiral-project file to import",
            ));
        }
        let summary = project
            .projects()
            .import(project.session(), project.project(), &bundle)
            .await?;
        Ok(Json(summary))
    }
}
