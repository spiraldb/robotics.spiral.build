//! Browser-native project bundle download.

use super::*;
use crate::data_commands::DataCommands;

impl AppState {
    /// Downloads stay HTTP so the browser can stream bytes to a native file.
    pub(super) fn data_transfer_router() -> Router<Self> {
        Router::new().route("/project/export", get(Self::export_project))
    }

    async fn export_project(
        Extension(commands): Extension<DataCommands>,
        Query(query): Query<ProjectQuery>,
    ) -> Result<Response, (StatusCode, String)> {
        let commands = commands
            .for_session(&query.session_id)
            .map_err(|error| (StatusCode::CONFLICT, error.to_string()))?;
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
