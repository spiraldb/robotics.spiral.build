//! Project-lived storage for feature state. It retains values, never components
//! or request tasks. Each routed view creates its own resources when mounted.
use super::activity::Activity;
use dioxus::prelude::*;
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
};

type ActivityStateValues = HashMap<(Activity, TypeId), Box<dyn Any>>;

#[derive(Clone)]
pub(crate) struct ProjectViewStates {
    owner: ScopeId,
    values: Rc<RefCell<ActivityStateValues>>,
}

impl ProjectViewStates {
    pub(crate) fn use_provider() -> Self {
        use_context_provider(|| Self {
            owner: dioxus::dioxus_core::current_scope_id(),
            values: Rc::new(RefCell::new(HashMap::new())),
        })
    }

    /// Factories create plain values and signals, not hooks, resources or tasks.
    fn retain<T: Clone + 'static>(&self, activity: Activity, initialize: impl FnOnce() -> T) -> T {
        let key = (activity, TypeId::of::<T>());
        if let Some(value) = self.values.borrow().get(&key) {
            return value
                .downcast_ref::<T>()
                .expect("state type matches its key")
                .clone();
        }
        let value = dioxus::dioxus_core::Runtime::current().in_scope(self.owner, initialize);
        self.values
            .borrow_mut()
            .insert(key, Box::new(value.clone()));
        value
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ViewState {
    activity: Activity,
}

impl ViewState {
    pub(crate) fn use_provider(activity: Activity) {
        use_context_provider(|| Self { activity });
    }

    pub(crate) fn activity() -> Activity {
        consume_context::<Self>().activity
    }

    pub(crate) fn use_retained<T: Clone + 'static>(initialize: impl FnOnce() -> T) -> T {
        let project = consume_context::<ProjectViewStates>();
        let activity = Self::activity();
        use_hook(move || project.retain(activity, initialize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::training::state::TrainingController;
    use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
    use dioxus::router::{Navigator, components::HistoryProvider};
    use std::cell::Cell;

    #[derive(Clone, Default)]
    struct Harness(Rc<RefCell<Observation>>);
    #[derive(Default)]
    struct Observation {
        navigator: Option<(Navigator, ScopeId)>,
        project: Option<Signal<u64>>,
        training: Option<TrainingController>,
        mounts: usize,
        mounted: usize,
        requests: Rc<Cell<usize>>,
        cancelled: Rc<Cell<usize>>,
    }
    impl Harness {
        fn render(self) -> Element {
            use_context_provider(|| self.clone());
            let project = use_signal(|| 1_u64);
            self.0.borrow_mut().project = Some(project);
            rsx! {
                for project_id in [project()] {
                    HistoryProvider { key: "{project_id}",
                        history: |_| Rc::new(dioxus::history::MemoryHistory::with_initial_path("/settings")) as Rc<dyn History>,
                        Router::<TestRoute> {}
                    }
                }
            }
        }
        fn navigate(&self, route: TestRoute) {
            let (navigator, scope) = self.0.borrow().navigator.unwrap();
            dioxus::dioxus_core::Runtime::current().in_scope(scope, || navigator.push(route));
        }
    }

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum TestRoute {
        #[layout(ProjectBoundary)]
        #[route("/training", TrainingProbe)]
        Training {},
        #[route("/settings", SettingsProbe)]
        Settings {},
    }
    #[component]
    fn ProjectBoundary() -> Element {
        ProjectViewStates::use_provider();
        QueryRefresh::use_provider();
        consume_context::<Harness>().0.borrow_mut().navigator =
            Some((use_navigator(), dioxus::dioxus_core::current_scope_id()));
        rsx! { Outlet::<TestRoute> {} }
    }
    #[derive(Clone, Copy)]
    struct ProbeDraft(Signal<String>);
    #[component]
    fn SettingsProbe() -> Element {
        ViewState::use_provider(Activity::Settings);
        let draft = ViewState::use_retained(|| ProbeDraft(Signal::new("settings draft".into())));
        rsx! { p { "{draft.0}" } }
    }
    struct PendingRead(Rc<Cell<usize>>);
    impl Drop for PendingRead {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    #[component]
    fn TrainingProbe() -> Element {
        ViewState::use_provider(Activity::Training);
        let harness = consume_context::<Harness>();
        let training = TrainingController::use_controller("robotics", "default");
        let draft = ViewState::use_retained(|| ProbeDraft(Signal::new("training draft".into())));
        assert_eq!(
            &*draft.0.peek(),
            "training draft",
            "same state type is isolated between views"
        );
        harness.0.borrow_mut().training = Some(training);
        use_hook(|| {
            let mut observed = harness.0.borrow_mut();
            observed.mounts += 1;
            observed.mounted += 1;
        });
        let requests = harness.0.borrow().requests.clone();
        let cancelled = harness.0.borrow().cancelled.clone();
        QueryPolicy::new(QueryDomain::Training, 1_000).use_resource(move || {
            let requests = requests.clone();
            let cancelled = cancelled.clone();
            async move {
                requests.set(requests.get() + 1);
                let _read = PendingRead(cancelled);
                std::future::pending::<Result<(), crate::client::CommandError>>().await
            }
        });
        use_drop(move || harness.0.borrow_mut().mounted -= 1);
        rsx! { input { value: training.config().run_name() } }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn real_routes_unmount_requests_reuse_state_and_reset_on_project_change() {
        let harness = Harness::default();
        let mut dom = VirtualDom::new_with_props(Harness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(
            harness.0.borrow().mounts,
            0,
            "unvisited Training creates no state or reads"
        );
        dom.in_runtime(|| harness.navigate(TestRoute::Training {}));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        let training = harness.0.borrow().training.unwrap();
        dom.in_runtime(|| training.set_run_name("unsaved experiment".into()));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().requests.get(), 1);
        dom.in_runtime(|| harness.navigate(TestRoute::Settings {}));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().mounted, 0);
        assert_eq!(harness.0.borrow().cancelled.get(), 1);
        assert_eq!(training.config().run_name(), "unsaved experiment");
        dom.in_runtime(|| harness.navigate(TestRoute::Training {}));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(
            harness.0.borrow().mounts,
            2,
            "the routed component really remounts"
        );
        assert_eq!(harness.0.borrow().requests.get(), 2);
        assert_eq!(
            harness.0.borrow().training.unwrap().config().run_name(),
            "unsaved experiment"
        );
        let mut project = harness.0.borrow().project.unwrap();
        dom.in_runtime(|| project.set(2));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().mounted, 0);
        assert_eq!(harness.0.borrow().cancelled.get(), 2);
        dom.in_runtime(|| harness.navigate(TestRoute::Training {}));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().mounts, 3);
        assert_ne!(
            harness.0.borrow().training.unwrap().config().run_name(),
            "unsaved experiment"
        );
        drop(dom);
        assert_eq!(harness.0.borrow().mounted, 0);
        assert_eq!(harness.0.borrow().cancelled.get(), 3);
    }
}
