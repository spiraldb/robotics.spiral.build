//! Durable state and commands for the Training screen.
//!
//! Training can be left while a run is active. The draft, selected history
//! item, and in-flight command guards therefore belong to the shell rather
//! than to the mounted Training workspace.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_api::catalog as spiral;
use spiral_robotics_api::training::{StartTrainingRequest, TrainingRun};

use crate::activities::data::sql_draft::GeneratedSqlDraft;
use crate::client::http as api;
use crate::shell::notifications::MessageSource;
use crate::shell::state::Studio;

#[derive(Clone, Copy)]
pub(crate) struct TrainingController {
    draft: TrainingDraftSignals,
    history: TrainingHistorySignals,
    commands: TrainingCommandSignals,
}

#[derive(Clone, Copy)]
struct TrainingDraftSignals {
    request: Signal<StartTrainingRequest>,
    generated_sql: Signal<String>,
    automatic_run_name: Signal<bool>,
}

#[derive(Clone, Copy)]
struct TrainingHistorySignals {
    selected_run: Signal<Option<String>>,
    opened_runs: Signal<Vec<String>>,
}

#[derive(Clone, Copy)]
struct TrainingCommandSignals {
    starting: Signal<bool>,
    stopping: Signal<Option<String>>,
    deleting: Signal<Option<String>>,
    confirming_delete: Signal<Option<String>>,
}

pub(crate) fn use_training_controller(namespace: &str, dataset_id: &str) -> TrainingController {
    let initial_sql =
        spiral::ManagedTable::Datasets.training_sql_for_dataset(namespace, dataset_id);
    let request_sql = initial_sql.clone();
    let request_dataset = dataset_id.to_owned();
    let controller = TrainingController {
        draft: TrainingDraftSignals {
            request: use_signal(move || {
                StartTrainingRequest::for_dataset("act-run-001", request_dataset, request_sql)
            }),
            generated_sql: use_signal(move || initial_sql),
            automatic_run_name: use_signal(|| true),
        },
        history: TrainingHistorySignals {
            selected_run: use_signal(|| None),
            opened_runs: use_signal(Vec::new),
        },
        commands: TrainingCommandSignals {
            starting: use_signal(|| false),
            stopping: use_signal(|| None),
            deleting: use_signal(|| None),
            confirming_delete: use_signal(|| None),
        },
    };
    let namespace_dependency = namespace.to_owned();
    let dataset_dependency = dataset_id.to_owned();
    use_effect(use_reactive(
        (&namespace_dependency, &dataset_dependency),
        move |(namespace, dataset_id)| {
            controller.sync_dataset(DatasetSync {
                namespace: &namespace,
                dataset_id: &dataset_id,
            })
        },
    ));
    use_context_provider(|| controller)
}

pub(crate) fn training_controller() -> TrainingController {
    use_context::<TrainingController>()
}

struct DatasetSync<'a> {
    namespace: &'a str,
    dataset_id: &'a str,
}

pub(crate) struct TrainingRunIdentity {
    id: String,
    name: String,
}

impl TrainingRunIdentity {
    pub(crate) fn new(id: String, name: String) -> Self {
        Self { id, name }
    }
}

impl TrainingController {
    fn sync_dataset(mut self, dataset: DatasetSync<'_>) {
        let previous = self.draft.generated_sql.peek().to_owned();
        let next = spiral::ManagedTable::Datasets
            .training_sql_for_dataset(dataset.namespace, dataset.dataset_id);
        self.draft
            .request
            .write()
            .set_dataset_id(dataset.dataset_id);
        if previous == next {
            return;
        }
        {
            let mut request = self.draft.request.write();
            let mut sql = request.sql().to_owned();
            if sql.rebase_generated(&previous, &next) {
                request.set_sql(sql);
            }
        }
        self.draft.generated_sql.set(next);
    }

    pub(crate) fn request(&self) -> StartTrainingRequest {
        self.draft.request.read().to_owned()
    }

