//! Project session lifecycle and immutable request contexts for mounted views.

use std::future::Future;

use super::queries::{QueryDomain, QueryRefresh};
use dioxus::prelude::*;
use spiral_robotics_common::project::ProjectSessionInfo;

use super::http::UrlComponent;
use super::transport::{CommandError, HttpRequest};
use serde::de::DeserializeOwned;

/// A failed read retains identity (and local drafts), but withdraws readiness.
#[derive(Clone, Default, PartialEq)]
pub(crate) enum ProjectSessionState {
    #[default]
    Loading,
    Unavailable(Option<ProjectSessionInfo>),
    Ready(spiral_robotics_common::project::ProjectStatus),
}
impl ProjectSessionState {
    pub(crate) fn session(&self) -> Option<&ProjectSessionInfo> {
        match self {
            Self::Loading => None,
            Self::Unavailable(session) => session.as_ref(),
            Self::Ready(status) => Some(status.session()),
        }
    }
    pub(crate) fn connected(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
    pub(crate) fn catalog_available(&self) -> bool {
        matches!(self, Self::Ready(status) if status.readiness().catalog_available())
    }
}

#[derive(Clone)]
pub(crate) struct ProjectClient {
    session: ProjectSessionInfo,
    state: ReadSignal<ProjectSessionState>,
    owner: ScopeId,
    refresh: QueryRefresh,
}

impl ProjectClient {
    pub(crate) fn new(
        session: ProjectSessionInfo,
        state: ReadSignal<ProjectSessionState>,
        refresh: QueryRefresh,
    ) -> Self {
        Self {
            session,
            state,
            refresh,
            owner: dioxus::dioxus_core::current_scope_id(),
        }
    }

    /// Dioxus polls tasks with their originating component scope. This context
    /// belongs to that immutable mount, never to a mutable global request header.
    pub(crate) fn current() -> Self {
        consume_context::<Self>()
    }

    /// Consequential requests survive route changes, but are canceled when
    /// their project mount is disposed.
    pub(crate) fn spawn(
        &self,
        task: impl Future<Output = ()> + 'static,
    ) -> dioxus::dioxus_core::Task {
        dioxus::dioxus_core::Runtime::current().in_scope(self.owner, || spawn(task))
    }

    /// Capture the mount before returning a future, then guard both transport
    /// dispatch and response delivery against a later session announcement.
    pub(crate) fn request<T: DeserializeOwned + 'static>(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<T, CommandError>> + use<T> {
        let client = self.clone();
        async move {
            client.require_current()?;
            let response = request
                .for_session(client.session_id().to_owned())
                .send()
                .await;
            if response.as_ref().err().is_some_and(|error| {
                error.code()
                    == spiral_robotics_common::command_failure::CommandFailureCode::StaleProject
            }) {
                client.refresh.invalidate(&[QueryDomain::Project]);
            }
            client.accept_response(response)
        }
    }
    pub(crate) fn mutate<T: DeserializeOwned + 'static>(
        &self,
        domains: &'static [QueryDomain],
        request: HttpRequest,
    ) -> impl Future<Output = Result<T, CommandError>> + use<T> {
        let request = self.request(request);
        let refresh = self.refresh;
        async move {
            let result = request.await;
            // A failure may follow partial backend work. Refetch either way.
            refresh.invalidate(domains);
            result
        }
    }

    pub(crate) fn session_id(&self) -> &str {
        self.session.id()
    }

    pub(crate) fn require_current(&self) -> Result<(), CommandError> {
        if self
            .state
            .peek()
            .session()
            .is_some_and(|active| active.id() == self.session.id())
        {
            Ok(())
        } else {
            Err(CommandError::stale_project())
        }
    }

    pub(crate) fn accept_response<T>(
        &self,
        response: Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.require_current()?;
        response
    }

    pub(crate) fn scoped_url(&self, path: &str) -> String {
        let separator = if path.contains('?') { '&' } else { '?' };
        format!(
            "{path}{separator}session_id={}",
            self.session.id().url_component()
        )
    }
}
