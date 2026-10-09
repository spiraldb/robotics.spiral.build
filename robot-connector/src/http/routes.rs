//! Compose the feature-owned HTTP endpoints. No connector behavior lives here.
use super::delivery::HttpFailure;
use crate::OperationError;
use crate::context::RobotConnectorContext;
use axum::{
    Json, Router,
    extract::Request,
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use spiral_robotics_common::robot_connector::RobotConnectorHealth;
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;

pub(super) struct RobotConnectorApi;
impl RobotConnectorApi {
    /// Every feature splits its routes into reads and commands, and one rule
    /// covers both: a read stays in its request, so Axum cancels it when the
    /// caller disconnects; a command runs to completion regardless, so a
    /// dropped connection never leaves a write or a hardware change half done.
    ///
    /// Studio runs on another origin — its hosted site or a developer's
    /// localhost — so every origin may call the connector.
    pub(super) fn router(connector: RobotConnectorContext) -> Router {
        let reads = Router::new()
            .route("/api/health", get(Self::health))
            .merge(crate::robots::http::RobotEndpoints::reads())
            .merge(crate::robots::camera_preview::CameraPreview::reads())
            .merge(crate::recording::http::RecordingEndpoints::reads())
            .merge(crate::policies::http::PolicyEndpoints::reads())
            .merge(crate::rollout::http::RolloutEndpoints::reads());
        let commands = Router::new()
            .merge(crate::robots::http::RobotEndpoints::commands())
            .merge(crate::recording::http::RecordingEndpoints::commands())
            .merge(crate::policies::http::PolicyEndpoints::commands())
            .merge(crate::rollout::http::RolloutEndpoints::commands())
            .layer(middleware::from_fn(Self::run_to_completion));
        reads
            .merge(commands)
            .fallback(HttpFailure::not_found)
            .method_not_allowed_fallback(HttpFailure::method_not_allowed)
            // Responses are live state unless an endpoint says otherwise.
            .layer(SetResponseHeaderLayer::if_not_present(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-store"),
            ))
            .layer(CorsLayer::very_permissive())
            .with_state(connector)
    }

    async fn health() -> Json<RobotConnectorHealth> {
        Json(RobotConnectorHealth::new(
            env!("CARGO_PKG_VERSION"),
            cfg!(feature = "simulation"),
        ))
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
