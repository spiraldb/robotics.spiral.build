//! Training retains history across route changes and polls only while active.
//! Rollout requests its own run/checkpoint choices while its workbench is mounted.
use crate::activities::training::TrainingClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::shell::frame::TrainingStatus;
use crate::shell::{project::ProjectSessionSignals, state::studio};
use dioxus::prelude::*;
use spiral_robotics_common::training::TrainingSnapshot;

#[derive(Clone, Copy)]
pub(crate) struct TrainingHistory {
    snapshot: Signal<TrainingSnapshot>,
    error: Signal<Option<String>>,
    loaded: Signal<bool>,
}

impl TrainingHistory {
    pub(crate) fn use_history(selected: impl Fn() -> Option<String> + 'static) -> Self {
        let sessions = ProjectSessionSignals::current();
        let request = QueryPolicy::new(QueryDomain::Training, 1_000).use_when(
            move || sessions.catalog_available(),
            move || TrainingClient::current().training_snapshot(selected()),
        );
        let history = Self::use_response(request);
        let studio = studio();
        use_effect(move || {
            let status = TrainingStatus::from(&*history.snapshot.read());
            if *studio.training_status().peek() != status {
                studio.training_status().set(status);
            }
        });
        use_context_provider(|| history)
    }

    fn use_response(
        request: Resource<Result<TrainingSnapshot, crate::client::CommandError>>,
    ) -> Self {
        let mut history = crate::shell::view_state::ViewState::use_retained(|| Self {
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
    pub(crate) fn snapshot(self) -> ReadSignal<TrainingSnapshot> {
        self.snapshot.into()
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
    struct HistoryHarness(Rc<RefCell<Option<(Signal<bool>, TrainingHistory)>>>);
    impl HistoryHarness {
        fn render(self) -> Element {
            crate::shell::view_state::ProjectViewStates::use_provider();
            crate::shell::view_state::ViewState::use_provider(
                crate::shell::activity::Activity::Training,
            );
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
            *self.0.borrow_mut() = Some((complete, history));
            rsx! {}
        }
    }

    #[test]
    fn history_leaves_loading_when_its_first_request_completes() {
        let harness = HistoryHarness::default();
        let mut dom = VirtualDom::new_with_props(HistoryHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let (mut complete, history) = harness.0.borrow().unwrap();
        dom.in_runtime(|| assert!(!history.loaded()));
        dom.in_runtime(|| complete.set(true));
        dom.render_immediate_to_vec();
        dom.in_runtime(|| {
            assert!(history.loaded());
            assert!(history.error().is_none());
        });
    }
}
