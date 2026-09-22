//! HTTP endpoints for robots.

use crate::http::delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    routing::{get, post, put},
};
use spiral_robotics_common::robots::{
    CameraStatus, ModeSelection, RobotGoal, RobotSummary, SerialPortSummary,
};

pub(crate) struct RobotEndpoints;

impl RobotEndpoints {
    pub(crate) fn router() -> Router<StudioContext> {
        Router::new()
            .route("/serial-ports", get(Self::list_serial_ports))
            .route("/cameras/refresh", post(Self::refresh_cameras))
            .route("/hardware/status", get(Self::fetch_hardware_status))
            .route("/robots/telemetry", get(Self::fetch_telemetry))
            .route("/robots", get(Self::list_robots))
            .route("/robots/save", post(Self::save_robot))
            .route(
                "/robots/{id}",
                get(Self::get_robot).delete(Self::delete_robot),
            )
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
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
    ) -> Result<Json<Vec<SerialPortSummary>>, HttpFailure> {
        let result = commands.list_ports().await?;
        Ok(Json(result))
    }
    async fn refresh_cameras(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
    ) -> Result<Json<Vec<CameraStatus>>, HttpFailure> {
        let result = commands.refresh_cameras().await?;
        Ok(Json(result))
    }
    async fn fetch_telemetry(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
    ) -> Result<Json<spiral_robotics_common::robots::TelemetryFrame>, HttpFailure> {
        let result = commands.telemetry()?;
        Ok(Json(result))
    }
    async fn fetch_hardware_status(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
    ) -> Result<Json<spiral_robotics_common::robots::HardwareStatus>, HttpFailure> {
        let result = commands.hardware_status()?;
        Ok(Json(result))
    }
    async fn list_robots(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
    ) -> Result<Json<Vec<RobotSummary>>, HttpFailure> {
        let result = commands.list_robots().await?;
        Ok(Json(result))
    }
    async fn save_robot(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiJson(request): ApiJson<spiral_robotics_common::robots::config::RobotConfigSave>,
    ) -> Result<Json<RobotSummary>, HttpFailure> {
        Ok(Json(commands.save_robot(request).await?))
    }
    async fn get_robot(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<RobotSummary>, HttpFailure> {
        let result = commands.get_robot(id).await?;
        Ok(Json(result))
    }
    async fn delete_robot(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.delete_robot(id).await?;
        Ok(Json(()))
    }
    async fn connect_robot(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.connect_robot(id).await?;
        Ok(Json(()))
    }
    async fn disconnect_robot(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.disconnect_robot(id).await?;
        Ok(Json(()))
    }
    async fn hold_robot(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.hold_robot(id).await?;
        Ok(Json(()))
    }
    async fn release_robot(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.release_robot(id).await?;
        Ok(Json(()))
    }
    async fn set_robot_goal(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiPath(id): ApiPath<String>,
        ApiJson(request): ApiJson<RobotGoal>,
    ) -> Result<Json<()>, HttpFailure> {
        let (normalized,) = request.into_parts();
        commands.set_robot_goal(id, normalized).await?;
        Ok(Json(()))
    }
    async fn start_calibration(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.start_calibration(id).await?;
        Ok(Json(()))
    }
    async fn finish_calibration(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<RobotSummary>, HttpFailure> {
        let result = commands.finish_calibration(id).await?;
        Ok(Json(result))
    }
    async fn cancel_calibration(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.cancel_calibration(id).await?;
        Ok(Json(()))
    }

    async fn set_mode(
        ScopedProject(commands): ScopedProject<crate::robots::commands::DeviceCommands>,
        ApiJson(selection): ApiJson<ModeSelection>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.set_mode(selection).await?;
        Ok(Json(()))
    }
}
