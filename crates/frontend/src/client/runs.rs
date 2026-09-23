//! The runs this browser submitted. A run is one request that answers when the
//! run ends; until then a placeholder stands in for it, so every view reads one
//! list and nothing reconciles observations.
use crate::components::scoped_task::ScopedTask;
use dioxus::prelude::*;
use futures_util::future::{FutureExt, LocalBoxFuture, Shared};
use spiral_robotics_common::runs::RunPhase;

/// The id of the placeholder shown while a submitted run is in flight.
pub(crate) const PENDING_RUN_ID: &str = "pending";

/// The answer to one submitted run, awaitable by every caller that wants it.
pub(crate) type RunAnswer<Run> = Shared<LocalBoxFuture<'static, Result<Run, String>>>;

/// What the history reads from a workflow or curation run.
pub(crate) trait SubmittedRun: Clone + 'static {
    fn document_id(&self) -> &str;
    fn run_id(&self) -> Option<&str>;
    fn phase(&self) -> RunPhase;
    fn started_ms(&self) -> u64;
}

pub(crate) struct RunHistory<Run: 'static> {
    runs: Signal<Vec<Run>>,
    selected: Signal<Option<String>>,
}

// Derives would demand `Run: Copy`; a history is two signal handles whatever it holds.
impl<Run> Clone for RunHistory<Run> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<Run> Copy for RunHistory<Run> {}
impl<Run> PartialEq for RunHistory<Run> {
    fn eq(&self, other: &Self) -> bool {
        self.runs == other.runs && self.selected == other.selected
    }
}

impl<Run: SubmittedRun> RunHistory<Run> {
    pub(crate) fn new() -> Self {
        Self {
            runs: Signal::new(Vec::new()),
            selected: Signal::new(None),
        }
    }

    pub(crate) fn with_runs<Output>(&self, read: impl FnOnce(&[Run]) -> Output) -> Output {
        read(&self.runs.read())
    }

    /// One document's runs, oldest first.
    pub(crate) fn for_document(&self, document_id: &str) -> Vec<Run> {
        let mut runs = self.with_runs(|runs| {
            runs.iter()
                .filter(|run| run.document_id() == document_id)
                .cloned()
                .collect::<Vec<_>>()
        });
        runs.sort_by_key(SubmittedRun::started_ms);
        runs
    }

    /// The run a document shows: the selected one, else its latest.
    pub(crate) fn visible(&self, document_id: &str) -> Option<Run> {
        let runs = self.for_document(document_id);
        let selected = self.selected.read();
        selected
            .as_deref()
            .and_then(|id| runs.iter().find(|run| run.run_id() == Some(id)))
            .or_else(|| runs.last())
            .cloned()
    }

    pub(crate) fn find(&self, run_id: &str) -> Option<Run> {
        self.with_runs(|runs| {
            runs.iter()
                .find(|run| run.run_id() == Some(run_id))
                .cloned()
        })
    }

    /// Select one of a document's runs; `false` when it has no such run.
    pub(crate) fn select(mut self, document_id: &str, run_id: String) -> bool {
        let known = self
            .for_document(document_id)
            .iter()
            .any(|run| run.run_id() == Some(run_id.as_str()));
        if known {
            self.selected.set(Some(run_id));
        }
        known
    }

    pub(crate) fn clear_selection(mut self) {
        self.selected.set(None);
    }

    pub(crate) fn any_active(&self) -> bool {
        self.with_runs(|runs| runs.iter().any(|run| run.phase().active()))
    }

    /// Show `placeholder` while `request` runs on the history's own scope, then
    /// replace it with the answer. The request outlives a caller that stops
    /// awaiting it, such as a cancelled Assistant turn.
    pub(crate) fn submit(
        mut self,
        placeholder: Run,
        request: impl Future<Output = Result<Run, String>> + 'static,
    ) -> RunAnswer<Run> {
        let placeholder_id = placeholder.run_id().map(str::to_owned);
        self.selected.set(None);
        self.runs.write().push(placeholder);
        let answer = request.boxed_local().shared();
        let settled = answer.clone();
        ScopedTask::spawn(self.runs.origin_scope(), async move {
            let answered = settled.await;
            let mut runs = self.runs.write();
            runs.retain(|run| run.run_id() != placeholder_id.as_deref());
            if let Ok(run) = answered {
                runs.push(run);
            }
        });
        answer
    }
}
