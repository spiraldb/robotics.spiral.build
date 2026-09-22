//! Compose the feature-owned HTTP endpoints. No application behavior lives here.
use super::delivery::HttpFailure;
use crate::projects::context::StudioContext;
use axum::{
    Router,
    http::{HeaderValue, header},
};
use tower_http::set_header::SetResponseHeaderLayer;
pub(super) struct StudioApi;
impl StudioApi {
    pub(super) fn router(studio: StudioContext) -> Router {
        Router::new()
            .merge(crate::projects::http::ProjectEndpoints::router())
            .merge(crate::transport::http::TransportEndpoints::router())
            .merge(crate::recording::http::RecordingEndpoints::router())
            .merge(crate::training::http::TrainingEndpoints::router())
            .merge(crate::workflow::http::WorkflowEndpoints::router())
            .merge(crate::curation::http::CurationEndpoints::router())
            .merge(crate::exploration::http::ExplorationEndpoints::router())
            .merge(crate::robots::http::RobotEndpoints::router())
            .merge(crate::data::settings_http::SettingsEndpoints::router())
            .merge(crate::data::http::DatasetQueryEndpoints::router())
            .merge(crate::data::project_download::ProjectDownload::router())
            .merge(crate::robots::camera_preview::CameraPreview::router())
            .merge(crate::rollout::http::RolloutEndpoints::router())
            .fallback(HttpFailure::not_found)
            .method_not_allowed_fallback(HttpFailure::method_not_allowed)
            .layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-store"),
            ))
            .with_state(studio)
    }
}
#[cfg(test)]
mod tests;
