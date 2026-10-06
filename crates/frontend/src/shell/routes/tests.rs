use super::*;

#[wasm_bindgen_test::wasm_bindgen_test]
fn every_primary_screen_has_a_stable_route() {
    assert_eq!(StudioRoute::Data {}.to_string(), "/data");
    assert_eq!(StudioRoute::Curation {}.to_string(), "/curation");
    assert_eq!(StudioRoute::Training {}.to_string(), "/training");
    assert_eq!(StudioRoute::Rollout {}.to_string(), "/rollout");
    assert_eq!(StudioRoute::Settings {}.to_string(), "/settings");
    for route in StudioRoute::all() {
        assert_eq!(StudioRoute::for_activity(route.activity()), route);
    }
    assert_eq!(StudioRoute::Settings {}.activity(), Activity::Settings);
    assert_eq!(StudioRoute::RolloutSetup {}.to_string(), "/rollout/setup");
    assert_eq!(StudioRoute::RolloutSetup {}.activity(), Activity::Rollout);
    assert_eq!(
        StudioRoute::NewWorkspace {}.to_string(),
        "/settings/workspaces/new"
    );
    assert_eq!(StudioRoute::NewWorkspace {}.activity(), Activity::Settings);
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
    refresh: Option<QueryRefresh>,
}

#[derive(Default)]
struct WorkspaceObservation {
    mounts: usize,
    drops: usize,
    draft: Option<Signal<String>>,
    is_active: Option<ReadSignal<bool>>,
    requests: Rc<Cell<usize>>,
    cancelled: Rc<Cell<usize>>,
    catalog: CatalogObservation,
}

#[derive(Default)]
struct CatalogObservation {
    reads: Rc<Cell<usize>>,
    resource: Option<Resource<Result<usize, crate::client::CommandError>>>,
}

