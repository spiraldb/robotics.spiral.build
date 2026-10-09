//! HTTP endpoints for robots.

use super::discovery::SerialPortDiscovery;
use super::operations::RobotOperations;
use crate::OperationError;
use crate::context::RobotConnectorContext;
use crate::http::delivery::{ApiJson, ApiPath};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::header,
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use spiral_robotics_common::robots::config::RobotConfigSave;
use spiral_robotics_common::robots::visualization::{RobotModelFiles, RobotModelId};
use spiral_robotics_common::robots::{
    CameraStatus, HardwareStatus, ModeSelection, RobotGoal, RobotSummary, SerialPortSummary,
    TelemetryFrame,
};

/// A robot model's meshes, base64-encoded, stay well inside this.
const MAX_MODEL_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct RobotEndpoints;

impl RobotEndpoints {
    pub(crate) fn reads() -> Router<RobotConnectorContext> {
        Router::new()
            .route("/api/serial-ports", get(Self::list_serial_ports))
            .route("/api/hardware/status", get(Self::fetch_hardware_status))
            .route("/api/robots/telemetry", get(Self::fetch_telemetry))
            .route("/api/robots", get(Self::list_robots))
            .route("/api/robots/models/{model}", get(Self::fetch_model))
    }
    pub(crate) fn commands() -> Router<RobotConnectorContext> {
        let commands = Router::new()
            .route("/api/cameras/refresh", post(Self::refresh_cameras))
            .route("/api/robots/save", post(Self::save_robot))
            .route(
                "/api/robots/models",
                post(Self::save_model).layer(DefaultBodyLimit::max(MAX_MODEL_BYTES)),
            )
            .route("/api/robots/{id}", delete(Self::delete_robot))
            .route("/api/robots/{id}/connect", post(Self::connect_robot))
            .route("/api/robots/{id}/disconnect", post(Self::disconnect_robot))
            .route("/api/robots/{id}/hold", post(Self::hold_robot))
            .route("/api/robots/{id}/release", post(Self::release_robot))
            .route("/api/robots/{id}/goal", post(Self::set_robot_goal))
            .route(
                "/api/robots/{id}/calibration/start",
                post(Self::start_calibration),
            )
            .route(
                "/api/robots/{id}/calibration/finish",
                post(Self::finish_calibration),
            )
            .route(
                "/api/robots/{id}/calibration/cancel",
                post(Self::cancel_calibration),
            )
            .route("/api/mode", put(Self::set_mode));
        #[cfg(feature = "simulation")]
        let commands = commands.route("/api/demo-bench", post(Self::install_demo_bench));
        commands
    }
    async fn list_serial_ports(
        State(connector): State<RobotConnectorContext>,
    ) -> Result<Json<Vec<SerialPortSummary>>, OperationError> {
        Ok(Json(SerialPortDiscovery::list(&connector).await?))
    }
    async fn refresh_cameras(
        State(connector): State<RobotConnectorContext>,
    ) -> Result<Json<Vec<CameraStatus>>, OperationError> {
        Ok(Json(connector.cameras().refresh().await?))
    }
    async fn fetch_telemetry(
        State(connector): State<RobotConnectorContext>,
    ) -> Json<TelemetryFrame> {
        Json(connector.hardware().state().telemetry().clone())
    }
    async fn fetch_hardware_status(
        State(connector): State<RobotConnectorContext>,
    ) -> Json<HardwareStatus> {
        let hardware = connector.hardware().state();
        let mut issues = connector.cameras().issues();
        if let Some(issue) = hardware.recording_issue() {
            issues.push(issue.to_owned());
        }
        Json(HardwareStatus::new(
            hardware.mode_status().clone(),
            connector.cameras().statuses(),
            issues,
        ))
    }
    async fn list_robots(
        State(connector): State<RobotConnectorContext>,
    ) -> Result<Json<Vec<RobotSummary>>, OperationError> {
        Ok(Json(RobotOperations::new(&connector).list().await?))
    }
    /// A model never changes under its address, so the browser keeps it.
    async fn fetch_model(
        State(connector): State<RobotConnectorContext>,
        ApiPath(model): ApiPath<RobotModelId>,
    ) -> Result<impl IntoResponse, OperationError> {
        let bytes = connector.workspace().models().bytes(&model)?;
        Ok((
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            bytes,
        ))
    }
    async fn save_model(
        State(connector): State<RobotConnectorContext>,
        ApiJson(model): ApiJson<RobotModelFiles>,
    ) -> Result<Json<RobotModelId>, OperationError> {
        Ok(Json(connector.workspace().models().save(&model)?))
    }
    async fn save_robot(
        State(connector): State<RobotConnectorContext>,
        ApiJson(request): ApiJson<RobotConfigSave>,
    ) -> Result<Json<RobotSummary>, OperationError> {
        Ok(Json(RobotOperations::new(&connector).save(request).await?))
    }
    async fn delete_robot(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        RobotOperations::new(&connector).delete(&id).await?;
        Ok(Json(()))
    }
    #[cfg(feature = "simulation")]
    async fn install_demo_bench(
        State(connector): State<RobotConnectorContext>,
    ) -> Result<Json<()>, OperationError> {
        RobotOperations::new(&connector)
            .install_demo_bench()
            .await?;
        Ok(Json(()))
    }
    async fn connect_robot(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        RobotOperations::new(&connector).connect(&id).await?;
        Ok(Json(()))
    }
    async fn disconnect_robot(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        connector
            .hardware()
            .disconnect(&id)
            .await
            .map_err(OperationError::conflict)?;
        Ok(Json(()))
    }
    async fn hold_robot(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        connector
            .hardware()
            .hold(&id)
            .await
            .map_err(OperationError::conflict)?;
        Ok(Json(()))
    }
    async fn release_robot(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        connector
            .hardware()
            .release(&id)
            .await
            .map_err(OperationError::conflict)?;
        Ok(Json(()))
    }
    async fn set_robot_goal(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
        ApiJson(request): ApiJson<RobotGoal>,
    ) -> Result<Json<()>, OperationError> {
        let (normalized,) = request.into_parts();
        RobotOperations::new(&connector)
            .set_goals(&id, normalized)
            .await?;
        Ok(Json(()))
    }
    async fn start_calibration(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        connector
            .hardware()
            .start_calibration(&id)
            .await
            .map_err(OperationError::conflict)?;
        Ok(Json(()))
    }
    async fn finish_calibration(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<RobotSummary>, OperationError> {
        Ok(Json(
            RobotOperations::new(&connector)
                .finish_calibration(&id)
                .await?,
        ))
    }
    async fn cancel_calibration(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        connector
            .hardware()
            .cancel_calibration(&id)
            .await
            .map_err(OperationError::conflict)?;
        Ok(Json(()))
    }
    async fn set_mode(
        State(connector): State<RobotConnectorContext>,
        ApiJson(selection): ApiJson<ModeSelection>,
    ) -> Result<Json<()>, OperationError> {
        RobotOperations::new(&connector).set_mode(selection).await?;
        Ok(Json(()))
    }
}
