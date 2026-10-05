//! Immutable request contexts for the project a mounted view works on.

use std::future::Future;

use super::queries::{QueryDomain, QueryRefresh};
use dioxus::prelude::*;
use spiral_robotics_common::project::ProjectKey;

use super::transport::{CommandError, HttpRequest};
use serde::de::DeserializeOwned;

/// Every request names `project`, so a request made just before another
/// project is selected still acts on the project its view belongs to. Tasks
/// end with the view's mount.
#[derive(Clone)]
pub(crate) struct ProjectClient {
    project: ProjectKey,
    owner: ScopeId,
    refresh: QueryRefresh,
    /// The browser's selected project, which a stale reply clears.
    selected: Option<Signal<Option<ProjectKey>>>,
}

impl ProjectClient {
    pub(crate) fn new(project: ProjectKey, refresh: QueryRefresh) -> Self {
        Self {
            project,
            refresh,
            owner: dioxus::dioxus_core::current_scope_id(),
            selected: None,
        }
    }

    /// A reply saying this client's project is stale clears it from
    /// `selected`, so the browser goes on in its saved one.
    pub(crate) fn forgotten_when_stale(mut self, selected: Signal<Option<ProjectKey>>) -> Self {
        self.selected = Some(selected);
        self
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

    pub(crate) fn request<T: DeserializeOwned + 'static>(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<T, CommandError>> + use<T> {
        let client = self.clone();
        async move {
            let response = request.for_project(client.project.clone()).send().await;
            client.observe(response)
        }
    }
    /// An Arrow IPC response's bytes.
    pub(crate) fn request_bytes(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<Vec<u8>, CommandError>> + use<> {
        let client = self.clone();
        async move {
            let response = request
                .for_project(client.project.clone())
                .bytes("application/vnd.apache.arrow.stream")
                .await;
            client.observe(response)
        }
    }

    /// A binary response of `media_type`, kept as a browser blob.
    pub(crate) fn request_blob(
        &self,
        request: HttpRequest,
        media_type: &'static str,
    ) -> impl Future<Output = Result<web_sys::Blob, CommandError>> + use<> {
        let client = self.clone();
        async move {
            let response = request
                .for_project(client.project.clone())
                .blob_response(media_type)
                .await;
            client.observe(response)
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

    /// A stale project means the saved endpoint changed or the workspace was
    /// deleted; rereading the saved connection selects a project again.
    fn observe<T>(&self, response: Result<T, CommandError>) -> Result<T, CommandError> {
        if response.as_ref().err().is_some_and(|error| {
            error.code()
                == spiral_robotics_common::command_failure::CommandFailureCode::StaleProject
        }) {
            if let Some(mut selected) = self.selected
                && selected.peek().as_ref() == Some(&self.project)
            {
                selected.set(None);
            }
            self.refresh.invalidate(&[QueryDomain::Project]);
        }
        response
    }

    /// A browser-native download names the project in the URL.
    pub(crate) fn scoped_url(&self, path: &str) -> String {
        let project = serde_urlencoded::to_string(&self.project)
            .expect("a project key encodes as a query string");
        format!("{path}?{project}")
    }
}
