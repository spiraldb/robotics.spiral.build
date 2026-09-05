//! Process-level event delivery and the lifetime of mounted project state.

use dioxus::prelude::*;
use spiral_robotics_common::project::ProjectSessionInfo;

#[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
use crate::client::http::ProjectStreamMessage;
use crate::client::http::StudioEvent;

#[derive(Clone, Copy)]
pub(crate) struct ProjectSessionSignals {
    active: Signal<Option<ProjectSessionInfo>>,
    events: Signal<Vec<StudioEvent>>,
    connected: Signal<bool>,
}

impl ProjectSessionSignals {
    pub(crate) fn use_provider() -> Self {
        let active = use_signal(|| None);
        let events = use_signal(Vec::new);
        let connected = use_signal(|| false);
        use_context_provider(|| Self {
            active,
            events,
            connected,
        })
    }

    pub(crate) fn current() -> Self {
        consume_context::<Self>()
    }

    pub(crate) fn active(self) -> Signal<Option<ProjectSessionInfo>> {
        self.active
    }

    pub(crate) fn connected(self) -> bool {
        *self.connected.read()
    }

    pub(crate) fn take_events(mut self) -> Vec<StudioEvent> {
        if self.events.read().is_empty() {
            return Vec::new();
        }
        std::mem::take(&mut *self.events.write())
    }

    #[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
    pub(crate) fn accept(mut self, message: ProjectStreamMessage) {
        match message {
            ProjectStreamMessage::Session(session) => {
                if self.active.peek().as_ref() != Some(&session) {
                    // Reset before publishing the new key. Initial snapshots
                    // remain buffered until the new subtree mounts its effect.
                    self.events.write().clear();
                    self.active.set(Some(session));
                }
            }
            ProjectStreamMessage::Event(event) => {
                let (session_id, payload) = event.into_parts();
                if self
                    .active
                    .peek()
                    .as_ref()
                    .is_some_and(|active| active.id() == session_id)
                {
                    self.events.write().push(payload);
                }
            }
            ProjectStreamMessage::Connected(connected) => self.connected.set(connected),
        }
    }

