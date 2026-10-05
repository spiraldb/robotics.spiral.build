//! HTTP endpoints for the policies Studio writes here.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    routing::{get, put},
};
use spiral_robotics_common::rollout::StoredPolicy;

use crate::OperationError;
use crate::context::RobotConnectorContext;
use crate::http::delivery::{ApiBytes, ApiPath};

/// A policy envelope stays below the inference crate's model limits.
const MAX_POLICY_BYTES: usize = 520 * 1024 * 1024;

pub(crate) struct PolicyEndpoints;

impl PolicyEndpoints {
    pub(crate) fn reads() -> Router<RobotConnectorContext> {
        Router::new().route("/api/policies", get(Self::list))
    }
    pub(crate) fn commands() -> Router<RobotConnectorContext> {
        Router::new().route(
            "/api/policies/{run}/{step}",
            put(Self::write)
                .delete(Self::delete)
                .layer(DefaultBodyLimit::max(MAX_POLICY_BYTES)),
        )
    }
    async fn list(
        State(connector): State<RobotConnectorContext>,
    ) -> Result<Json<Vec<StoredPolicy>>, OperationError> {
        Ok(Json(connector.policies().list().await?))
    }
    async fn write(
        State(connector): State<RobotConnectorContext>,
        ApiPath((run, step)): ApiPath<(String, u64)>,
        ApiBytes(envelope): ApiBytes,
    ) -> Result<Json<StoredPolicy>, OperationError> {
        Ok(Json(
            connector.policies().save(&run, step, &envelope).await?,
        ))
    }
    async fn delete(
        State(connector): State<RobotConnectorContext>,
        ApiPath((run, step)): ApiPath<(String, u64)>,
    ) -> Result<Json<()>, OperationError> {
        connector.policies().delete(&run, step).await?;
        Ok(Json(()))
    }
}
