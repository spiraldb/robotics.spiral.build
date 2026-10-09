//! The robot connector this workspace drives: a program that runs next to the
//! arms and owns their hardware and recordings. It links itself to
//! Studio with a token the workspace issues, and the browser reaches it only
//! through Studio's relay, as it makes any other request of the workspace.

use std::future::Future;

use dioxus::prelude::*;
use serde::de::DeserializeOwned;
use spiral_robotics_common::robot_connector::{
    RELAY_PREFIX, RobotConnectorHealth, RobotConnectorToken,
};

use super::project::ProjectClient;
use super::queries::{QueryDomain, QueryPolicy};
use super::transport::{CommandError, HttpRequest};
use crate::shell::workspace::WorkspaceSettings;

#[derive(Clone, Copy)]
pub(crate) struct RobotConnector {
    /// False until the workspace's settings are read.
    token_issued: Memo<bool>,
    health: Signal<Option<Result<RobotConnectorHealth, CommandError>>>,
}

impl RobotConnector {
    pub(crate) fn use_provider(settings: WorkspaceSettings) -> Self {
        let token_issued = use_memo(move || {
            settings
                .loaded()
                .is_some_and(|settings| settings.robot_connector().token_issued())
        });
        let health = use_signal(|| None);
        use_context_provider(|| Self {
            token_issued,
            health,
        })
    }

    pub(crate) fn current() -> Self {
        consume_context()
    }

    /// Ask whether a robot connector is linked while `active`. Only views that
    /// drive hardware ask.
    pub(crate) fn use_health(self, active: ReadSignal<bool>) {
        let response = QueryPolicy::new(QueryDomain::RobotConnector, 2_000).use_when(
            move || *active.read(),
            move || self.request(HttpRequest::new("GET", "/health")),
        );
        let mut health = self.health;
        use_effect(move || {
            let result = response.read();
            if response.finished()
                && let Some(result) = result.as_ref()
                && health.peek().as_ref() != Some(result)
            {
                health.set(Some(result.clone()));
            }
        });
    }

    /// Whether the workspace has issued a token a robot connector can link
    /// with.
    pub(crate) fn token_issued(self) -> bool {
        *self.token_issued.read()
    }

    /// Issue a token for a robot connector to link with. It replaces the
    /// workspace's last one, unlinking the robot connector that used it, and
    /// this answer is the only one that holds it.
    pub(crate) fn issue_token(
        self,
    ) -> impl Future<Output = Result<RobotConnectorToken, CommandError>> + use<> {
        ProjectClient::current().mutate(
            &[QueryDomain::Settings, QueryDomain::RobotConnector],
            HttpRequest::new("POST", "/api/settings/robot-connector/token"),
        )
    }

    /// The linked robot connector's build when it last answered, or why none
    /// did.
    pub(crate) fn health(self) -> Option<Result<RobotConnectorHealth, CommandError>> {
        self.health.read().clone()
    }

    pub(crate) fn linked(self) -> bool {
        matches!(*self.health.read(), Some(Ok(_)))
    }

    /// Relay `request` to the robot connector. Its path is the robot
    /// connector's own below `/api`, such as `/robots`.
    pub(crate) fn request<T: DeserializeOwned + 'static>(
        self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<T, CommandError>> + use<T> {
        ProjectClient::current().request(request.under(RELAY_PREFIX))
    }

    /// Relay `request`, then refetch `domains`: a failure may follow partial
    /// robot connector work.
    pub(crate) fn mutate<T: DeserializeOwned + 'static>(
        self,
        domains: &'static [QueryDomain],
        request: HttpRequest,
    ) -> impl Future<Output = Result<T, CommandError>> + use<T> {
        ProjectClient::current().mutate(domains, request.under(RELAY_PREFIX))
    }

    /// A binary response of `media_type`, kept as a browser blob.
    pub(crate) fn request_blob(
        self,
        request: HttpRequest,
        media_type: &'static str,
    ) -> impl Future<Output = Result<web_sys::Blob, CommandError>> + use<> {
        ProjectClient::current().request_blob(request.under(RELAY_PREFIX), media_type)
    }
}