    pub(crate) fn use_event_stream(self) {
        #[cfg(all(feature = "web", target_arch = "wasm32"))]
        use_future(move || async move {
            loop {
                let _ =
                    crate::client::http::Http::stream_events(move |message| self.accept(message))
                        .await;
                self.accept(ProjectStreamMessage::Connected(false));
                gloo_timers::future::TimeoutFuture::new(1_000).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use futures_util::FutureExt as _;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use spiral_robotics_common::project::ProjectEvent;

    use super::*;
    use crate::client::project::ProjectClient;

    #[derive(Clone, Default)]
    struct SessionHarness(Rc<RefCell<ObservedProjectState>>);

    impl PartialEq for SessionHarness {
        fn eq(&self, other: &Self) -> bool {
            Rc::ptr_eq(&self.0, &other.0)
        }
    }

    #[derive(Default)]
    struct ObservedProjectState {
        sessions: Option<ProjectSessionSignals>,
        mounts: Vec<ProjectMountProbe>,
        canceled: Rc<Cell<usize>>,
    }

    struct ProjectMountProbe {
        client: ProjectClient,
        draft: Signal<String>,
        selected_run: Signal<Option<String>>,
    }

    struct PendingRequest(Rc<Cell<usize>>);

    impl Drop for PendingRequest {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    impl SessionHarness {
        fn render(self) -> Element {
            let sessions = ProjectSessionSignals::use_provider();
            self.0.borrow_mut().sessions = Some(sessions);
            let session = sessions.active().read().clone();
            rsx! {
                if let Some(session) = session {
                    ProjectProbe { key: "{session.id()}", session, harness: self }
                }
            }
        }

        fn sessions(&self) -> ProjectSessionSignals {
            self.0.borrow().sessions.unwrap()
        }

        fn announce(&self, id: &str) {
            self.sessions()
                .accept(ProjectStreamMessage::Session(ProjectSessionInfo::new(
                    id,
                    "same_project",
                )));
        }

        fn snapshot(&self, id: &str) {
            self.sessions()
                .accept(ProjectStreamMessage::Event(ProjectEvent::new(
                    id,
                    StudioEvent::Snapshot(Box::default()),
                )));
        }
    }

    #[component]
    fn ProjectProbe(session: ProjectSessionInfo, harness: SessionHarness) -> Element {
        let sessions = ProjectSessionSignals::current();
        let client = use_context_provider(|| ProjectClient::new(session, sessions.active()));
        let draft = use_signal(|| "initial draft".to_owned());
        let selected_run = use_signal(|| None);
        use_hook(|| {
            harness.0.borrow_mut().mounts.push(ProjectMountProbe {
                client,
                draft,
                selected_run,
            });
        });
        rsx! { div { "{draft}" } }
    }

    #[test]
    fn project_key_replaces_local_state_and_cancels_its_requests() {
        let harness = SessionHarness::default();
        let mut dom = VirtualDom::new_with_props(SessionHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.in_runtime(|| harness.announce("first"));
        dom.render_immediate_to_vec();
        let old_client = harness.0.borrow().mounts[0].client.clone();
        let mut old_draft = harness.0.borrow().mounts[0].draft;
        let mut old_selection = harness.0.borrow().mounts[0].selected_run;
        dom.in_runtime(|| {
            old_draft.set("project A draft".to_owned());
            old_selection.set(Some("run-a".to_owned()));
            let request = PendingRequest(harness.0.borrow().canceled.clone());
            old_client.spawn(async move {
                let _request = request;
                std::future::pending::<()>().await;
            });
        });
        dom.render_immediate_to_vec();
        assert!(old_client.accept_response(Ok(7)).is_ok());
        let request_created_in_old_project = old_client.test_training_runtime();

        dom.in_runtime(|| harness.announce("second"));
        // Reject completion as soon as the announcement arrives, even before
        // Dioxus has committed disposal of the previous keyed subtree.
        assert!(old_client.accept_response(Ok(7)).is_err());
        assert!(
            request_created_in_old_project
                .now_or_never()
                .unwrap()
                .is_err(),
            "a delayed request must reject before transport instead of adopting the new project"
        );
        dom.render_immediate_to_vec();
        let observed = harness.0.borrow();
        assert_eq!(observed.mounts.len(), 2);
        assert_eq!(&*observed.mounts[1].draft.peek(), "initial draft");
        assert_eq!(*observed.mounts[1].selected_run.peek(), None);
        assert!(old_draft.try_read().is_err());
        assert_eq!(observed.canceled.get(), 1);
        assert_eq!(old_client.session_id(), "first");
        assert_eq!(observed.mounts[1].client.session_id(), "second");
    }

    #[test]
    fn announcement_drops_old_events_and_buffers_new_initial_snapshots() {
        let harness = SessionHarness::default();
        let mut dom = VirtualDom::new_with_props(SessionHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.in_runtime(|| {
            harness.announce("first");
            harness.snapshot("first");
            harness.announce("second");
            harness.snapshot("first");
            harness.snapshot("second");
            assert_eq!(harness.sessions().take_events().len(), 1);
            harness.snapshot("second");
            harness.announce("second");
            assert_eq!(
                harness.sessions().take_events().len(),
                1,
                "reconnects retain the same mount's events"
            );
            harness
                .sessions()
                .accept(ProjectStreamMessage::Connected(true));
            assert!(harness.sessions().connected());
            harness
                .sessions()
                .accept(ProjectStreamMessage::Connected(false));
            assert!(!harness.sessions().connected());
        });
    }

    #[test]
    fn media_links_capture_the_mount_and_preserve_existing_query_parameters() {
        let harness = SessionHarness::default();
        let mut dom = VirtualDom::new_with_props(SessionHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.in_runtime(|| harness.announce("session /one"));
        dom.render_immediate_to_vec();
        let client = harness.0.borrow().mounts[0].client.clone();
        assert_eq!(
            client.scoped_url("/api/project/export"),
            "/api/project/export?session_id=session%20%2Fone"
        );
        assert_eq!(
            client.scoped_url("/api/episodes/take/videos/wrist?segment=2"),
            "/api/episodes/take/videos/wrist?segment=2&session_id=session%20%2Fone"
        );
    }
}
