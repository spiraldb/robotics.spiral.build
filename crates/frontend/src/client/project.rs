//! Project session lifecycle and immutable request contexts for mounted views.

use std::future::Future;

use dioxus::prelude::*;
use spiral_robotics_common::project::ProjectSessionInfo;
use spiral_robotics_common::status::BackendReadiness;

use super::http::UrlComponent;
use super::transport::CommandError;

/// Readiness belongs to the session confirmed on the current event stream.
/// A disconnected stream retains only the identity needed to keep its mount.
#[cfg_attr(
    not(any(test, all(feature = "web", target_arch = "wasm32"))),
    allow(dead_code)
)]
#[derive(Clone, PartialEq)]
pub(crate) enum ProjectSessionState {
    Disconnected(Option<ProjectSessionInfo>),
    AwaitingSession(Option<ProjectSessionInfo>),
    AwaitingSnapshot(ProjectSessionInfo),
    Synchronized {
        session: ProjectSessionInfo,
        readiness: BackendReadiness,
    },
}

impl Default for ProjectSessionState {
    fn default() -> Self {
        Self::Disconnected(None)
    }
}

impl ProjectSessionState {
    pub(crate) fn session(&self) -> Option<&ProjectSessionInfo> {
        match self {
            Self::Disconnected(session) | Self::AwaitingSession(session) => session.as_ref(),
            Self::AwaitingSnapshot(session) | Self::Synchronized { session, .. } => Some(session),
        }
    }

    pub(crate) fn connected(&self) -> bool {
        !matches!(self, Self::Disconnected(_))
    }

    pub(crate) fn storage_available(&self) -> bool {
        matches!(self, Self::Synchronized { readiness, .. } if readiness.storage_available())
    }

    pub(crate) fn catalog_available(&self) -> bool {
        matches!(self, Self::Synchronized { readiness, .. } if readiness.catalog_available())
    }
}

#[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
impl ProjectSessionState {
    pub(crate) fn connection_changed(self, connected: bool) -> Self {
        match (self, connected) {
            (Self::Disconnected(session), true) => Self::AwaitingSession(session),
            (state, false) => Self::Disconnected(state.session().cloned()),
            (state, true) => state,
        }
    }

    pub(crate) fn announced(self, session: ProjectSessionInfo) -> Self {
        match &self {
            Self::Disconnected(_) => self,
            Self::AwaitingSession(_) => Self::AwaitingSnapshot(session),
            _ if self.session() == Some(&session) => self,
            _ => Self::AwaitingSnapshot(session),
        }
    }

    pub(crate) fn accepts_events_from(&self, session_id: &str) -> bool {
        match self {
            Self::AwaitingSnapshot(session) | Self::Synchronized { session, .. } => {
                session.id() == session_id
            }
            Self::Disconnected(_) | Self::AwaitingSession(_) => false,
        }
    }

    pub(crate) fn with_readiness(self, session_id: &str, readiness: BackendReadiness) -> Self {
        match self {
            Self::AwaitingSnapshot(session) | Self::Synchronized { session, .. }
                if session.id() == session_id =>
            {
                Self::Synchronized { session, readiness }
            }
            state => state,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProjectClient {
    session: ProjectSessionInfo,
    state: ReadSignal<ProjectSessionState>,
    owner: ScopeId,
}

impl ProjectClient {
    pub(crate) fn new(session: ProjectSessionInfo, state: ReadSignal<ProjectSessionState>) -> Self {
        Self {
            session,
            state,
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::task::Poll;

    use futures_util::FutureExt as _;
    use spiral_robotics_common::status::BackendReadinessState;

    use super::*;

    #[test]
    fn readiness_is_accepted_only_for_the_confirmed_session_on_an_open_stream() {
        let first = ProjectSessionInfo::new("first", "project");
        let ready = BackendReadiness::new(BackendReadinessState::Ready, true, "Ready");
        let disconnected = ProjectSessionState::default()
            .announced(first.clone())
            .with_readiness(first.id(), ready.clone());
        assert!(matches!(
            disconnected,
            ProjectSessionState::Disconnected(None)
        ));

        let unannounced = disconnected
            .connection_changed(true)
            .with_readiness(first.id(), ready.clone());
        assert!(matches!(
            unannounced,
            ProjectSessionState::AwaitingSession(None)
        ));
        let awaiting_snapshot = unannounced
            .announced(first.clone())
            .with_readiness("wrong-session", ready.clone());
        assert!(matches!(
            awaiting_snapshot,
            ProjectSessionState::AwaitingSnapshot(_)
        ));
        assert!(!awaiting_snapshot.storage_available());

        let synchronized = awaiting_snapshot.with_readiness(first.id(), ready.clone());
        assert!(synchronized.catalog_available());
        let disconnected = synchronized
            .connection_changed(false)
            .with_readiness(first.id(), ready.clone());
        assert!(
            matches!(&disconnected, ProjectSessionState::Disconnected(Some(retained)) if retained == &first)
        );
        assert!(!disconnected.connected());
        assert!(!disconnected.storage_available());
        let reconnected = disconnected
            .connection_changed(true)
            .with_readiness(first.id(), ready.clone());
        assert!(!reconnected.catalog_available());
        assert!(!reconnected.accepts_events_from(first.id()));

        let replacement = ProjectSessionInfo::new("replacement", "other-project");
        let awaiting_snapshot = reconnected
            .announced(replacement.clone())
            .with_readiness(first.id(), ready.clone());
        assert_eq!(awaiting_snapshot.session(), Some(&replacement));
        assert!(!awaiting_snapshot.catalog_available());
        assert!(
            awaiting_snapshot
                .with_readiness(replacement.id(), ready)
                .catalog_available()
        );
    }

    #[test]
    fn request_rejects_a_response_that_arrives_after_the_session_changes() {
        let mut dom = VirtualDom::new(|| rsx! {});
        dom.rebuild_in_place();
        let (client, mut state) = dom.in_runtime(|| {
            dioxus::dioxus_core::Runtime::current().in_scope(ScopeId::APP, || {
                let session = ProjectSessionInfo::new("origin", "project");
                let state = Signal::new(ProjectSessionState::AwaitingSnapshot(session.clone()));
                (ProjectClient::new(session, state.into()), state)
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
            state.set(ProjectSessionState::AwaitingSnapshot(
                ProjectSessionInfo::new("replacement", "project"),
            ));
        });
        response_ready.set(true);
        assert!(request.now_or_never().unwrap().is_err());
    }
}
