//! Rollout endpoints.
use crate::http::contract::ApiRoutes;
use crate::http::delivery::{ApiJson, HttpFailure, ScopedProject};
use axum::Json;
use spiral_robotics_common::rollout::{RolloutConfig, RolloutSnapshot};
pub(crate) struct RolloutEndpoints;
impl RolloutEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/rollout", Self::fetch_rollout);
        routes.post("/rollout", Self::start_rollout);
        routes.post("/rollout/stop", Self::stop_rollout);
    }
    async fn start_rollout(
        ScopedProject(commands): ScopedProject<crate::rollout::commands::RolloutCommands>,
        ApiJson(config): ApiJson<RolloutConfig>,
    ) -> Result<Json<RolloutSnapshot>, HttpFailure> {
        let result = commands.start_rollout(config).await?;
        Ok(Json(result))
    }
    async fn stop_rollout(
        ScopedProject(commands): ScopedProject<crate::rollout::commands::RolloutCommands>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.stop_rollout().await?;
        Ok(Json(()))
    }
    async fn fetch_rollout(
        ScopedProject(commands): ScopedProject<crate::rollout::commands::RolloutCommands>,
    ) -> Result<Json<RolloutSnapshot>, HttpFailure> {
        let result = commands.snapshot()?;
        Ok(Json(result))
    }
}
