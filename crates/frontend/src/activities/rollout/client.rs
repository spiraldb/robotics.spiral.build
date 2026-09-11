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
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) async fn runs(&self) -> Result<Vec<RolloutSnapshot>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/rollout-runs"))
            .await
    }
    pub(crate) async fn run(&self, id: &str) -> Result<RolloutSnapshot, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                format!("/api/rollout-runs/{}", id.url_component()),
            ))
            .await
    }

    pub(crate) fn start_rollout(
        &self,
        config: RolloutConfig,
    ) -> impl Future<Output = Result<RolloutSnapshot, CommandError>> + use<> {
        self.project.mutate(
            &[Rollout, RobotSetup, Telemetry],
            HttpRequest::new("POST", "/api/rollout-runs").json(&config),
        )
    }

    pub(crate) fn stop_rollout(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[Rollout, RobotSetup, Telemetry],
            HttpRequest::new(
                "POST",
                format!("/api/rollout-runs/{}/stop", id.url_component()),
            ),
        )
    }
}
