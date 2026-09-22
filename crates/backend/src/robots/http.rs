//! HTTP endpoints for robots.

use crate::http::contract::ApiRoutes;
use crate::http::delivery::{ApiJson, ApiPath, HttpFailure, ScopedProject};

use axum::Json;
use spiral_robotics_common::robots::{
    CameraStatus, ModeSelection, RobotGoal, RobotSummary, SerialPortSummary,
};

pub(crate) struct RobotEndpoints;

impl RobotEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/serial-ports", Self::list_serial_ports);
        routes.post("/cameras/refresh", Self::refresh_cameras);
        routes.get("/hardware/status", Self::fetch_hardware_status);
        routes.get("/robots/telemetry", Self::fetch_telemetry);
        routes.get("/robots", Self::list_robots);
        routes.post("/robots/save", Self::save_robot);
        routes.get("/robots/{id}", Self::get_robot);
        routes.delete("/robots/{id}", Self::delete_robot);
        routes.post("/robots/{id}/connect", Self::connect_robot);
        routes.post("/robots/{id}/disconnect", Self::disconnect_robot);
        routes.post("/robots/{id}/hold", Self::hold_robot);
        routes.post("/robots/{id}/release", Self::release_robot);
        routes.post("/robots/{id}/goal", Self::set_robot_goal);
        routes.post("/robots/{id}/calibration/start", Self::start_calibration);
        routes.post("/robots/{id}/calibration/finish", Self::finish_calibration);
        routes.post("/robots/{id}/calibration/cancel", Self::cancel_calibration);
        routes.put("/mode", Self::set_mode);
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