    pub(crate) fn edit_request(mut self, edit: impl FnOnce(&mut StartTrainingRequest)) {
        edit(&mut self.draft.request.write());
    }

    pub(crate) fn set_run_name(mut self, value: String) {
        self.draft.automatic_run_name.set(false);
        self.draft.request.write().set_run_name(value);
    }

    pub(crate) fn sync_automatic_run_name(mut self, existing_names: &[String]) {
        if self.draft.automatic_run_name.peek().to_owned() {
            self.draft
                .request
                .write()
                .set_run_name(existing_names.next_default_run_name());
        }
    }

    pub(crate) fn selected_run(&self) -> Option<String> {
        self.history.selected_run.read().to_owned()
    }

    pub(crate) fn opened_runs(&self) -> Vec<String> {
        self.history.opened_runs.read().to_owned()
    }

    pub(crate) fn starting(&self) -> bool {
        self.commands.starting.read().to_owned()
    }

    pub(crate) fn stopping(&self) -> Option<String> {
        self.commands.stopping.read().to_owned()
    }

    pub(crate) fn deleting(&self) -> Option<String> {
        self.commands.deleting.read().to_owned()
    }

    pub(crate) fn confirming_delete(&self) -> Option<String> {
        self.commands.confirming_delete.read().to_owned()
    }

    pub(crate) fn confirm_delete(mut self, id: String) {
        if self.commands.deleting.peek().is_none() {
            self.commands.confirming_delete.set(Some(id));
        }
    }

    pub(crate) fn cancel_delete(mut self) {
        self.commands.confirming_delete.set(None);
    }

    pub(crate) fn open_run(mut self, id: String) {
        self.commands.confirming_delete.set(None);
        if !self.history.opened_runs.peek().contains(&id) {
            self.history.opened_runs.write().push(id.clone());
        }
        self.history.selected_run.set(Some(id));
    }

    pub(crate) fn select_run(mut self, id: String) {
        self.commands.confirming_delete.set(None);
        self.history.selected_run.set(Some(id));
    }

    pub(crate) fn close_run(mut self, id: String) {
        self.commands.confirming_delete.set(None);
        let selected = self.history.selected_run.peek().to_owned();
        let next_selected = {
            let mut opened = self.history.opened_runs.write();
            opened.selection_after_close(selected.as_deref(), &id)
        };
        if selected.as_deref() == Some(id.as_str()) {
            self.history.selected_run.set(next_selected);
        }
    }

    pub(crate) fn start(mut self, studio: Studio) {
        if self.commands.starting.peek().to_owned() {
            return;
        }
        self.commands.starting.set(true);
        let request = (self.draft.request)();
        let mut starting = self.commands.starting;
        let mut selected_run = self.history.selected_run;
        let mut opened_runs = self.history.opened_runs;
        let mut request_signal = self.draft.request;
        let mut automatic_run_name = self.draft.automatic_run_name;
        spawn(async move {
            let result = api::Http::send_json::<_, TrainingRun>(
                Method::POST,
                "/api/training/runs",
                Some(&request),
            )
            .await;
            starting.set(false);
            match result {
                Ok(run) => {
                    let mut existing_names = studio
                        .training()
                        .read()
                        .runs()
                        .iter()
                        .map(|existing| existing.config().run_name().to_owned())
                        .collect::<Vec<_>>();
                    if !existing_names
                        .iter()
                        .any(|name| name == run.config().run_name())
                    {
                        existing_names.push(run.config().run_name().to_owned());
                    }
                    request_signal
                        .write()
                        .set_run_name(existing_names.next_default_run_name());
                    automatic_run_name.set(true);
                    if !opened_runs.peek().iter().any(|id| id == run.id()) {
                        opened_runs.write().push(run.id().to_owned());
                    }
                    selected_run.set(Some(run.id().to_owned()));
                }
                Err(error) => studio.report_from(MessageSource::Training, error),
            }
        });
    }

