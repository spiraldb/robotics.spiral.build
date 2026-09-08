//! Training retains history across route changes and polls only while active.
//! Rollout requests its own run/checkpoint choices while its activity is visible.
use crate::activities::training::TrainingClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::shell::project::ProjectSessionSignals;
use dioxus::prelude::*;
use spiral_robotics_common::training::TrainingSnapshot;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct TrainingHistory {
    snapshot: Signal<TrainingSnapshot>,
    error: Signal<Option<String>>,
    loaded: Signal<bool>,
}

impl TrainingHistory {
    pub(crate) fn use_history(
        is_active: ReadSignal<bool>,
        selected: impl Fn() -> Option<String> + 'static,
    ) -> Self {
        let sessions = ProjectSessionSignals::current();
        let request = QueryPolicy::new(QueryDomain::Training, 1_000).use_when(
            move || is_active() && sessions.catalog_available(),
            move || TrainingClient::current().training_snapshot(selected()),
        );
        let history = Self::use_response(request);
        use_context_provider(|| history)
    }

    fn use_response(
        request: Resource<Result<TrainingSnapshot, crate::client::CommandError>>,
    ) -> Self {
        let mut history = use_hook(|| Self {
            snapshot: Signal::new(TrainingSnapshot::default()),
            error: Signal::new(None),
            loaded: Signal::new(false),
        });
        use_effect(move || {
            let result = request.read();
            if !request.finished() {
                return;
            }
            match result.as_ref() {
                Some(Ok(snapshot)) => {
                    if *history.snapshot.peek() != *snapshot {
                        history.snapshot.set(snapshot.clone());
                    }
                    history.loaded.set(true);
                    history.error.set(None);
                }
                Some(Err(error)) => history.error.set(Some(error.to_string())),
                None => {}
            }
        });
        history
    }

    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn snapshot(self) -> MappedSignal<TrainingSnapshot, Signal<TrainingSnapshot>> {
        self.snapshot.map(|value| value)
    }
    pub(crate) fn error(self) -> Option<String> {
        self.error.read().clone()
    }
    pub(crate) fn loaded(self) -> bool {
        *self.loaded.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct HistoryHarness(Rc<RefCell<Option<HistoryObservation>>>);

    #[derive(Clone, Copy)]
    struct HistoryObservation {
        complete: Signal<bool>,
        history: TrainingHistory,
    }
    impl HistoryHarness {
        fn render(self) -> Element {
            let complete = use_signal(|| false);
            let request = use_resource(move || {
                let complete = complete();
                async move {
                    if !complete {
                        return std::future::pending().await;
                    }
                    Ok(TrainingSnapshot::default())
                }
            });
            let history = TrainingHistory::use_response(request);
            *self.0.borrow_mut() = Some(HistoryObservation { complete, history });
            rsx! {}
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn history_leaves_loading_when_its_first_request_completes() {
        let harness = HistoryHarness::default();
        let mut dom = VirtualDom::new_with_props(HistoryHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let HistoryObservation {
            mut complete,
            history,
        } = harness.0.borrow().unwrap();
        dom.in_runtime(|| assert!(!history.loaded()));
        dom.in_runtime(|| complete.set(true));
        dom.render_immediate_to_vec();
        dom.in_runtime(|| {
            assert!(history.loaded());
            assert!(history.error().is_none());
        });
    }
}
