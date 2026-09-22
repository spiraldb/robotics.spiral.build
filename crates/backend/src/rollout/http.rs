//! Rollout endpoints.
use crate::http::delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject};
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    routing::{get, post},
};
use spiral_robotics_common::rollout::{RolloutConfig, RolloutSnapshot};
pub(crate) struct RolloutEndpoints;
impl RolloutEndpoints {
    pub(crate) fn router() -> Router<StudioContext> {
        Router::new()
            .route(
                "/rollout-runs",
                get(Self::fetch_rollout).post(Self::start_rollout),
            )
            .route("/rollout-runs/{id}", get(Self::fetch_run))
            .route("/rollout-runs/{id}/stop", post(Self::stop_rollout))
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
