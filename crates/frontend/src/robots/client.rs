//! Robot requests and their query invalidation policy.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::robots::{
    CameraBinding, CameraStatus, ModeSelection, NewRobot, RobotGoal, RobotSummary, RobotUpdate,
    SerialPortIdentification, SerialPortScan, SerialPortSummary,
};
use std::collections::BTreeMap;
use std::future::Future;

pub(crate) struct RobotClient {
    project: ProjectClient,
}
impl RobotClient {
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) fn telemetry_snapshot(
        &self,
    ) -> impl Future<Output = Result<spiral_robotics_common::robots::TelemetryFrame, CommandError>> + use<>
    {
        self.project
            .request(HttpRequest::new("GET", "/api/robots/telemetry"))
    }

    pub(crate) fn setup_snapshot(
        &self,
    ) -> impl Future<
        Output = Result<spiral_robotics_common::robots::RobotSetupSnapshot, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/robots/setup"))
    }

    pub(crate) fn list_serial_ports(
        &self,
    ) -> impl Future<Output = Result<Vec<SerialPortSummary>, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/serial-ports"))
    }

    pub(crate) fn identify_serial_port(
        &self,
        before: Vec<String>,
    ) -> impl Future<Output = Result<SerialPortIdentification, CommandError>> + use<> {
        self.project.request(
            HttpRequest::new("POST", "/api/serial-ports/identify")
                .json(&SerialPortScan::new(before)),
        )
    }

    pub(crate) fn refresh_cameras(
        &self,
    ) -> impl Future<Output = Result<Vec<CameraStatus>, CommandError>> + use<> {
        self.project.mutate(
            &[RobotSetup],
            HttpRequest::new("POST", "/api/cameras/refresh"),
        )
    }

    pub(crate) fn create_robot(
        &self,
        request: NewRobot,
    ) -> impl Future<Output = Result<RobotSummary, CommandError>> + use<> {
        self.project.mutate(
            &[RobotSetup, Catalog],
            HttpRequest::new("POST", "/api/robots").json(&request),
        )
    }

    pub(crate) fn import_robot_configuration(
        &self,
        document: Vec<u8>,
    ) -> impl Future<Output = Result<RobotSummary, CommandError>> + use<> {
        self.project.mutate(
            &[RobotSetup, Catalog],
            HttpRequest::new("POST", "/api/robots/import").raw_json(document),
        )
    }

    pub(crate) fn update_robot(
        &self,
        request: RobotUpdate<Vec<CameraBinding>>,
    ) -> impl Future<Output = Result<RobotSummary, CommandError>> + use<> {
        self.project.mutate(
            &[RobotSetup],
            HttpRequest::new("PATCH", "/api/robots").json(&request),
        )
    }

    pub(crate) fn delete_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
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
        self.project.mutate(
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
        self.project.mutate(
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
        self.project.mutate(
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
        self.project.mutate(
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
        self.project.mutate(
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
        self.project.mutate(
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
        self.project.mutate(
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
        self.project.mutate(
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
        self.project.mutate(
            &[RobotSetup, Telemetry],
            HttpRequest::new("PUT", "/api/mode").json(&selection),
        )
    }
}
