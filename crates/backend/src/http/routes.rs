//! Compose the feature-owned HTTP endpoints. No application behavior lives here.
use super::contract::ApiRoutes;
use crate::projects::context::StudioContext;
use axum::Router;
pub(super) struct StudioApi;
impl StudioApi {
    pub(super) fn router(studio: StudioContext) -> Router {
        Self::routes().into_router(studio)
    }
    pub(super) fn routes() -> ApiRoutes {
        let mut routes = ApiRoutes::new();
        crate::projects::http::ProjectEndpoints::register(&mut routes);
        crate::transport::http::TransportEndpoints::register(&mut routes);
        crate::recording::http::RecordingEndpoints::register(&mut routes);
        crate::training::http::TrainingEndpoints::register(&mut routes);
        crate::workflow::http::WorkflowEndpoints::register(&mut routes);
        crate::curation::http::CurationEndpoints::register(&mut routes);
        crate::exploration::http::ExplorationEndpoints::register(&mut routes);
        crate::robots::http::RobotEndpoints::register(&mut routes);
        crate::data::settings_http::SettingsEndpoints::register(&mut routes);
        crate::data::http::DatasetQueryEndpoints::register(&mut routes);
        crate::data::project_download::ProjectDownload::register(&mut routes);
        crate::robots::camera_preview::CameraPreview::register(&mut routes);
        crate::rollout::http::RolloutEndpoints::register(&mut routes);
        routes
    }
}
#[cfg(test)]
mod tests;
