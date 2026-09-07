//! HTTP endpoints for settings.

use crate::http::contract::ApiRoutes;
use crate::http::delivery::{ApiJson, HttpFailure, ScopedProject};

use axum::Json;
use spiral_robotics_common::settings::{ConnectionTest, DataSettings, SettingsUpdate};

pub(crate) struct SettingsEndpoints;

impl SettingsEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/settings", Self::data_settings);
        routes.put("/settings", Self::update_data_settings);
        routes.post("/settings/spiral/test", Self::test_spiral_connection);
        routes.post("/settings/training/test", Self::test_training_runtime);
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
