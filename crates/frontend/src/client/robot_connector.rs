//! The robot connector this browser drives: an HTTP endpoint that runs next to
//! the arms and owns their hardware, recordings, and rollouts. The browser
//! talks to it directly. Its address belongs to this browser — `localhost`
//! means the machine the browser runs on — so it is kept here, not in Studio.

use std::future::Future;

use dioxus::prelude::*;
use serde::de::DeserializeOwned;
use spiral_robotics_common::robot_connector::RobotConnectorHealth;

use super::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use super::transport::{CommandError, HttpRequest};

const ADDRESS_KEY: &str = "spiral-studio.robot-connector";
const DEFAULT_ADDRESS: &str = "http://localhost:7070";
const UNREACHABLE: &str =
    "Could not reach the robot connector. Check that it is running and its address in Settings.";

#[derive(Clone, Copy)]
pub(crate) struct RobotConnector {
    address: Signal<String>,
    health: Signal<Option<Result<RobotConnectorHealth, CommandError>>>,
    refresh: QueryRefresh,
}

impl RobotConnector {
    pub(crate) fn use_provider(refresh: QueryRefresh) -> Self {
        let address = use_signal(RememberedAddress::read);
        let health = use_signal(|| None);
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
            move || *active.read(),
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

    /// Drive the robot connector at `address` from now on, in this browser.
    pub(crate) fn set_address(mut self, address: &str) {
        let address = address.trim().trim_end_matches('/').to_owned();
        if *self.address.peek() == address {
            return;
        }
        RememberedAddress::keep(&address);
        self.address.set(address);
        self.health.set(None);
        self.refresh.invalidate(&[
            QueryDomain::RobotConnector,
            QueryDomain::RobotSetup,
            QueryDomain::Telemetry,
            QueryDomain::Recording,
            QueryDomain::Rollout,
        ]);
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

/// The address this browser last chose, which survives reloads.
struct RememberedAddress;

impl RememberedAddress {
    fn read() -> String {
        Self::storage()
            .and_then(|storage| storage.get_item(ADDRESS_KEY).ok().flatten())
            .unwrap_or_else(|| DEFAULT_ADDRESS.to_owned())
    }

    fn keep(address: &str) {
        if let Some(storage) = Self::storage() {
            let _ = storage.set_item(ADDRESS_KEY, address);
        }
    }

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }
}
