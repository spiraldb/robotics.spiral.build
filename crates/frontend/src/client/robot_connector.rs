//! The robot connector this workspace drives: an HTTP endpoint that runs next
//! to the arms and owns their hardware, recordings, and rollouts. The browser
//! talks to it directly, at the address the workspace keeps: `localhost` is
//! each viewer's own machine, and a network address one robot connector
//! everyone in the workspace shares.

use std::future::Future;

use dioxus::prelude::*;
use serde::de::DeserializeOwned;
use spiral_robotics_common::robot_connector::RobotConnectorHealth;
use spiral_robotics_common::settings::{DataSettings, RobotConnectorSettings};

use super::project::ProjectClient;
use super::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use super::transport::{CommandError, HttpRequest};
use crate::shell::workspace::WorkspaceSettings;

const UNREACHABLE: &str = "Could not reach the robot connector. Check that it is running and its address in Rollout's Setup.";

#[derive(Clone, Copy)]
pub(crate) struct RobotConnector {
    /// Empty until the workspace's settings are read.
    address: Memo<String>,
    health: Signal<Option<Result<RobotConnectorHealth, CommandError>>>,
    refresh: QueryRefresh,
}

impl RobotConnector {
    pub(crate) fn use_provider(settings: WorkspaceSettings, refresh: QueryRefresh) -> Self {
        let address = use_memo(move || {
            settings
                .loaded()
                .map(|settings| settings.robot_connector().address().to_owned())
                .unwrap_or_default()
        });
        let mut health = use_signal(|| None);
        // Another address is another robot connector: nothing heard from the
        // last one holds.
        use_effect(move || {
            let _ = address.read();
            health.set(None);
            refresh.invalidate(&[
                QueryDomain::RobotConnector,
                QueryDomain::RobotSetup,
                QueryDomain::Telemetry,
                QueryDomain::Recording,
                QueryDomain::Rollout,
            ]);
        });
        use_context_provider(|| Self {
            address,
            health,
            refresh,
        })
    }

    pub(crate) fn current() -> Self {
        consume_context()
    }

    /// Ask whether the robot connector answers while `active`. Only views that
    /// drive hardware ask, so a browser that never opens one never reaches for
    /// a local network address.
    pub(crate) fn use_health(self, active: ReadSignal<bool>) {
        let response = QueryPolicy::new(QueryDomain::RobotConnector, 2_000).use_when(
            move || *active.read() && !self.address.read().is_empty(),
            move || self.request(HttpRequest::new("GET", "/api/health")),
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

    pub(crate) fn address(self) -> String {
        self.address.read().clone()
    }

    /// Drive the robot connector at `address` from now on, from every
    /// browser in this workspace.
    pub(crate) fn save_address(
        self,
        address: &str,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        let saved = ProjectClient::current().mutate::<DataSettings>(
            &[QueryDomain::Settings],
            HttpRequest::new("PUT", "/api/settings/robot-connector")
                .json(&RobotConnectorSettings::new(address.trim())),
        );
        async move { saved.await.map(|_| ()) }
    }

    /// Ask again now rather than at the next poll.
    pub(crate) fn check_again(self) {
        self.refresh.invalidate(&[QueryDomain::RobotConnector]);
    }

    /// Whether this page may need the browser's permission to reach the robot
    /// connector: a secure page calling a plain-HTTP address on the local
    /// network.
    pub(crate) fn needs_local_network_permission(self) -> bool {
        let secure_page = web_sys::window()
            .and_then(|window| window.location().protocol().ok())
            .is_some_and(|protocol| protocol == "https:");
        secure_page && self.address.read().starts_with("http://")
    }

    /// The robot connector's build when it last answered, or why it did not.
    pub(crate) fn health(self) -> Option<Result<RobotConnectorHealth, CommandError>> {
        self.health.read().clone()
    }

    pub(crate) fn reachable(self) -> bool {
        matches!(*self.health.read(), Some(Ok(_)))
    }

    /// A browser-native URL on the robot connector, such as a camera stream.
    pub(crate) fn url(self, path: &str) -> String {
        format!("{}{path}", self.address.read())
    }

    pub(crate) fn request<T: DeserializeOwned + 'static>(
        self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<T, CommandError>> + use<T> {
        self.addressed(request).send()
    }

    pub(crate) fn mutate<T: DeserializeOwned + 'static>(
        self,
        domains: &'static [QueryDomain],
        request: HttpRequest,
    ) -> impl Future<Output = Result<T, CommandError>> + use<T> {
        let request = self.request(request);
        let refresh = self.refresh;
        async move {
            let result = request.await;
            // A failure may follow partial robot connector work. Refetch either way.
            refresh.invalidate(domains);
            result
        }
    }

    /// A binary response of `media_type`, kept as a browser blob.
    pub(crate) fn request_blob(
        self,
        request: HttpRequest,
        media_type: &'static str,
    ) -> impl Future<Output = Result<web_sys::Blob, CommandError>> + use<> {
        self.addressed(request).blob_response(media_type)
    }

    fn addressed(self, request: HttpRequest) -> HttpRequest {
        request.at(&self.address.peek()).unreachable(UNREACHABLE)
    }
}
