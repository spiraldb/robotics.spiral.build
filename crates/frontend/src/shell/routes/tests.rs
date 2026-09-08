use super::*;

#[wasm_bindgen_test::wasm_bindgen_test]
fn every_primary_screen_has_a_stable_route() {
    assert_eq!(StudioRoute::Live {}.to_string(), "/live");
    assert_eq!(StudioRoute::Data {}.to_string(), "/data");
    assert_eq!(StudioRoute::Workflow {}.to_string(), "/workflow");
    assert_eq!(StudioRoute::Training {}.to_string(), "/training");
    assert_eq!(StudioRoute::Rollout {}.to_string(), "/rollout");
    assert_eq!(StudioRoute::Settings {}.to_string(), "/settings");
    for route in StudioRoute::all() {
        assert_eq!(StudioRoute::for_activity(route.activity()), route);
    }
    assert_eq!(StudioRoute::Settings {}.activity(), Activity::Settings);
}

use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use dioxus::router::{Navigator, components::HistoryProvider};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

#[derive(Clone, Default)]
struct NavigationHarness(Rc<RefCell<NavigationObservation>>);

#[derive(Default)]
struct NavigationObservation {
    navigator: Option<(Navigator, ScopeId)>,
    project: Option<Signal<u64>>,
    workspaces: HashMap<Activity, WorkspaceObservation>,
}

#[derive(Default)]
struct WorkspaceObservation {
    mounts: usize,
    drops: usize,
    draft: Option<Signal<String>>,
    is_active: Option<ReadSignal<bool>>,
    requests: Rc<Cell<usize>>,
    cancelled: Rc<Cell<usize>>,
}

impl NavigationHarness {
    fn render(self) -> Element {
        use_context_provider(|| self.clone());
        let project = use_signal(|| 1_u64);
        self.0.borrow_mut().project = Some(project);
        rsx! {
            for project_id in [project()] {
                HistoryProvider { key: "{project_id}",
                    history: |_| Rc::new(dioxus::history::MemoryHistory::with_initial_path("/live")) as Rc<dyn History>,
                    Router::<ProbeRoute> {}
                }
            }
        }
    }

    fn navigate(&self, route: ProbeRoute) {
        let (navigator, scope) = self.0.borrow().navigator.unwrap();
        dioxus::dioxus_core::Runtime::current().in_scope(scope, || navigator.push(route));
    }

    fn traverse_history(&self, forward: bool) {
        let (navigator, scope) = self.0.borrow().navigator.unwrap();
        dioxus::dioxus_core::Runtime::current().in_scope(scope, || {
            if forward {
                navigator.go_forward();
            } else {
                navigator.go_back();
            }
        });
    }

    fn assert_active(&self, activity: Activity) {
        for (candidate, workspace) in &self.0.borrow().workspaces {
            assert_eq!((workspace.is_active.unwrap())(), *candidate == activity);
        }
    }

