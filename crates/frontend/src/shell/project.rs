//! The process-owned project read determines the lifetime of mounted UI state.
use crate::client::CommandError;
use crate::client::project::{ProjectClient, ProjectSessionState};
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use dioxus::prelude::*;
use spiral_robotics_common::project::{ProjectSessionInfo, ProjectStatus};

#[derive(Clone, Copy)]
pub(crate) struct ProjectSessionSignals {
    state: Signal<ProjectSessionState>,
    refresh: QueryRefresh,
}
impl ProjectSessionSignals {
    pub(crate) fn use_provider() -> Self {
        let refresh = QueryRefresh::use_provider();
        let state = use_signal(ProjectSessionState::default);
        use_context_provider(|| Self { state, refresh })
    }
    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn mounted_session(self) -> Option<ProjectSessionInfo> {
        self.state.read().session().cloned()
    }
    pub(crate) fn client(self, session: ProjectSessionInfo) -> ProjectClient {
        ProjectClient::new(session, self.state.into(), self.refresh)
    }
    pub(crate) fn readiness(self) -> spiral_robotics_common::status::BackendReadiness {
        self.state.read().readiness()
    }
    pub(crate) fn connected(self) -> bool {
        self.state.read().connected()
    }
    pub(crate) fn catalog_available(self) -> bool {
        self.state.read().catalog_available()
    }
    pub(crate) fn accept(mut self, result: Result<ProjectStatus, CommandError>) {
        let next = match result {
            Ok(status) => ProjectSessionState::Ready(status),
            Err(_) => ProjectSessionState::Unavailable(self.state.peek().session().cloned()),
        };
        if *self.state.peek() != next {
            self.state.set(next);
        }
    }
    pub(crate) fn use_polling(self) {
        let status = QueryPolicy::new(QueryDomain::Project, 1_000).use_resource(|| {
            crate::client::transport::HttpRequest::new("GET", "/api/project/status")
                .send::<spiral_robotics_common::project::ProjectStatus>()
        });
        use_effect(move || {
            let result = status.read();
            if status.finished()
                && let Some(result) = result.as_ref()
            {
                self.accept(result.clone());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use futures_util::FutureExt as _;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use spiral_robotics_common::status::{BackendReadiness, BackendReadinessState};

    use super::*;

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
        requests: Rc<Cell<usize>>,
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
            let session = sessions.mounted_session();
            rsx! {
                if let Some(session) = session {
                    ProjectProbe { key: "{session.id()}", session, harness: self }
                }
            }
        }

        fn sessions(&self) -> ProjectSessionSignals {
            self.0.borrow().sessions.unwrap()
        }

        fn fetched(&self, id: &str, state: BackendReadinessState) {
            self.sessions().accept(Ok(ProjectStatus::new(
                ProjectSessionInfo::new(id, "same_project"),
                BackendReadiness::new(
                    state,
                    state == BackendReadinessState::Ready,
                    "Project readiness",
                ),
            )));
        }
        fn unavailable(&self) {
            self.sessions().accept(Err(CommandError::new(
                spiral_robotics_common::command_failure::CommandFailureCode::Transport,
                "connection lost",
            )));
        }
    }

    #[component]
    fn ProjectProbe(session: ProjectSessionInfo, harness: SessionHarness) -> Element {
        let sessions = ProjectSessionSignals::current();
        let client = use_context_provider(|| sessions.client(session));
        let draft = use_signal(|| "initial draft".to_owned());
        let selected_run = use_signal(|| None);
        let requests = use_hook(|| harness.0.borrow().requests.clone());
        use_resource(move || {
            let available = sessions.catalog_available();
            let requests = requests.clone();
            async move {
                if !available {
                    return std::future::pending().await;
                }
                requests.set(requests.get() + 1);
            }
        });
        use_hook(|| {
            harness.0.borrow_mut().mounts.push(ProjectMountProbe {
                client,
                draft,
                selected_run,
            });
        });
        rsx! { div { "{draft}" } }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn failed_status_read_preserves_drafts_and_restored_readiness_restarts_resources() {
        let harness = SessionHarness::default();
        let mut dom = VirtualDom::new_with_props(SessionHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.in_runtime(|| harness.fetched("first", BackendReadinessState::Ready));
        dom.render_immediate_to_vec();
        let mut draft = harness.0.borrow().mounts[0].draft;
        dom.in_runtime(|| draft.set("unsaved".to_owned()));
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().requests.get(), 1);
        dom.in_runtime(|| harness.fetched("first", BackendReadinessState::Ready));
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().requests.get(), 1);
        dom.in_runtime(|| harness.unavailable());
        dom.render_immediate_to_vec();
        assert!(!harness.sessions().catalog_available());
        assert_eq!(&*draft.peek(), "unsaved");
        dom.in_runtime(|| harness.fetched("first", BackendReadinessState::Ready));
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().mounts.len(), 1);
        assert_eq!(harness.0.borrow().requests.get(), 2);
        assert_eq!(&*draft.peek(), "unsaved");
        dom.in_runtime(|| harness.fetched("first", BackendReadinessState::Bootstrapping));
        dom.render_immediate_to_vec();
        assert!(!harness.sessions().catalog_available());
        assert_eq!(harness.0.borrow().mounts.len(), 1);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn project_key_replaces_local_state_and_cancels_its_requests() {
        let harness = SessionHarness::default();
        let mut dom = VirtualDom::new_with_props(SessionHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.in_runtime(|| {
            harness.fetched("first", BackendReadinessState::Ready);
        });
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
        let request_created_in_old_project = old_client
            .request::<spiral_robotics_common::settings::ConnectionTest>(
            crate::client::transport::HttpRequest::new("POST", "/api/settings/training/test"),
        );

        dom.in_runtime(|| harness.fetched("second", BackendReadinessState::Ready));
        // Reject completion as soon as the status is fetched, even before
        // Dioxus has committed disposal of the previous keyed subtree.
        assert!(old_client.accept_response(Ok(7)).is_err());
        assert_eq!(
            request_created_in_old_project
                .now_or_never()
                .unwrap()
                .unwrap_err()
                .code(),
            spiral_robotics_common::command_failure::CommandFailureCode::StaleProject,
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn media_links_capture_the_mount_and_preserve_existing_query_parameters() {
        let harness = SessionHarness::default();
        let mut dom = VirtualDom::new_with_props(SessionHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.in_runtime(|| {
            harness.fetched("session /one", BackendReadinessState::Ready);
        });
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
