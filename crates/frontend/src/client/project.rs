//! Immutable request contexts for the project a mounted view works on.

use std::future::Future;

use super::queries::{QueryDomain, QueryRefresh};
use dioxus::prelude::*;
use spiral_robotics_common::project::ProjectKey;

use super::transport::{CommandError, HttpRequest};
use serde::de::DeserializeOwned;

#[derive(Clone)]
pub(crate) struct ProjectClient {
    project: ProjectKey,
    selection: ReadSignal<Option<ProjectKey>>,
    owner: ScopeId,
    refresh: QueryRefresh,
}

impl ProjectClient {
    pub(crate) fn new(
        project: ProjectKey,
        selection: ReadSignal<Option<ProjectKey>>,
        refresh: QueryRefresh,
    ) -> Self {
        Self {
            project,
            selection,
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
    /// dispatch and response delivery against a later project selection.
    pub(crate) fn request<T: DeserializeOwned + 'static>(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<T, CommandError>> + use<T> {
        let client = self.clone();
        async move {
            client.require_current()?;
            let response = request.for_project(client.project.clone()).send().await;
            client.accept_response(response)
        }
    }
    pub(crate) fn request_bytes(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<Vec<u8>, CommandError>> + use<> {
        let client = self.clone();
        async move {
            client.require_current()?;
            let response = request.for_project(client.project.clone()).bytes().await;
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

    pub(crate) fn project(&self) -> &ProjectKey {
        &self.project
    }

    pub(crate) fn require_current(&self) -> Result<(), CommandError> {
        if self.selection.peek().as_ref() == Some(&self.project) {
            Ok(())
        } else {
            Err(CommandError::stale_project())
        }
    }

    /// A stale project means the saved endpoint changed; rereading the saved
    /// connection selects the project again.
    pub(crate) fn accept_response<T>(
        &self,
        response: Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.require_current()?;
        if response.as_ref().err().is_some_and(|error| {
            error.code()
                == spiral_robotics_common::command_failure::CommandFailureCode::StaleProject
        }) {
            self.refresh.invalidate(&[QueryDomain::Project]);
        }
        response
    }

    /// Browser-native media (images, downloads) name the project in the URL.
    pub(crate) fn scoped_url(&self, path: &str) -> String {
        let separator = if path.contains('?') { '&' } else { '?' };
        let project = serde_urlencoded::to_string(&self.project)
            .expect("a project key encodes as a query string");
        format!("{path}{separator}{project}")
    }
}
