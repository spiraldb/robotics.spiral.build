//! Rollout endpoints.
use crate::http::contract::ApiRoutes;
use crate::http::delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject};
use axum::Json;
use spiral_robotics_common::rollout::{RolloutConfig, RolloutSnapshot};
pub(crate) struct RolloutEndpoints;
impl RolloutEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/rollout-runs", Self::fetch_rollout);
        routes.get("/rollout-runs/{id}", Self::fetch_run);
        routes.post("/rollout-runs", Self::start_rollout);
        routes.post("/rollout-runs/{id}/stop", Self::stop_rollout);
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
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.stop_rollout(id).await?;
        Ok(Json(()))
    }
    async fn fetch_rollout(
        ScopedProject(commands): ScopedProject<crate::rollout::commands::RolloutCommands>,
    ) -> Result<Json<Vec<RolloutSnapshot>>, HttpFailure> {
        let result = commands.runs().await?;
        Ok(Json(result))
    }
    async fn fetch_run(
        ScopedProject(commands): ScopedProject<crate::rollout::commands::RolloutCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<RolloutSnapshot>, HttpFailure> {
        Ok(Json(commands.run(id).await?))
    }
}
