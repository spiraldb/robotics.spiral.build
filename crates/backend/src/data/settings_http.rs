//! HTTP endpoints for settings.

use super::settings::UpdateDataSettingsCommand;
use crate::OperationError;
use crate::http::delivery::{ApiJson, Project};
use crate::projects::context::StudioContext;
use crate::robots::operations::RobotOperations;

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post, put},
};
use spiral_robotics_common::settings::{
    ConnectionTest, DataSettings, SettingsUpdate, SpiralConnectionUpdate, SpiralSettings,
};
use std::sync::Arc;

pub(crate) struct SettingsEndpoints;

impl SettingsEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/settings/connection", get(Self::connection_settings))
            .route("/settings", get(Self::data_settings))
            .route("/settings/spiral/test", post(Self::test_spiral_connection))
            .route("/settings/training/test", post(Self::test_training_runtime))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/settings/connection", put(Self::update_connection))
            .route("/settings", put(Self::update_data_settings))
    }
    /// The saved connection names the project a browser starts on.
    async fn connection_settings(State(state): State<StudioContext>) -> Json<SpiralSettings> {
        Json(state.projects().saved_settings())
    }
    async fn update_connection(
        Project(project): Project,
        ApiJson(change): ApiJson<SpiralConnectionUpdate>,
    ) -> Result<Json<SpiralSettings>, OperationError> {
        let origin = project.project();
        let switched = project.projects().update_connection(origin, change).await?;
        if !Arc::ptr_eq(&switched, origin) {
            RobotOperations::spawn_auto_connect(project.with_project(switched));
        }
        Ok(Json(project.projects().saved_settings()))
    }
    async fn data_settings(
        Project(project): Project,
    ) -> Result<Json<DataSettings>, OperationError> {
        Ok(Json(project.project().settings().await?))
    }
    async fn update_data_settings(
        Project(project): Project,
        ApiJson(request): ApiJson<SettingsUpdate>,
    ) -> Result<Json<DataSettings>, OperationError> {
        let update = project
            .projects()
            .update_settings(project.project(), UpdateDataSettingsCommand::from(request))
            .await?;
        let (settings, switched_to) = update.into_parts();
        if let Some(switched) = switched_to {
            RobotOperations::spawn_auto_connect(project.with_project(switched));
        }
        Ok(Json(settings))
    }
    async fn test_spiral_connection(Project(project): Project) -> Json<ConnectionTest> {
        Json(ConnectionTest::from(
            project.storage().database().test().await,
        ))
    }
    async fn test_training_runtime(Project(project): Project) -> Json<ConnectionTest> {
        Json(ConnectionTest::from(
            project.training().test_runtime().await,
        ))
    }
}
