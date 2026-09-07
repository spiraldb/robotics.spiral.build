//! Rollout requests and their query invalidation policy.
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
    pub(crate) fn rollout_snapshot(
        &self,
    ) -> impl Future<Output = Result<RolloutSnapshot, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/rollout"))
    }

    pub(crate) fn start_rollout(
        &self,
        config: RolloutConfig,
    ) -> impl Future<Output = Result<RolloutSnapshot, CommandError>> + use<> {
        self.project.mutate(
            &[Rollout, Setup, Telemetry],
            HttpRequest::new("POST", "/api/rollout").json(&config),
        )
    }

    pub(crate) fn stop_rollout(&self) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[Rollout, Setup, Telemetry],
            HttpRequest::new("POST", "/api/rollout/stop"),
        )
    }
}
