//! Saved Build documents: list, load by identity, and one revision-guarded save.
use super::commands::BuildCommands;
use crate::http::{
    contract::{ApiRoutes, OperationSchema, RequestSchema},
    delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject},
};
use axum::{
    Json,
    extract::{FromRequest, Request},
    http::{StatusCode, header},
};
use spiral_robotics_common::build::{BuildDocument, BuildDocumentSummary, MAX_BUILD_PAYLOAD_BYTES};

/// The body carries the payload as a JSON string, so escaping and the
/// surrounding fields both add bytes to it. Bounding the request at twice the
/// payload limit keeps every document the contract allows acceptable; one past
/// that limit is still reported as invalid input, not as an oversized request.
const MAX_SAVE_BODY_BYTES: usize = 2 * MAX_BUILD_PAYLOAD_BYTES;

pub(crate) struct BuildEndpoints;
impl BuildEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/build-documents", Self::list);
        routes.get("/build-documents/{id}", Self::load);
        routes.post("/build-documents/save", Self::save);
    }
    async fn list(
        ScopedProject(commands): ScopedProject<BuildCommands>,
    ) -> Result<Json<Vec<BuildDocumentSummary>>, HttpFailure> {
        Ok(Json(commands.list().await?))
    }
    async fn load(
        ScopedProject(commands): ScopedProject<BuildCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<BuildDocument>, HttpFailure> {
        Ok(Json(commands.load(&id).await?))
    }
    async fn save(
        ScopedProject(commands): ScopedProject<BuildCommands>,
        SubmittedBuildDocument(document): SubmittedBuildDocument,
    ) -> Result<Json<BuildDocument>, HttpFailure> {
        Ok(Json(commands.save(document).await?))
    }
}

/// A Build document reaches 32 MiB, far past the default JSON body bound, so
/// this route reads its own body. The documented request stays a plain
/// `BuildDocument`; only the bound differs from every other JSON endpoint.
struct SubmittedBuildDocument(BuildDocument);

impl<State: Send + Sync> FromRequest<State> for SubmittedBuildDocument {
    type Rejection = HttpFailure;
    async fn from_request(request: Request, _: &State) -> Result<Self, Self::Rejection> {
        Self::read(request, MAX_SAVE_BODY_BYTES).await
    }
}

impl SubmittedBuildDocument {
    /// The advertised length and the streamed bytes are both held to `limit`;
    /// neither alone describes what a client actually sends.
    async fn read(request: Request, limit: usize) -> Result<Self, HttpFailure> {
        let json = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"));
        if !json {
            return Err(HttpFailure::rejection(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Expected application/json",
            ));
        }
        let oversized = || {
            HttpFailure::rejection(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Build document exceeds the request limit",
            )
        };
        if request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|size| size > limit as u64)
        {
            return Err(oversized());
        }
        let mut stream = request.into_body().into_data_stream();
        let mut body = Vec::new();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| HttpFailure::invalid("Could not read the Build document"))?;
            if chunk.len() > limit.saturating_sub(body.len()) {
                return Err(oversized());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map(Self).map_err(|error| {
            // Well-formed JSON that is not a Build document is a contract
            // disagreement, matching how every other JSON route reports it.
            let status = match error.classify() {
                serde_json::error::Category::Data => StatusCode::UNPROCESSABLE_ENTITY,
                _ => StatusCode::BAD_REQUEST,
            };
            HttpFailure::rejection(status, error.to_string())
        })
    }
}

impl RequestSchema for SubmittedBuildDocument {
    fn describe(operation: &mut OperationSchema, schemas: &mut schemars::SchemaGenerator) {
        <ApiJson<BuildDocument> as RequestSchema>::describe(operation, schemas);
    }
}

#[cfg(test)]
mod tests;
