//! HTTP endpoints for project.

use crate::OperationError;
use crate::http::delivery::{ApiBytes, Project};
use crate::projects::context::StudioContext;

use axum::{Json, Router, extract::DefaultBodyLimit, routing::post};
use spiral_robotics_common::datasets::ProjectImportSummary;
use spiral_robotics_storage::bundle::ProjectBundle;

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