    fn settle(dom: &mut VirtualDom) {
        for _ in 0..4 {
            dom.render_immediate_to_vec();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Routable)]
enum ProbeRoute {
    #[layout(ProbeShell)]
    #[route("/live", SelectedActivityRoute)]
    Live {},
    #[route("/data", SelectedActivityRoute)]
    Data {},
}
impl ProbeRoute {
    fn activity(self) -> Activity {
        match self {
            Self::Live {} => Activity::Live,
            Self::Data {} => Activity::Data,
        }
    }
}

#[component]
fn ProbeShell() -> Element {
    QueryRefresh::use_provider();
    let harness = use_context::<NavigationHarness>();
    harness.0.borrow_mut().navigator =
        Some((use_navigator(), dioxus::dioxus_core::current_scope_id()));
    let route = use_route::<ProbeRoute>();
    let selected_view = use_memo(use_reactive((&route,), |(route,)| route.activity()));
    rsx! {
        for activity in [Activity::Live, Activity::Data] {
            ProbeWorkspaceSlot { key: "{activity.label()}", activity, selected_view }
        }
        Outlet::<ProbeRoute> {}
    }
}

#[component]
fn ProbeWorkspaceSlot(activity: Activity, selected_view: ReadSignal<Activity>) -> Element {
    let is_active = use_memo(move || selected_view() == activity);
    rsx! { PersistentWorkspace { is_active, LocalDraftProbe { activity, is_active } } }
}

struct PendingWorkspaceRead(Rc<Cell<usize>>);
impl Drop for PendingWorkspaceRead {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[component]
fn LocalDraftProbe(activity: Activity, is_active: ReadSignal<bool>) -> Element {
    let harness = use_context::<NavigationHarness>();
    // Ordinary component state, deliberately independent of ProjectViewStates.
    let mut draft = use_signal(String::new);
    let (requests, cancelled) = {
        let mut observations = harness.0.borrow_mut();
        let workspace = observations.workspaces.entry(activity).or_default();
        workspace.draft = Some(draft);
        workspace.is_active = Some(is_active);
        (workspace.requests.clone(), workspace.cancelled.clone())
    };
    use_hook(|| {
        harness
            .0
            .borrow_mut()
            .workspaces
            .get_mut(&activity)
            .unwrap()
            .mounts += 1
    });
    use_drop(move || {
        harness
            .0
            .borrow_mut()
            .workspaces
            .get_mut(&activity)
            .unwrap()
            .drops += 1
    });
    QueryPolicy::new(QueryDomain::Data, 1000).use_when(
        move || *is_active.read(),
        move || {
            requests.set(requests.get() + 1);
            let read = PendingWorkspaceRead(cancelled.clone());
            async move {
                let _read = read;
                std::future::pending::<Result<(), crate::client::CommandError>>().await
            }
        },
    );
    rsx! { input { value: draft(), oninput: move |event| draft.set(event.value()) } }
}

#[wasm_bindgen_test::wasm_bindgen_test]
fn navigation_preserves_local_state_and_pauses_reads_until_reactivation() {
    let harness = NavigationHarness::default();
    let mut dom = VirtualDom::new_with_props(NavigationHarness::render, harness.clone());
    dom.rebuild_in_place();
    NavigationHarness::settle(&mut dom);
    assert!(!harness.0.borrow().workspaces.contains_key(&Activity::Data));
    let mut live_draft = harness.0.borrow().workspaces[&Activity::Live]
        .draft
        .unwrap();
    dom.in_runtime(|| live_draft.set("unsaved live draft".into()));
    dom.in_runtime(|| harness.navigate(ProbeRoute::Data {}));
    NavigationHarness::settle(&mut dom);
    dom.in_runtime(|| harness.assert_active(Activity::Data));
    {
        let observed = harness.0.borrow();
        let live = &observed.workspaces[&Activity::Live];
        assert_eq!(live.mounts, 1);
        assert_eq!(live.drops, 0);
        assert_eq!(live.cancelled.get(), 1);
        assert_eq!(live.requests.get(), 1);
    }
    let mut data_draft = harness.0.borrow().workspaces[&Activity::Data]
        .draft
        .unwrap();
    dom.in_runtime(|| data_draft.set("unsaved data query".into()));
    dom.in_runtime(|| harness.traverse_history(false));
    NavigationHarness::settle(&mut dom);
    dom.in_runtime(|| {
        harness.assert_active(Activity::Live);
        assert_eq!(live_draft(), "unsaved live draft");
        assert_eq!(data_draft(), "unsaved data query");
    });
    assert_eq!(
        harness.0.borrow().workspaces[&Activity::Live]
            .requests
            .get(),
        2
    );
    dom.in_runtime(|| harness.traverse_history(true));
    NavigationHarness::settle(&mut dom);
    dom.in_runtime(|| harness.assert_active(Activity::Data));
    for workspace in harness.0.borrow().workspaces.values() {
        assert_eq!(workspace.mounts, 1);
        assert_eq!(workspace.drops, 0);
    }
    drop(dom);
    for workspace in harness.0.borrow().workspaces.values() {
        assert_eq!(workspace.drops, 1);
        assert_eq!(workspace.requests.get(), workspace.cancelled.get());
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
fn changing_projects_disposes_active_and_hidden_workspaces() {
    let harness = NavigationHarness::default();
    let mut dom = VirtualDom::new_with_props(NavigationHarness::render, harness.clone());
    dom.rebuild_in_place();
    NavigationHarness::settle(&mut dom);
    let mut live_draft = harness.0.borrow().workspaces[&Activity::Live]
        .draft
        .unwrap();
    dom.in_runtime(|| live_draft.set("old project".into()));
    dom.in_runtime(|| harness.navigate(ProbeRoute::Data {}));
    NavigationHarness::settle(&mut dom);
    let mut project = harness.0.borrow().project.unwrap();
    dom.in_runtime(|| project.set(2));
    NavigationHarness::settle(&mut dom);
    let observed = harness.0.borrow();
    assert_eq!(observed.workspaces[&Activity::Data].drops, 1);
    assert_eq!(observed.workspaces[&Activity::Live].drops, 1);
    assert_eq!(observed.workspaces[&Activity::Live].mounts, 2);
    dom.in_runtime(|| assert!(observed.workspaces[&Activity::Live].draft.unwrap()().is_empty()));
}
