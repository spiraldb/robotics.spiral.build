//! HTTP endpoints for settings.

use crate::http::delivery::{ApiJson, HttpFailure, ScopedProject};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use spiral_robotics_common::settings::{
    ConnectionTest, DataSettings, SettingsUpdate, SpiralSettings,
};

pub(crate) struct SettingsEndpoints;

impl SettingsEndpoints {
    pub(crate) fn router() -> Router<StudioContext> {
        Router::new()
            .route(
                "/settings/connection",
                get(Self::connection_settings).put(Self::update_connection),
            )
            .route(
                "/settings",
                get(Self::data_settings).put(Self::update_data_settings),
            )
            .route("/settings/spiral/test", post(Self::test_spiral_connection))
            .route("/settings/training/test", post(Self::test_training_runtime))
    }
    /// The saved connection names the project a browser starts on.
    async fn connection_settings(
        State(state): State<StudioContext>,
    ) -> Result<Json<SpiralSettings>, HttpFailure> {
        Ok(Json(state.projects().saved_settings()))
    }
    async fn update_connection(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(change): ApiJson<spiral_robotics_common::settings::SpiralConnectionUpdate>,
    ) -> Result<Json<SpiralSettings>, HttpFailure> {
        Ok(Json(commands.update_connection(change).await?))
    }
    async fn data_settings(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
    ) -> Result<Json<DataSettings>, HttpFailure> {
        let result = commands.settings().await?;
        Ok(Json(result))
    }
    async fn update_data_settings(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<SettingsUpdate>,
    ) -> Result<Json<DataSettings>, HttpFailure> {
        let result = commands.update_settings(request).await?;
        Ok(Json(result))
    }
    async fn test_spiral_connection(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
    ) -> Result<Json<ConnectionTest>, HttpFailure> {
        let result = commands.test_spiral().await?;
        Ok(Json(result))
    }
    async fn test_training_runtime(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
    ) -> Result<Json<ConnectionTest>, HttpFailure> {
        let result = commands.test_training_runtime().await?;
        Ok(Json(result))
    }
}
