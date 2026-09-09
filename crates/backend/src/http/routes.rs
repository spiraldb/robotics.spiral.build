//! Compose the feature-owned HTTP endpoints. No application behavior lives here.
use super::contract::ApiRoutes;
use crate::project_session::context::ProjectContext;
use axum::Router;
pub(super) struct StudioApi;
impl StudioApi {
    pub(super) fn router(project: ProjectContext) -> Router {
        Self::routes().into_router(project)
    }
    pub(super) fn routes() -> ApiRoutes {
        let mut routes = ApiRoutes::new();
        crate::project_session::http::ProjectEndpoints::register(&mut routes);
        crate::recording::http::EpisodeEndpoints::register(&mut routes);
        crate::training::http::TrainingEndpoints::register(&mut routes);
        crate::workflow::http::WorkflowEndpoints::register(&mut routes);
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
