//! Rollout requests and their query invalidation policy.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::rollout::{RolloutConfig, RolloutSnapshot};
use std::future::Future;

pub(crate) struct RolloutClient {
    project: ProjectClient,
}
impl RolloutClient {
    pub(super) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    /// The current rollout session, or an idle snapshot before the first one.
    pub(super) async fn session(&self) -> Result<RolloutSnapshot, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/rollout"))
            .await
    }

    pub(super) fn start_rollout(
        &self,
        config: RolloutConfig,
    ) -> impl Future<Output = Result<RolloutSnapshot, CommandError>> + use<> {
        self.project.mutate(
            &[Rollout, RobotSetup, Telemetry],
            HttpRequest::new("POST", "/api/rollout").json(&config),
        )
    }

    pub(super) fn stop_rollout(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[Rollout, RobotSetup, Telemetry],
            HttpRequest::new("POST", format!("/api/rollout/{}/stop", id.url_component())),
        )
    }
}
