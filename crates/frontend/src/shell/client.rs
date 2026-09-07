//! Studio requests and their query invalidation policy.
use crate::client::project::ProjectClient;
use crate::client::transport::{CommandError, HttpRequest};
use std::future::Future;

pub(crate) struct StudioClient {
    project: ProjectClient,
}
impl StudioClient {
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) fn setup_snapshot(
        &self,
    ) -> impl Future<Output = Result<spiral_robotics_common::status::StudioSnapshot, CommandError>> + use<>
    {
        self.project
            .request(HttpRequest::new("GET", "/api/project/setup"))
    }

    pub(crate) fn telemetry_snapshot(
        &self,
    ) -> impl Future<Output = Result<spiral_robotics_common::robots::TelemetryFrame, CommandError>> + use<>
    {
        self.project
            .request(HttpRequest::new("GET", "/api/project/telemetry"))
    }
}
