//! Rollout endpoints.
use crate::OperationError;
use crate::context::RobotConnectorContext;
use crate::http::delivery::{ApiJson, ApiPath};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use spiral_robotics_common::rollout::{RolloutConfig, RolloutSnapshot};
pub(crate) struct RolloutEndpoints;
impl RolloutEndpoints {
    pub(crate) fn reads() -> Router<RobotConnectorContext> {
        Router::new().route("/api/rollout", get(Self::fetch_rollout))
    }
    pub(crate) fn commands() -> Router<RobotConnectorContext> {
        Router::new()
            .route("/api/rollout", post(Self::start_rollout))
            .route("/api/rollout/{id}/stop", post(Self::stop_rollout))
    }
    /// Roll out a policy Studio wrote here; loading it is what proves it exists.
    async fn start_rollout(
        State(connector): State<RobotConnectorContext>,
        ApiJson(config): ApiJson<RolloutConfig>,
    ) -> Result<Json<RolloutSnapshot>, OperationError> {
        let snapshot = connector
            .rollout()
            .start(config)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(snapshot))
    }
    async fn stop_rollout(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        connector
            .rollout()
            .stop(&id)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(()))
    }
    /// The current rollout session, or an idle snapshot before the first one.
    async fn fetch_rollout(
        State(connector): State<RobotConnectorContext>,
    ) -> Json<RolloutSnapshot> {
        Json(connector.rollout().snapshot())
    }
}
