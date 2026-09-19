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
    detailed_run: Signal<Option<String>>,
}

impl TrainingHistory {
    pub(in crate::activities) fn use_history(
        is_active: ReadSignal<bool>,
        selected: impl Fn() -> Option<String> + 'static,
    ) -> Self {
        let sessions = ProjectSessionSignals::current();
        let request = QueryPolicy::new(QueryDomain::Training, 1_000).use_when(
            move || is_active() && sessions.studio_available(),
            move || {
                let selected = selected();
                async move {
                    let client = TrainingClient::current();
                    let catalog = client.catalog().await?;
                    // Selection and assembling the visible history belong to this workspace.
                    let detail = match selected
                        .filter(|id| catalog.runs().iter().any(|run| run.id() == id))
                    {
                        Some(id) => Some(client.run(&id).await?),
                        None => None,
                    };
                    Ok(TrainingHistoryResponse::new(catalog, detail))
                }
            },
        );
        Self::use_response(request)
    }

    fn use_response(
        request: Resource<Result<TrainingHistoryResponse, crate::client::CommandError>>,
    ) -> Self {
        let mut history = use_hook(|| Self {
            snapshot: Signal::new(TrainingSnapshot::default()),
            error: Signal::new(None),
            loaded: Signal::new(false),
            detailed_run: Signal::new(None),
        });
        use_effect(move || {
            let result = request.read();
            if !request.finished() {
                return;
            }
            match result.as_ref() {
                Some(Ok(response)) => {
                    let snapshot = &response.snapshot;
                    if *history.detailed_run.peek() != response.detailed_run {
                        history.detailed_run.set(response.detailed_run.clone());
                    }
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

    pub(super) fn details_loaded(self, id: &str) -> bool {
        self.detailed_run.read().as_deref() == Some(id)
    }

    pub(crate) fn snapshot(self) -> MappedSignal<TrainingSnapshot, Signal<TrainingSnapshot>> {
        self.snapshot.map(|value| value)
    }
    pub(super) fn error(self) -> Option<String> {
        self.error.read().clone()
    }
    pub(super) fn loaded(self) -> bool {
        *self.loaded.read()
    }
}

struct TrainingHistoryResponse {
    snapshot: TrainingSnapshot,
    detailed_run: Option<String>,
}
impl TrainingHistoryResponse {
    fn new(
        catalog: TrainingSnapshot,
        detail: Option<spiral_robotics_common::training::TrainingRun>,
    ) -> Self {
        let detailed_run = detail.as_ref().map(|run| run.id().to_owned());
        let runs = catalog
            .runs()
            .iter()
            .map(|run| {
                detail
                    .as_ref()
                    .filter(|detail| detail.id() == run.id())
                    .unwrap_or(run)
                    .clone()
            })
            .collect();
        Self {
            snapshot: TrainingSnapshot::new(catalog.readiness().clone(), runs),
            detailed_run,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn selected_details_are_assembled_locally_without_expanding_other_runs() {
        use spiral_robotics_common::training::{TrainingConfig, TrainingReadiness, TrainingRun};
        let run_a =
            TrainingRun::starting("a", TrainingConfig::default()).with_logs(vec!["a log".into()]);
        let run_b =
            TrainingRun::starting("b", TrainingConfig::default()).with_logs(vec!["b log".into()]);
        let catalog = TrainingSnapshot::new(
            TrainingReadiness::default(),
            vec![run_a.clone(), run_b.clone()],
        )
        .catalog();
        let selected_a = TrainingHistoryResponse::new(catalog.clone(), Some(run_a.clone()));
        assert_eq!(selected_a.detailed_run.as_deref(), Some("a"));
        assert_eq!(selected_a.snapshot.runs()[0], run_a);
        assert!(selected_a.snapshot.runs()[1].logs().is_empty());
        let selected_b = TrainingHistoryResponse::new(catalog, Some(run_b.clone()));
        assert_eq!(selected_b.detailed_run.as_deref(), Some("b"));
        assert!(selected_b.snapshot.runs()[0].logs().is_empty());
        assert_eq!(selected_b.snapshot.runs()[1], run_b);
    }

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
                    Ok(TrainingHistoryResponse::new(
                        TrainingSnapshot::default(),
                        None,
                    ))
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
