//! Saved explorations: list, load by identity, and one revision-guarded save.
use super::commands::ExplorationCommands;
use crate::http::{
    contract::{ApiRoutes, OperationSchema, RequestSchema},
    delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject},
};
use axum::{
    Json,
    extract::{FromRequest, Request},
    http::{StatusCode, header},
};
use spiral_robotics_common::exploration::{
    ExplorationDocument, ExplorationSummary, MAX_EXPLORATION_PAYLOAD_BYTES,
};

/// The body carries the payload as a JSON string, so escaping and the
/// surrounding fields both add bytes to it. Bounding the request at twice the
/// payload limit keeps every document the contract allows acceptable; one past
/// that limit is still reported as invalid input, not as an oversized request.
const MAX_SAVE_BODY_BYTES: usize = 2 * MAX_EXPLORATION_PAYLOAD_BYTES;

pub(crate) struct ExplorationEndpoints;
impl ExplorationEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/explorations", Self::list);
        routes.get("/explorations/{id}", Self::load);
        routes.post("/explorations/save", Self::save);
    }
    async fn list(
        ScopedProject(commands): ScopedProject<ExplorationCommands>,
    ) -> Result<Json<Vec<ExplorationSummary>>, HttpFailure> {
        Ok(Json(commands.list().await?))
    }
    async fn load(
        ScopedProject(commands): ScopedProject<ExplorationCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<ExplorationDocument>, HttpFailure> {
        Ok(Json(commands.load(&id).await?))
    }
    async fn save(
        ScopedProject(commands): ScopedProject<ExplorationCommands>,
        SubmittedExploration(document): SubmittedExploration,
    ) -> Result<Json<ExplorationDocument>, HttpFailure> {
        Ok(Json(commands.save(document).await?))
    }
}

/// An exploration payload reaches 32 MiB, far past the default JSON body
/// bound, so this route reads its own body. The documented request stays a
/// plain `ExplorationDocument`; only the bound differs from every other JSON
/// endpoint.
struct SubmittedExploration(ExplorationDocument);

impl<State: Send + Sync> FromRequest<State> for SubmittedExploration {
    type Rejection = HttpFailure;
    async fn from_request(request: Request, _: &State) -> Result<Self, Self::Rejection> {
        Self::read(request, MAX_SAVE_BODY_BYTES).await
    }
}

impl SubmittedExploration {
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
                "The exploration exceeds the request limit",
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
                chunk.map_err(|_| HttpFailure::invalid("Could not read the exploration"))?;
            if chunk.len() > limit.saturating_sub(body.len()) {
                return Err(oversized());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map(Self).map_err(|error| {
            // Well-formed JSON that is not an exploration is a contract
            // disagreement, matching how every other JSON route reports it.
            let status = match error.classify() {
                serde_json::error::Category::Data => StatusCode::UNPROCESSABLE_ENTITY,
                _ => StatusCode::BAD_REQUEST,
            };
            HttpFailure::rejection(status, error.to_string())
        })
    }
}

impl RequestSchema for SubmittedExploration {
    fn describe(operation: &mut OperationSchema, schemas: &mut schemars::SchemaGenerator) {
        <ApiJson<ExplorationDocument> as RequestSchema>::describe(operation, schemas);
    }
}

#[cfg(test)]
mod tests;
