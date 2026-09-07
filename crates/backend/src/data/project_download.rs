//! Browser-native project bundle download.

use crate::http::contract::{ApiRoutes, MediaEndpoint};
use crate::project_session::context::ProjectContext;
use axum::{
    body::Body,
    extract::{Query, State},
    http::{StatusCode, header},
    response::Response,
    routing::get,
};
use serde::Deserialize;

use crate::data::commands::DataCommands;

pub(crate) struct ProjectDownload;

impl ProjectDownload {
    /// Downloads stay HTTP so the browser can stream bytes to a native file.
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.media("/project/export", MediaEndpoint::new(get(Self::export_project), serde_json::json!({"get": {
            "operationId": "export_project", "parameters": [{"name": "session_id", "in": "query", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "Project bundle download", "content": {"application/vnd.spiral-robotics.project": {"schema": {"type": "string", "format": "binary"}}}}}
        }})["get"].take()));
    }

    async fn export_project(
        State(state): State<ProjectContext>,
        Query(query): Query<ProjectQuery>,
    ) -> Result<Response, (StatusCode, String)> {
        let commands = DataCommands::from(
            state
                .for_session(&query.session_id)
                .map_err(|error| (StatusCode::CONFLICT, error.to_string()))?,
        );
        let bytes = commands
            .export_project()
            .await
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        Response::builder()
            .status(StatusCode::OK)
            .header(
                header::CONTENT_TYPE,
                "application/vnd.spiral-robotics.project",
            )
            .header(
                header::CONTENT_DISPOSITION,
                "attachment; filename=spiral-robotics-project.spiral-project",
            )
            .header(header::CONTENT_LENGTH, bytes.len())
            .header(header::CACHE_CONTROL, "private, no-store")
            .body(Body::from(bytes))
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
    }
}

#[derive(Deserialize)]
struct ProjectQuery {
    session_id: String,
}
