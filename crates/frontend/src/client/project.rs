//! Immutable request context captured by one mounted project subtree.

use std::future::Future;

use dioxus::prelude::*;
use spiral_robotics_common::project::ProjectSessionInfo;

use super::http::UrlComponent;
use super::transport::CommandError;

#[derive(Clone)]
pub(crate) struct ProjectClient {
    session: ProjectSessionInfo,
    active: Signal<Option<ProjectSessionInfo>>,
    owner: ScopeId,
}

impl ProjectClient {
    pub(crate) fn new(
        session: ProjectSessionInfo,
        active: Signal<Option<ProjectSessionInfo>>,
    ) -> Self {
        Self {
            session,
            active,
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
    pub(super) fn request<T, SendRequest, RequestFuture>(
        &self,
        send_request: SendRequest,
    ) -> impl Future<Output = Result<T, CommandError>> + use<T, SendRequest, RequestFuture>
    where
        SendRequest: FnOnce(String) -> RequestFuture,
        RequestFuture: Future<Output = Result<T, CommandError>>,
    {
        let client = self.clone();
        async move {
            client.require_current()?;
            let response = send_request(client.session_id().to_owned()).await;
            client.accept_response(response)
        }
    }

    pub(crate) fn session_id(&self) -> &str {
        self.session.id()
    }

    pub(crate) fn require_current(&self) -> Result<(), CommandError> {
        if self
            .active
            .peek()
            .as_ref()
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::task::Poll;

    use futures_util::FutureExt as _;

    use super::*;

    #[test]
    fn request_rejects_a_response_that_arrives_after_the_session_changes() {
        let mut dom = VirtualDom::new(|| rsx! {});
        dom.rebuild_in_place();
        let (client, mut active) = dom.in_runtime(|| {
            dioxus::dioxus_core::Runtime::current().in_scope(ScopeId::APP, || {
                let session = ProjectSessionInfo::new("origin", "project");
                let active = Signal::new(Some(session.clone()));
                (ProjectClient::new(session, active), active)
            })
        });
        let response_ready = Rc::new(Cell::new(false));
        let ready = response_ready.clone();
        let mut request = Box::pin(client.request(move |session_id| {
            assert_eq!(session_id, "origin");
            std::future::poll_fn(move |_| {
                if ready.get() {
                    Poll::Ready(Ok(7))
                } else {
                    Poll::Pending
                }
            })
        }));
        assert!(request.as_mut().now_or_never().is_none());

        dom.in_runtime(|| {
            active.set(Some(ProjectSessionInfo::new("replacement", "project")));
        });
        response_ready.set(true);
        assert!(request.now_or_never().unwrap().is_err());
    }
}
