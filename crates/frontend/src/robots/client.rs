//! Robot requests to the robot connector, and their query invalidation policy.
use crate::client::http::UrlComponent;
use crate::client::queries::QueryDomain::*;
use crate::client::robot_connector::RobotConnector;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::robots::config::RobotConfigSave;
use spiral_robotics_common::robots::visualization::{RobotModelFiles, RobotModelId};
use spiral_robotics_common::robots::{
    CameraStatus, ModeSelection, RobotGoal, RobotSummary, SerialPortSummary,
};
use std::collections::BTreeMap;
use std::future::Future;

pub(crate) struct RobotClient {
    connector: RobotConnector,
}
impl RobotClient {
    pub(crate) fn current() -> Self {
        Self {
            connector: RobotConnector::current(),
        }
    }
    pub(super) fn telemetry_snapshot(
        &self,
    ) -> impl Future<Output = Result<spiral_robotics_common::robots::TelemetryFrame, CommandError>> + use<>
    {
        self.connector
            .request(HttpRequest::new("GET", "/api/robots/telemetry"))
    }

    pub(super) fn robots(
        &self,
    ) -> impl Future<Output = Result<Vec<RobotSummary>, CommandError>> + use<> {
        self.connector
            .request(HttpRequest::new("GET", "/api/robots"))
    }

    /// A stored robot model, which never changes under its address.
    pub(super) fn robot_model(
        &self,
        model: &RobotModelId,
    ) -> impl Future<Output = Result<RobotModelFiles, CommandError>> + use<> {
        self.connector.request(HttpRequest::new(
            "GET",
            format!("/api/robots/models/{model}"),
        ))
    }
    pub(super) fn hardware_status(
        &self,
    ) -> impl Future<Output = Result<spiral_robotics_common::robots::HardwareStatus, CommandError>> + use<>
    {
        self.connector
            .request(HttpRequest::new("GET", "/api/hardware/status"))
    }

    pub(crate) fn list_serial_ports(
        &self,
    ) -> impl Future<Output = Result<Vec<SerialPortSummary>, CommandError>> + use<> {
        self.connector
            .request(HttpRequest::new("GET", "/api/serial-ports"))
    }

    pub(crate) fn refresh_cameras(
        &self,
    ) -> impl Future<Output = Result<Vec<CameraStatus>, CommandError>> + use<> {
        self.connector.mutate(
            &[RobotSetup],
            HttpRequest::new("POST", "/api/cameras/refresh"),
        )
    }

    pub(crate) fn save_robot(
        &self,
        request: RobotConfigSave,
    ) -> impl Future<Output = Result<RobotSummary, CommandError>> + use<> {
        self.connector.mutate(
            &[RobotSetup, Catalog],
            HttpRequest::new("POST", "/api/robots/save").json(&request),
        )
    }

    /// Save the simulated leader and follower over any robots of their names,
    /// and connect both. Only a robot connector that simulates arms offers it.
    pub(crate) fn add_demo_bench(&self) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[RobotSetup, Catalog, Telemetry],
            HttpRequest::new("POST", "/api/demo-bench"),
        )
    }

    pub(crate) fn delete_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[RobotSetup, Catalog],
            HttpRequest::new(
                "DELETE",
                format!("/api/robots/{id}", id = id.url_component()),
            ),
        )
    }

    pub(crate) fn connect_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[RobotSetup, Telemetry],
            HttpRequest::new(
                "POST",
                format!("/api/robots/{id}/connect", id = id.url_component()),
            ),
        )
    }

    pub(crate) fn disconnect_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[RobotSetup, Telemetry],
            HttpRequest::new(
                "POST",
                format!("/api/robots/{id}/disconnect", id = id.url_component()),
            ),
        )
    }

    pub(crate) fn hold_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[Telemetry],
            HttpRequest::new(
                "POST",
                format!("/api/robots/{id}/hold", id = id.url_component()),
            ),
        )
    }

    pub(crate) fn release_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[Telemetry],
            HttpRequest::new(
                "POST",
                format!("/api/robots/{id}/release", id = id.url_component()),
            ),
        )
    }

    pub(crate) fn set_robot_goal(
        &self,
        id: String,
        normalized: BTreeMap<String, f64>,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[Telemetry],
            HttpRequest::new(
                "POST",
                format!("/api/robots/{id}/goal", id = id.url_component()),
            )
            .json(&RobotGoal::new(normalized)),
        )
    }

    pub(crate) fn start_calibration(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[Telemetry, RobotSetup],
            HttpRequest::new(
                "POST",
                format!(
                    "/api/robots/{id}/calibration/start",
                    id = id.url_component()
                ),
            ),
        )
    }

    pub(crate) fn finish_calibration(
        &self,
        id: String,
    ) -> impl Future<Output = Result<RobotSummary, CommandError>> + use<> {
        self.connector.mutate(
            &[Telemetry, RobotSetup],
            HttpRequest::new(
                "POST",
                format!(
                    "/api/robots/{id}/calibration/finish",
                    id = id.url_component()
                ),
            ),
        )
    }

    pub(crate) fn cancel_calibration(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[Telemetry, RobotSetup],
            HttpRequest::new(
                "POST",
                format!(
                    "/api/robots/{id}/calibration/cancel",
                    id = id.url_component()
                ),
            ),
        )
    }

    pub(crate) fn set_mode(
        &self,
        selection: ModeSelection,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[RobotSetup, Telemetry],
            HttpRequest::new("PUT", "/api/mode").json(&selection),
        )
    }
}
