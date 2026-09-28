//! HTTP endpoints for robots.

use super::discovery::SerialPortDiscovery;
use super::operations::RobotOperations;
use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, Project};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    routing::{delete, get, post, put},
};
use spiral_robotics_common::robots::config::RobotConfigSave;
use spiral_robotics_common::robots::{
    CameraStatus, HardwareStatus, ModeSelection, RobotGoal, RobotSummary, SerialPortSummary,
    TelemetryFrame,
};

pub(crate) struct RobotEndpoints;

impl RobotEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/serial-ports", get(Self::list_serial_ports))
            .route("/hardware/status", get(Self::fetch_hardware_status))
            .route("/robots/telemetry", get(Self::fetch_telemetry))
            .route("/robots", get(Self::list_robots))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/cameras/refresh", post(Self::refresh_cameras))
            .route("/robots/save", post(Self::save_robot))
            .route("/robots/{id}", delete(Self::delete_robot))
            .route("/robots/{id}/connect", post(Self::connect_robot))
            .route("/robots/{id}/disconnect", post(Self::disconnect_robot))
            .route("/robots/{id}/hold", post(Self::hold_robot))
            .route("/robots/{id}/release", post(Self::release_robot))
            .route("/robots/{id}/goal", post(Self::set_robot_goal))
            .route(
                "/robots/{id}/calibration/start",
                post(Self::start_calibration),
            )
            .route(
                "/robots/{id}/calibration/finish",
                post(Self::finish_calibration),
            )
            .route(
                "/robots/{id}/calibration/cancel",
                post(Self::cancel_calibration),
            )
            .route("/mode", put(Self::set_mode))
    }
    async fn list_serial_ports(
        Project(project): Project,
    ) -> Result<Json<Vec<SerialPortSummary>>, OperationError> {
        Ok(Json(SerialPortDiscovery::list(&project).await?))
    }
    async fn refresh_cameras(
        Project(project): Project,
    ) -> Result<Json<Vec<CameraStatus>>, OperationError> {
        Ok(Json(project.cameras().refresh().await?))
    }
    async fn fetch_telemetry(Project(project): Project) -> Json<TelemetryFrame> {
        Json(project.hardware().state().telemetry().clone())
    }
    async fn fetch_hardware_status(Project(project): Project) -> Json<HardwareStatus> {
        let hardware = project.hardware().state();
        let mut issues = project.cameras().issues();
        if let Some(issue) = hardware.recording_issue() {
            issues.push(issue.to_owned());
        }
        Json(HardwareStatus::new(
            hardware.mode_status().clone(),
            project.cameras().statuses(),
            issues,
        ))
    }
    async fn list_robots(
        Project(project): Project,
    ) -> Result<Json<Vec<RobotSummary>>, OperationError> {
        Ok(Json(RobotOperations::new(&project).list().await?))
    }
    async fn save_robot(
        Project(project): Project,
        ApiJson(request): ApiJson<RobotConfigSave>,
    ) -> Result<Json<RobotSummary>, OperationError> {
        Ok(Json(RobotOperations::new(&project).save(request).await?))
    }
    async fn delete_robot(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        RobotOperations::new(&project).delete(&id).await?;
        Ok(Json(()))
    }
    async fn connect_robot(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        RobotOperations::new(&project).connect(&id).await?;
        Ok(Json(()))
    }
    async fn disconnect_robot(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        project
            .hardware()
            .disconnect(&id)
            .await
            .map_err(OperationError::conflict)?;
        Ok(Json(()))
    }
    async fn hold_robot(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        project
            .hardware()
            .hold(&id)
            .await
            .map_err(OperationError::conflict)?;
        Ok(Json(()))
    }
    async fn release_robot(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        project
            .hardware()
            .release(&id)
            .await
            .map_err(OperationError::conflict)?;
        Ok(Json(()))
    }
    async fn set_robot_goal(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
        ApiJson(request): ApiJson<RobotGoal>,
    ) -> Result<Json<()>, OperationError> {
        let (normalized,) = request.into_parts();
        RobotOperations::new(&project)
            .set_goals(&id, normalized)
            .await?;
        Ok(Json(()))
    }
    async fn start_calibration(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        project
            .hardware()
            .start_calibration(&id)
            .await
            .map_err(OperationError::conflict)?;
        Ok(Json(()))
    }
    async fn finish_calibration(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<RobotSummary>, OperationError> {
        Ok(Json(
            RobotOperations::new(&project)
                .finish_calibration(&id)
                .await?,
        ))
    }
    async fn cancel_calibration(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, OperationError> {
        project
            .hardware()
            .cancel_calibration(&id)
            .await
            .map_err(OperationError::conflict)?;
        Ok(Json(()))
    }
    async fn set_mode(
        Project(project): Project,
        ApiJson(selection): ApiJson<ModeSelection>,
    ) -> Result<Json<()>, OperationError> {
        RobotOperations::new(&project).set_mode(selection).await?;
        Ok(Json(()))
    }
}
