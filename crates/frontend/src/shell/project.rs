//! The project this browser works on. The browser chooses it, not the
//! backend: the saved connection supplies the first one, and this browser's
//! settings saves change it. Every project resource, draft, and request is
//! mounted under it.
use crate::client::project::ProjectClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::client::{CommandError, transport::HttpRequest};
use dioxus::prelude::*;
use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_common::settings::SpiralSettings;

#[derive(Clone, Copy)]
pub(crate) struct ProjectSelection {
    selected: Signal<Option<ProjectKey>>,
    live: Signal<bool>,
    refresh: QueryRefresh,
}
impl ProjectSelection {
    pub(super) fn use_provider() -> Self {
        let refresh = QueryRefresh::use_provider();
        let selected = use_signal(|| None);
        let live = use_signal(|| false);
        use_context_provider(|| Self {
            selected,
            live,
            refresh,
        })
    }
    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn selected(self) -> Option<ProjectKey> {
        self.selected.read().clone()
    }
    /// Whether the last read of the saved connection reached Studio.
    pub(crate) fn live(self) -> bool {
        *self.live.read()
    }
    /// Whether catalog features can run: Studio is reachable and a Spiral
    /// project is chosen.
    pub(crate) fn catalog_available(self) -> bool {
        self.live()
            && self
                .selected
                .read()
                .as_ref()
                .is_some_and(ProjectKey::is_chosen)
    }
    pub(super) fn client(self, project: ProjectKey) -> ProjectClient {
        ProjectClient::new(project, self.refresh)
    }
    /// Work on `project` from now on, once this browser has saved it.
    pub(crate) fn select(mut self, project: ProjectKey) {
        if self.selected.peek().as_ref() != Some(&project) {
            self.selected.set(Some(project));
        }
    }
    /// The saved connection proves Studio is reachable. It picks the project
    /// only before one is chosen, or when its endpoint moved, because the
    /// backend then refuses projects on the old endpoint.
    fn accept(mut self, result: Result<SpiralSettings, CommandError>) {
        let live = result.is_ok();
        if *self.live.peek() != live {
            self.live.set(live);
        }
        let Ok(saved) = result else {
            return;
        };
        if self
            .selected
            .peek()
            .as_ref()
            .is_none_or(|project| project.endpoint() != saved.endpoint())
        {
            self.select(saved.project());
        }
    }
    pub(super) fn use_polling(self) {
        let saved = QueryPolicy::new(QueryDomain::Project, 1_000).use_resource(|| {
            HttpRequest::new("GET", "/api/settings/connection").send::<SpiralSettings>()
        });
        use_effect(move || {
            let result = saved.read();
            if saved.finished()
                && let Some(result) = result.as_ref()
            {
                self.accept(result.clone());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::*;

    const ENDPOINT: &str = "https://api.spiraldb.dev";

    #[derive(Clone, Default)]
    struct SelectionHarness(Rc<RefCell<ObservedProjects>>);

    impl PartialEq for SelectionHarness {
        fn eq(&self, other: &Self) -> bool {
            Rc::ptr_eq(&self.0, &other.0)
        }
    }

    #[derive(Default)]
    struct ObservedProjects {
        selection: Option<ProjectSelection>,
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

    impl SelectionHarness {
        fn render(self) -> Element {
            let selection = ProjectSelection::use_provider();
            self.0.borrow_mut().selection = Some(selection);
            rsx! {
                if let Some(project) = selection.selected() {
                    ProjectProbe {
                        key: "{project.endpoint()} {project.project_id()}",
                        project,
                        harness: self,
                    }
                }
            }
        }

        fn selection(&self) -> ProjectSelection {
            self.0.borrow().selection.unwrap()
        }

        fn saved(&self, endpoint: &str, project_id: &str) {
            self.selection().accept(Ok(SpiralSettings::new(
                endpoint,
                "https://spfs.spiraldb.dev",
                project_id,
            )));
        }

        fn unreachable(&self) {
            self.selection().accept(Err(CommandError::new(
                spiral_robotics_common::command_failure::CommandFailureCode::Transport,
                "connection lost",
            )));
        }
    }

    #[component]
    fn ProjectProbe(project: ProjectKey, harness: SelectionHarness) -> Element {
        let selection = ProjectSelection::current();
        let client = use_context_provider(|| selection.client(project));
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn only_this_browser_or_a_moved_endpoint_changes_the_mounted_project() {
        let harness = SelectionHarness::default();
        let mut dom = VirtualDom::new_with_props(SelectionHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.in_runtime(|| harness.saved(ENDPOINT, "first"));
        dom.render_immediate_to_vec();
        assert!(dom.in_runtime(|| harness.selection().live()));
        let mut draft = harness.0.borrow().mounts[0].draft;
        dom.in_runtime(|| draft.set("unsaved".to_owned()));
        dom.render_immediate_to_vec();

        dom.in_runtime(|| harness.unreachable());
        dom.render_immediate_to_vec();
        assert!(!dom.in_runtime(|| harness.selection().live()));
        dom.in_runtime(|| harness.saved(ENDPOINT, "first"));
        dom.render_immediate_to_vec();
        // Another browser saving another project leaves this one alone.
        dom.in_runtime(|| harness.saved(ENDPOINT, "chosen_elsewhere"));
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().mounts.len(), 1);
        assert_eq!(&*draft.peek(), "unsaved");

        dom.in_runtime(|| harness.saved("https://moved.spiraldb.dev", "first"));
        dom.render_immediate_to_vec();
        let observed = harness.0.borrow();
        assert_eq!(observed.mounts.len(), 2);
        assert_eq!(
            observed.mounts[1].client.project().endpoint(),
            "https://moved.spiraldb.dev"
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn selecting_another_project_replaces_local_state_and_cancels_its_requests() {
        let harness = SelectionHarness::default();
        let mut dom = VirtualDom::new_with_props(SelectionHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.in_runtime(|| harness.saved(ENDPOINT, "first"));
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
        dom.in_runtime(|| {
            harness
                .selection()
                .select(ProjectKey::new(ENDPOINT, "second"))
        });
        dom.render_immediate_to_vec();
        let observed = harness.0.borrow();
        assert_eq!(observed.mounts.len(), 2);
        assert_eq!(&*observed.mounts[1].draft.peek(), "initial draft");
        assert_eq!(*observed.mounts[1].selected_run.peek(), None);
        assert!(old_draft.try_read().is_err());
        assert_eq!(observed.canceled.get(), 1);
        assert_eq!(old_client.project().project_id(), "first");
        assert_eq!(observed.mounts[1].client.project().project_id(), "second");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn media_links_name_the_mounted_project_and_preserve_existing_query_parameters() {
        let harness = SelectionHarness::default();
        let mut dom = VirtualDom::new_with_props(SelectionHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.in_runtime(|| harness.saved(ENDPOINT, "project_one"));
        dom.render_immediate_to_vec();
        let client = harness.0.borrow().mounts[0].client.clone();
        assert_eq!(
            client.scoped_url("/api/project/export"),
            "/api/project/export?endpoint=https%3A%2F%2Fapi.spiraldb.dev&project_id=project_one"
        );
        assert_eq!(
            client.scoped_url("/api/project/export?format=bundle"),
            "/api/project/export?format=bundle&endpoint=https%3A%2F%2Fapi.spiraldb.dev&project_id=project_one"
        );
    }
}
