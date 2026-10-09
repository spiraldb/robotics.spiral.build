//! Policy requests and their query invalidation policy. A policy runs on the
//! robot connector, from a checkpoint Studio writes there first.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::robot_connector::RobotConnector;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::rollout::{
    POLICY_ENVELOPE_MEDIA_TYPE, RolloutConfig, RolloutSnapshot, StoredPolicy,
};
use std::future::Future;

pub(in crate::activities::rollout) struct PolicyClient {
    connector: RobotConnector,
    project: ProjectClient,
}
impl PolicyClient {
    pub(in crate::activities::rollout) fn current() -> Self {
        Self {
            connector: RobotConnector::current(),
            project: ProjectClient::current(),
        }
    }
    /// The current policy run, or an idle snapshot before the first one.
    pub(super) async fn run(&self) -> Result<RolloutSnapshot, CommandError> {
        self.connector
            .request(HttpRequest::new("GET", "/rollout"))
            .await
    }

    /// Write the checkpoint to the robot connector, then run it there.
    pub(in crate::activities::rollout) async fn start(
        &self,
        config: RolloutConfig,
    ) -> Result<(), CommandError> {
        self.store(&config).await?;
        self.connector
            .mutate::<RolloutSnapshot>(
                &[Rollout, RobotSetup, Telemetry],
                HttpRequest::new("POST", "/rollout").json(&config),
            )
            .await
            .map(|_| ())
    }

    /// Carry `config`'s checkpoint from Studio to the robot connector unless
    /// it already holds it.
    pub(in crate::activities::rollout) async fn store(
        &self,
        config: &RolloutConfig,
    ) -> Result<(), CommandError> {
        let stored: Vec<StoredPolicy> = self
            .connector
            .request(HttpRequest::new("GET", "/policies"))
            .await?;
        if stored.iter().any(|policy| {
            policy.run_id() == config.run_id()
                && policy.checkpoint_step() == config.checkpoint_step()
        }) {
            return Ok(());
        }
        let (run, step) = (config.run_id(), config.checkpoint_step());
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
            .request::<StoredPolicy>(
                HttpRequest::new("PUT", format!("/policies/{}/{step}", run.url_component()))
                    .blob(envelope),
            )
            .await
            .map(|_| ())
    }

    /// Stop run `id`; the follower holds where the policy left it.
    pub(in crate::activities::rollout) fn stop(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[Rollout, RobotSetup, Telemetry],
            HttpRequest::new("POST", format!("/rollout/{}/stop", id.url_component())),
        )
    }
}
