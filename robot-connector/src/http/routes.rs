//! Compose the feature-owned HTTP endpoints. No connector behavior lives here.
use super::delivery::HttpFailure;
use crate::OperationError;
use crate::context::RobotConnectorContext;
use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};

pub(super) struct RobotConnectorApi;
impl RobotConnectorApi {
    /// Every feature splits its routes into reads and commands, and one rule
    /// covers both: a read stays in its request, so Axum cancels it when the
    /// caller disconnects; a command runs to completion regardless, so a
    /// dropped connection never leaves a write or a hardware change half done.
    ///
    /// The router answers the browser requests Studio relays down the link;
    /// the connector listens on no port.
    pub(super) fn router(connector: RobotConnectorContext) -> Router {
        let reads = Router::new()
            .merge(crate::robots::http::RobotEndpoints::reads())
            .merge(crate::recording::http::RecordingEndpoints::reads());
        let commands = Router::new()
            .merge(crate::robots::http::RobotEndpoints::commands())
            .merge(crate::recording::http::RecordingEndpoints::commands())
            .layer(middleware::from_fn(Self::run_to_completion));
        reads
            .merge(commands)
            .fallback(HttpFailure::not_found)
            .method_not_allowed_fallback(HttpFailure::method_not_allowed)
            .with_state(connector)
    }

    async fn run_to_completion(request: Request, next: Next) -> Response {
        tokio::spawn(next.run(request))
            .await
            .unwrap_or_else(|error| {
                OperationError::Internal(format!("robot connector task failed: {error}"))
                    .into_response()
            })
    }
}
