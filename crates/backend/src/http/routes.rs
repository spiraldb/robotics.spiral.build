//! Compose the feature-owned HTTP endpoints. No application behavior lives here.
use super::delivery::HttpFailure;
use crate::OperationError;
use crate::projects::context::StudioContext;
use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use tower_http::set_header::SetResponseHeaderLayer;
pub(crate) struct StudioApi;
impl StudioApi {
    /// Every feature splits its routes into reads and commands, and one rule
    /// covers both: a read stays in its request, so Axum cancels it when the
    /// caller disconnects; a command runs to completion regardless, so a
    /// dropped connection never leaves a write half done.
    /// Every request must carry a live Spiral session.
    pub(crate) fn router(studio: StudioContext) -> Router {
        let reads = Router::new()
            .merge(crate::account::http::AccountEndpoints::reads())
            .merge(crate::projects::http::WorkspaceEndpoints::reads())
            .merge(crate::transport::http::TransportEndpoints::reads())
            .merge(crate::training::http::TrainingEndpoints::reads())
            .merge(crate::curation::http::CurationEndpoints::reads())
            .merge(crate::data::settings_http::SettingsEndpoints::reads())
            .merge(crate::data::http::DatasetQueryEndpoints::reads())
            .merge(crate::data::project_download::ProjectDownload::reads());
        let commands = Router::new()
            .merge(crate::account::http::AccountEndpoints::commands())
            .merge(crate::projects::http::ProjectEndpoints::commands())
            .merge(crate::projects::http::WorkspaceEndpoints::commands())
            .merge(crate::training::http::TrainingEndpoints::commands())
            .merge(crate::curation::http::CurationEndpoints::commands())
            .merge(crate::data::settings_http::SettingsEndpoints::commands())
            .merge(crate::data::http::DatasetQueryEndpoints::commands())
            .layer(middleware::from_fn(Self::run_to_completion));
        reads
            .merge(commands)
            .fallback(HttpFailure::not_found)
            .method_not_allowed_fallback(HttpFailure::method_not_allowed)
            .layer(middleware::from_fn_with_state(
                studio.clone(),
                crate::account::http::AccountEndpoints::require_session,
            ))
            .layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-store"),
            ))
            .with_state(studio)
    }

    async fn run_to_completion(request: Request, next: Next) -> Response {
        tokio::spawn(next.run(request))
            .await
            .unwrap_or_else(|error| {
                OperationError::Internal(format!("backend task failed: {error}")).into_response()
            })
    }
}
#[cfg(test)]
mod tests;
