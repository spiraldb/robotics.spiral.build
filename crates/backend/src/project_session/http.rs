//! HTTP endpoints for project.

use crate::http::contract::ApiRoutes;
use crate::http::delivery::{HttpFailure, ScopedProject};
use crate::project_session::context::ProjectContext;

use axum::Json;
use spiral_robotics_common::datasets::ProjectImportSummary;
use spiral_robotics_common::project::ProjectStatus;
use spiral_robotics_common::robots::TelemetryFrame;
use spiral_robotics_common::status::StudioSnapshot;

pub(crate) struct ProjectEndpoints;

impl ProjectEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.post("/project/import", Self::import_project);
        routes.get("/project/status", Self::project_status);
        routes.get("/project/setup", Self::fetch_setup);
        routes.get("/project/telemetry", Self::fetch_telemetry);
    }
    async fn project_status(
        axum::extract::State(state): axum::extract::State<ProjectContext>,
    ) -> Result<Json<ProjectStatus>, HttpFailure> {
        let result = crate::project_session::queries::ProjectQueries::from(state).status();
        Ok(Json(result))
    }
    async fn fetch_setup(
        ScopedProject(commands): ScopedProject<crate::project_session::queries::ProjectQueries>,
    ) -> Result<Json<StudioSnapshot>, HttpFailure> {
        let result = commands.setup().await?;
        Ok(Json(result))
    }
    async fn fetch_telemetry(
        ScopedProject(commands): ScopedProject<crate::project_session::queries::ProjectQueries>,
    ) -> Result<Json<TelemetryFrame>, HttpFailure> {
        let result = commands.telemetry()?;
        Ok(Json(result))
    }
}

impl ProjectEndpoints {
    async fn import_project(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        upload: ProjectUpload,
    ) -> Result<Json<ProjectImportSummary>, HttpFailure> {
        let document = upload
            .read(crate::data::commands::MAX_PROJECT_IMPORT_BYTES)
            .await?;
        // Resolve admission again after upload; a project may have changed while
        // the browser was sending bytes. No import runs in the replacement project.
        Ok(Json(commands.import_project(document).await?))
    }
}

/// A bounded byte stream; the router resolves project scope before constructing it.
struct ProjectUpload(axum::extract::Request);
impl ProjectUpload {
    async fn read(self, limit: usize) -> Result<Vec<u8>, HttpFailure> {
        use axum::http::{StatusCode, header};
        let content_type = self
            .0
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        if content_type != Some("application/vnd.spiral-robotics.project") {
            return Err(HttpFailure::rejection(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Expected application/vnd.spiral-robotics.project",
            ));
        }
        if self
            .0
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|size| size > limit as u64)
        {
            return Err(HttpFailure::rejection(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Project import exceeds the upload limit",
            ));
        }
        let mut stream = self.0.into_body().into_data_stream();
        let mut document = Vec::new();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| HttpFailure::invalid("Could not read the project upload"))?;
            if chunk.len() > limit.saturating_sub(document.len()) {
                return Err(HttpFailure::rejection(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Project import exceeds the upload limit",
                ));
            }
            document.extend_from_slice(&chunk);
        }
        Ok(document)
    }
}

impl axum::extract::FromRequest<ProjectContext> for ProjectUpload {
    type Rejection = HttpFailure;
    async fn from_request(
        request: axum::extract::Request,
        _: &ProjectContext,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(request))
    }
}
impl crate::http::contract::RequestSchema for ProjectUpload {
    fn describe(
        operation: &mut crate::http::contract::OperationSchema,
        _: &mut schemars::SchemaGenerator,
    ) {
        operation.upload("application/vnd.spiral-robotics.project");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        response::IntoResponse,
    };

    #[tokio::test]
    async fn upload_enforces_actual_stream_length_without_trusting_content_length() {
        for advertised in [None, Some("1")] {
            let mut request = Request::builder()
                .header("Content-Type", "application/vnd.spiral-robotics.project");
            if let Some(length) = advertised {
                request = request.header("Content-Length", length);
            }
            let chunks = futures_util::stream::iter([
                Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"1234")),
                Ok(bytes::Bytes::from_static(b"5678")),
            ]);
            let upload = ProjectUpload(request.body(Body::from_stream(chunks)).unwrap());
            let failure = upload.read(7).await.expect_err("stream exceeds the bound");
            assert_eq!(
                failure.into_response().status(),
                StatusCode::PAYLOAD_TOO_LARGE
            );
        }
        let request = Request::builder()
            .header("Content-Type", "application/vnd.spiral-robotics.project")
            .body(Body::from("12345678"))
            .unwrap();
        assert_eq!(ProjectUpload(request).read(8).await.unwrap(), b"12345678");
    }
}
