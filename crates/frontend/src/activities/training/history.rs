//! Training retains history across route changes and polls only while active.
//! Rollout requests its own run/checkpoint choices while its activity is visible.
//! Run status is live, so it is polled often; checkpoints are read from Spiral
//! and change only when a run commits one, so they are polled rarely.
use crate::activities::training::TrainingClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::shell::project::ProjectSelection;
use dioxus::prelude::*;
use spiral_robotics_common::training::{TrainingCheckpoint, TrainingSnapshot};

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct TrainingHistory {
    snapshot: Signal<TrainingSnapshot>,
    checkpoints: Signal<Vec<TrainingCheckpoint>>,
    error: Signal<Option<String>>,
    checkpoint_error: Signal<Option<String>>,
    loaded: Signal<bool>,
    detailed_run: Signal<Option<String>>,
}

impl TrainingHistory {
    pub(in crate::activities) fn use_history(
        is_active: ReadSignal<bool>,
        selected: impl Fn() -> Option<String> + 'static,
    ) -> Self {
        let selection = ProjectSelection::current();
        let request = QueryPolicy::new(QueryDomain::Training, 1_000).use_when(
            move || is_active() && selection.live(),
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
        let history = Self::use_response(request);
        let checkpoints = QueryPolicy::new(QueryDomain::Training, 10_000).use_when(
            move || is_active() && selection.live(),
            || async { TrainingClient::current().checkpoints().await },
        );
        history.use_checkpoints(checkpoints);
        history
    }

    fn use_response(
        request: Resource<Result<TrainingHistoryResponse, crate::client::CommandError>>,
    ) -> Self {
        let mut history = use_hook(|| Self {
            snapshot: Signal::new(TrainingSnapshot::default()),
            checkpoints: Signal::new(Vec::new()),
            error: Signal::new(None),
            checkpoint_error: Signal::new(None),
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

    fn use_checkpoints(
        self,
        request: Resource<Result<Vec<TrainingCheckpoint>, crate::client::CommandError>>,
    ) {
        let mut history = self;
        use_effect(move || {
            let result = request.read();
            if !request.finished() {
                return;
            }
            match result.as_ref() {
                Some(Ok(checkpoints)) => {
                    if *history.checkpoints.peek() != *checkpoints {
                        history.checkpoints.set(checkpoints.clone());
                    }
                    history.checkpoint_error.set(None);
                }
                Some(Err(error)) => history.checkpoint_error.set(Some(error.to_string())),
                None => {}
            }
        });
    }

    pub(super) fn details_loaded(self, id: &str) -> bool {
        self.detailed_run.read().as_deref() == Some(id)
    }

    pub(crate) fn snapshot(self) -> MappedSignal<TrainingSnapshot, Signal<TrainingSnapshot>> {
        self.snapshot.map(|value| value)
    }
    /// Every checkpoint in Spiral, across runs.
    pub(crate) fn checkpoints(self) -> ReadSignal<Vec<TrainingCheckpoint>> {
        self.checkpoints.into()
    }
    pub(super) fn error(self) -> Option<String> {
        self.error
            .read()
            .clone()
            .or_else(|| self.checkpoint_error.read().clone())
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
        use spiral_robotics_common::training::{
            TrainingConfig, TrainingReadiness, TrainingRun, TrainingRunHistory,
            TrainingRunIdentity, TrainingRunStatus,
        };
        let listed = |id: &str| {
            TrainingRun::new(
                TrainingRunIdentity::new(id, TrainingConfig::default(), 1),
                TrainingRunStatus::default(),
            )
        };
        let detailed = |id: &str| {
            listed(id).with_history(TrainingRunHistory::new(
                Vec::new(),
                vec![format!("{id} log")],
            ))
        };
        let (run_a, run_b) = (detailed("a"), detailed("b"));
        let catalog =
            TrainingSnapshot::new(TrainingReadiness::default(), vec![listed("a"), listed("b")]);
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
