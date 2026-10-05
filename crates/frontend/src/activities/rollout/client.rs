//! Rollout requests and their query invalidation policy. A rollout runs on
//! the robot connector, from a checkpoint Studio writes there first.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::robot_connector::RobotConnector;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::rollout::{
    POLICY_ENVELOPE_MEDIA_TYPE, RolloutConfig, RolloutSnapshot, StoredPolicy,
};
use std::future::Future;

pub(crate) struct RolloutClient {
    connector: RobotConnector,
    project: ProjectClient,
}
impl RolloutClient {
    pub(super) fn current() -> Self {
        Self {
            connector: RobotConnector::current(),
            project: ProjectClient::current(),
        }
    }
    /// The current rollout session, or an idle snapshot before the first one.
    pub(super) async fn session(&self) -> Result<RolloutSnapshot, CommandError> {
        self.connector
            .request(HttpRequest::new("GET", "/api/rollout"))
            .await
    }

    /// Write the checkpoint to the robot connector unless it already holds
    /// it, then roll it out there.
    pub(super) async fn start_rollout(
        &self,
        config: RolloutConfig,
    ) -> Result<RolloutSnapshot, CommandError> {
        let stored: Vec<StoredPolicy> = self
            .connector
            .request(HttpRequest::new("GET", "/api/policies"))
            .await?;
        if !stored.iter().any(|policy| {
            policy.run_id() == config.run_id()
                && policy.checkpoint_step() == config.checkpoint_step()
        }) {
            self.write_policy(config.run_id(), config.checkpoint_step())
                .await?;
        }
        self.connector
            .mutate(
                &[Rollout, RobotSetup, Telemetry],
                HttpRequest::new("POST", "/api/rollout").json(&config),
            )
            .await
    }

    /// Carry checkpoint `step` of `run` from Studio to the robot connector.
    async fn write_policy(&self, run: &str, step: u64) -> Result<StoredPolicy, CommandError> {
        let envelope = self
            .project
            .request_blob(
                HttpRequest::new(
                    "GET",
                    format!(
                        "/api/training/runs/{}/checkpoints/{step}/policy",
                        run.url_component()
                    ),
                ),
                POLICY_ENVELOPE_MEDIA_TYPE,
            )
            .await?;
        self.connector
            .request(
                HttpRequest::new(
                    "PUT",
                    format!("/api/policies/{}/{step}", run.url_component()),
                )
                .blob(envelope),
            )
            .await
    }

    pub(super) fn stop_rollout(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[Rollout, RobotSetup, Telemetry],
            HttpRequest::new("POST", format!("/api/rollout/{}/stop", id.url_component())),
        )
    }
}