    pub(crate) fn stop(mut self, studio: Studio, id: String) {
        if self.commands.stopping.peek().is_some() {
            return;
        }
        self.commands.stopping.set(Some(id.clone()));
        let mut stopping = self.commands.stopping;
        studio.run_utility("Stop requested.", async move {
            let result = api::Http::post(&format!("/api/training/runs/{id}/stop")).await;
            stopping.set(None);
            result
        });
    }

    pub(crate) fn delete(mut self, studio: Studio, run: TrainingRunIdentity) {
        if self.commands.deleting.peek().is_some() {
            return;
        }
        self.commands.deleting.set(Some(run.id.clone()));
        let path = format!("/api/training/runs/{}", run.id);
        let deleted_id = run.id.clone();
        let mut deleting = self.commands.deleting;
        let mut confirming_delete = self.commands.confirming_delete;
        let mut selected_run = self.history.selected_run;
        let mut opened_runs = self.history.opened_runs;
        studio.run_utility(format!("Deleted run {}.", run.name), async move {
            let result = api::Http::send::<()>(Method::DELETE, &path, None).await;
            deleting.set(None);
            if result.is_ok() {
                confirming_delete.set(None);
                let selected = selected_run.peek().to_owned();
                let next_selected = opened_runs
                    .write()
                    .selection_after_close(selected.as_deref(), &deleted_id);
                if selected.as_deref() == Some(deleted_id.as_str()) {
                    selected_run.set(next_selected);
                }
            }
            result
        });
    }
}

trait TrainingRunNames {
    fn next_default_run_name(&self) -> String;
}

impl TrainingRunNames for [String] {
    fn next_default_run_name(&self) -> String {
        const PREFIX: &str = "act-run-";

        let used = self.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let mut number = self
            .iter()
            .filter_map(|name| name.strip_prefix(PREFIX)?.parse::<u64>().ok())
            .max()
            .unwrap_or_default()
            .saturating_add(1);
        loop {
            let candidate = format!("{PREFIX}{number:03}");
            if !used.contains(candidate.as_str()) {
                return candidate;
            }
            number = number.saturating_add(1);
        }
    }
}

trait OpenedTrainingRuns {
    fn selection_after_close(&mut self, selected: Option<&str>, closing: &str) -> Option<String>;
}

impl OpenedTrainingRuns for Vec<String> {
    fn selection_after_close(&mut self, selected: Option<&str>, closing: &str) -> Option<String> {
        let Some(index) = self.iter().position(|id| id == closing) else {
            return selected.map(str::to_owned);
        };
        self.remove(index);
        if selected != Some(closing) {
            return selected.map(str::to_owned);
        }
        self.get(index)
            .or_else(|| index.checked_sub(1).and_then(|index| self.get(index)))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenedTrainingRuns, TrainingRunNames};

    #[test]
    fn default_run_names_advance_from_history_without_colliding() {
        let names = [
            "act-run-001".to_owned(),
            "experiment".to_owned(),
            "act-run-007".to_owned(),
            "act-run-008".to_owned(),
        ];
        assert_eq!(names.next_default_run_name(), "act-run-009");
        assert_eq!(Vec::<String>::new().next_default_run_name(), "act-run-001");
    }

    #[test]
    fn closing_selected_run_prefers_the_tab_to_its_right() {
        let mut opened = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let selected = opened.selection_after_close(Some("b"), "b");
        assert_eq!(opened, ["a", "c"]);
        assert_eq!(selected.as_deref(), Some("c"));
    }

    #[test]
    fn closing_last_selected_run_falls_back_left_then_to_empty() {
        let mut opened = vec!["a".to_owned(), "b".to_owned()];
        let selected = opened.selection_after_close(Some("b"), "b");
        assert_eq!(selected.as_deref(), Some("a"));
        let selected = opened.selection_after_close(selected.as_deref(), "a");
        assert!(opened.is_empty());
        assert_eq!(selected, None);
    }

    #[test]
    fn closing_background_run_preserves_selection() {
        let mut opened = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let selected = opened.selection_after_close(Some("c"), "a");
        assert_eq!(opened, ["b", "c"]);
        assert_eq!(selected.as_deref(), Some("c"));
    }
}