impl NavigationHarness {
    fn render(self) -> Element {
        use_context_provider(|| self.clone());
        let project = use_signal(|| 1_u64);
        self.0.borrow_mut().project = Some(project);
        rsx! {
            for project_id in [project()] {
                HistoryProvider { key: "{project_id}",
                    history: |_| Rc::new(dioxus::history::MemoryHistory::with_initial_path("/rollout")) as Rc<dyn History>,
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
    #[route("/rollout", SelectedActivityRoute)]
    Rollout {},
    #[route("/data", SelectedActivityRoute)]
    Data {},
}
impl ProbeRoute {
    fn activity(self) -> Activity {
        match self {
            Self::Rollout {} => Activity::Rollout,
            Self::Data {} => Activity::Data,
        }
    }
}

#[component]
fn ProbeShell() -> Element {
    let refresh = QueryRefresh::use_provider();
    let harness = use_context::<NavigationHarness>();
    harness.0.borrow_mut().refresh = Some(refresh);
    harness.0.borrow_mut().navigator =
        Some((use_navigator(), dioxus::dioxus_core::current_scope_id()));
    let route = use_route::<ProbeRoute>();
    let selected_view = use_memo(use_reactive((&route,), |(route,)| route.activity()));
    rsx! {
        for activity in [Activity::Rollout, Activity::Data] {
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
    // Ordinary component state owned by the retained workspace.
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
    let dropped = harness.clone();
    use_drop(move || {
        dropped
            .0
            .borrow_mut()
            .workspaces
            .get_mut(&activity)
            .unwrap()
            .drops += 1
    });
    QueryPolicy::new(QueryDomain::Catalog, 1000).use_when(
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
    let catalog_reads = harness.0.borrow().workspaces[&activity]
        .catalog
        .reads
        .clone();
    let catalog = QueryPolicy::on_demand(QueryDomain::Catalog).use_resource(move || {
        let count = catalog_reads.get() + 1;
        catalog_reads.set(count);
        async move { Ok::<_, crate::client::CommandError>(count) }
    });
    harness
        .0
        .borrow_mut()
        .workspaces
        .get_mut(&activity)
        .unwrap()
        .catalog
        .resource = Some(catalog);
    rsx! { input { value: draft(), oninput: move |event| draft.set(event.value()) } }
}

#[wasm_bindgen_test::wasm_bindgen_test]
fn navigation_preserves_local_state_and_pauses_reads_until_reactivation() {
    let harness = NavigationHarness::default();
    let mut dom = VirtualDom::new_with_props(NavigationHarness::render, harness.clone());
    dom.rebuild_in_place();
    NavigationHarness::settle(&mut dom);
    assert!(!harness.0.borrow().workspaces.contains_key(&Activity::Data));
    let mut rollout_draft = harness.0.borrow().workspaces[&Activity::Rollout]
        .draft
        .unwrap();
    dom.in_runtime(|| rollout_draft.set("unsaved rollout draft".into()));
    dom.in_runtime(|| harness.navigate(ProbeRoute::Data {}));
    NavigationHarness::settle(&mut dom);
    dom.in_runtime(|| harness.assert_active(Activity::Data));
    {
        let observed = harness.0.borrow();
        let rollout = &observed.workspaces[&Activity::Rollout];
        assert_eq!(rollout.mounts, 1);
        assert_eq!(rollout.drops, 0);
        assert_eq!(rollout.cancelled.get(), 1);
        assert_eq!(rollout.requests.get(), 1);
    }
    let mut data_draft = harness.0.borrow().workspaces[&Activity::Data]
        .draft
        .unwrap();
    dom.in_runtime(|| data_draft.set("unsaved data query".into()));
    dom.in_runtime(|| harness.traverse_history(false));
    NavigationHarness::settle(&mut dom);
    dom.in_runtime(|| {
        harness.assert_active(Activity::Rollout);
        assert_eq!(rollout_draft(), "unsaved rollout draft");
        assert_eq!(data_draft(), "unsaved data query");
    });
    assert_eq!(
        harness.0.borrow().workspaces[&Activity::Rollout]
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
    let mut rollout_draft = harness.0.borrow().workspaces[&Activity::Rollout]
        .draft
        .unwrap();
    dom.in_runtime(|| rollout_draft.set("old project".into()));
    dom.in_runtime(|| harness.navigate(ProbeRoute::Data {}));
    NavigationHarness::settle(&mut dom);
    let mut project = harness.0.borrow().project.unwrap();
    dom.in_runtime(|| project.set(2));
    NavigationHarness::settle(&mut dom);
    let observed = harness.0.borrow();
    assert_eq!(observed.workspaces[&Activity::Data].drops, 1);
    assert_eq!(observed.workspaces[&Activity::Rollout].drops, 1);
    assert_eq!(observed.workspaces[&Activity::Rollout].mounts, 2);
    dom.in_runtime(|| assert!(observed.workspaces[&Activity::Rollout].draft.unwrap()().is_empty()));
}

#[wasm_bindgen_test::wasm_bindgen_test]
fn completed_catalog_reads_survive_navigation_and_refresh_only_when_invalidated() {
    let harness = NavigationHarness::default();
    let mut dom = VirtualDom::new_with_props(NavigationHarness::render, harness.clone());
    dom.rebuild_in_place();
    NavigationHarness::settle(&mut dom);
    dom.in_runtime(|| harness.navigate(ProbeRoute::Data {}));
    NavigationHarness::settle(&mut dom);
    for _ in 0..3 {
        dom.in_runtime(|| harness.navigate(ProbeRoute::Rollout {}));
        NavigationHarness::settle(&mut dom);
        dom.in_runtime(|| harness.navigate(ProbeRoute::Data {}));
        NavigationHarness::settle(&mut dom);
    }
    {
        let observed = harness.0.borrow();
        for workspace in observed.workspaces.values() {
            assert_eq!(workspace.catalog.reads.get(), 1);
            assert!(workspace.catalog.resource.unwrap().finished());
            assert_eq!(*workspace.catalog.resource.unwrap().peek(), Some(Ok(1)));
        }
    }
    let refresh = harness.0.borrow().refresh.unwrap();
    dom.in_runtime(|| refresh.invalidate(&[QueryDomain::Catalog]));
    NavigationHarness::settle(&mut dom);
    dom.in_runtime(|| harness.navigate(ProbeRoute::Rollout {}));
    NavigationHarness::settle(&mut dom);
    {
        let observed = harness.0.borrow();
        for workspace in observed.workspaces.values() {
            assert_eq!(workspace.catalog.reads.get(), 2);
            assert_eq!(*workspace.catalog.resource.unwrap().peek(), Some(Ok(2)));
        }
    }
    let mut project = harness.0.borrow().project.unwrap();
    dom.in_runtime(|| project.set(2));
    NavigationHarness::settle(&mut dom);
    assert_eq!(
        harness.0.borrow().workspaces[&Activity::Rollout]
            .catalog
            .reads
            .get(),
        3
    );
}
