//! Durable state and commands for the Training screen.
//!
//! Training retains drafts and selection across route changes. Its reads run only
//! while Training is active.
//! Admitted jobs continue on the backend after the view is closed.

use crate::activities::training::TrainingClient;
use std::collections::BTreeSet;

use dioxus::prelude::*;
use spiral_robotics_common::training::TrainingConfig;

use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct TrainingController {
    draft: TrainingDraftSignals,
    history: TrainingHistorySignals,
    commands: TrainingCommandSignals,
}

#[derive(Clone, Copy, PartialEq)]
struct TrainingDraftSignals {
    config: Signal<TrainingConfig>,
    automatic_run_name: Signal<bool>,
}

#[derive(Clone, Copy, PartialEq)]
struct TrainingHistorySignals {
    selected_run: Signal<Option<String>>,
    opened_runs: Signal<Vec<String>>,
}

#[derive(Clone, Copy, PartialEq)]
struct TrainingCommandSignals {
    starting: Signal<bool>,
    stopping: Signal<Option<String>>,
    deleting: Signal<Option<String>>,
}

pub(crate) struct TrainingRunIdentity {
    id: String,
    name: String,
}

impl TrainingRunIdentity {
    pub(super) fn new(id: String, name: String) -> Self {
        Self { id, name }
    }
}

impl TrainingController {
    pub(super) fn use_controller(dataset_id: &str) -> Self {
        let config_sql = String::new();
        let config_dataset = dataset_id.to_owned();
        let controller = use_hook(|| Self {
            draft: TrainingDraftSignals {
                config: Signal::new({
                    TrainingConfig::for_dataset("act-run-001", config_dataset, config_sql)
                }),
                automatic_run_name: Signal::new(true),
            },
            history: TrainingHistorySignals {
                selected_run: Signal::new(None),
                opened_runs: Signal::new(Vec::new()),
            },
            commands: TrainingCommandSignals {
                starting: Signal::new(false),
                stopping: Signal::new(None),
                deleting: Signal::new(None),
            },
        });
        let dataset_dependency = dataset_id.to_owned();
        use_effect(use_reactive((&dataset_dependency,), move |(dataset_id,)| {
            let mut config = controller.draft.config;
            config.write().set_dataset_id(dataset_id);
        }));
        controller
    }

    pub(super) fn config(&self) -> TrainingConfig {
        self.draft.config.read().to_owned()
    }

    pub(super) fn edit_config(mut self, edit: impl FnOnce(&mut TrainingConfig)) {
        edit(&mut self.draft.config.write());
    }

    pub(super) fn set_run_name(mut self, value: String) {
        self.draft.automatic_run_name.set(false);
        self.draft.config.write().set_run_name(value);
    }

    pub(super) fn sync_automatic_run_name(mut self, existing_names: &[String]) {
        if self.draft.automatic_run_name.peek().to_owned() {
            self.draft
                .config
                .write()
                .set_run_name(existing_names.next_default_run_name());
        }
    }

    pub(super) fn selected_run(&self) -> Option<String> {
        self.history.selected_run.read().to_owned()
    }

    pub(super) fn opened_runs(&self) -> Vec<String> {
        self.history.opened_runs.read().to_owned()
    }

    pub(super) fn starting(&self) -> bool {
        self.commands.starting.read().to_owned()
    }

    pub(super) fn stopping(&self) -> Option<String> {
        self.commands.stopping.read().to_owned()
    }

    pub(super) fn deleting(&self) -> Option<String> {
        self.commands.deleting.read().to_owned()
    }

    pub(super) fn open_run(mut self, id: String) {
        if !self.history.opened_runs.peek().contains(&id) {
            self.history.opened_runs.write().push(id.clone());
        }
        self.history.selected_run.set(Some(id));
    }

    pub(super) fn select_run(mut self, id: String) {
        self.history.selected_run.set(Some(id));
    }

    pub(super) fn close_run(mut self, id: String) {
        self.history
            .opened_runs
            .write()
            .retain(|opened| opened != &id);
        if self.history.selected_run.peek().as_deref() == Some(id.as_str()) {
            self.history.selected_run.set(None);
        }
    }

    pub(super) fn start(
        mut self,
        studio: StudioProjectState,
        history: super::history::TrainingHistory,
    ) {
        if self.commands.starting.peek().to_owned() {
            return;
        }
        self.commands.starting.set(true);
        let config = (self.draft.config)();
        let mut starting = self.commands.starting;
        let mut selected_run = self.history.selected_run;
        let mut opened_runs = self.history.opened_runs;
        let mut config_signal = self.draft.config;
        let mut automatic_run_name = self.draft.automatic_run_name;
        let training = history.snapshot();
        dioxus::dioxus_core::Runtime::current().spawn(
            self.draft.config.origin_scope(),
            async move {
                let result = TrainingClient::current()
                    .start_training_run(config)
                    .await
                    .map_err(|error| error.to_string());
                starting.set(false);
                match result {
                    Ok(run) => {
                        let mut existing_names = training
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
                        config_signal
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
            },
        );
    }

    pub(super) fn stop(mut self, studio: StudioProjectState, id: String) {
        if self.commands.stopping.peek().is_some() {
            return;
        }
        self.commands.stopping.set(Some(id.clone()));
        let mut stopping = self.commands.stopping;
        studio.run_utility("Stop requested.", async move {
            let result = TrainingClient::current()
                .stop_training_run(id)
                .await
                .map_err(|error| error.to_string());
            stopping.set(None);
            result
        });
    }

    pub(super) fn delete(mut self, studio: StudioProjectState, run: TrainingRunIdentity) {
        if self.commands.deleting.peek().is_some() {
            return;
        }
        self.commands.deleting.set(Some(run.id.clone()));
        let deleted_id = run.id.clone();
        let run_id = run.id.clone();
        let mut deleting = self.commands.deleting;
        let mut selected_run = self.history.selected_run;
        let mut opened_runs = self.history.opened_runs;
        studio.run_utility(format!("Deleted run {}.", run.name), async move {
            let result = TrainingClient::current()
                .delete_training_run(run_id)
                .await
                .map_err(|error| error.to_string());
            deleting.set(None);
            if result.is_ok() {
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

    #[wasm_bindgen_test::wasm_bindgen_test]
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn closing_selected_run_prefers_the_tab_to_its_right() {
        let mut opened = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let selected = opened.selection_after_close(Some("b"), "b");
        assert_eq!(opened, ["a", "c"]);
        assert_eq!(selected.as_deref(), Some("c"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn closing_last_selected_run_falls_back_left_then_to_empty() {
        let mut opened = vec!["a".to_owned(), "b".to_owned()];
        let selected = opened.selection_after_close(Some("b"), "b");
        assert_eq!(selected.as_deref(), Some("a"));
        let selected = opened.selection_after_close(selected.as_deref(), "a");
        assert!(opened.is_empty());
        assert_eq!(selected, None);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn closing_background_run_preserves_selection() {
        let mut opened = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let selected = opened.selection_after_close(Some("c"), "a");
        assert_eq!(opened, ["b", "c"]);
        assert_eq!(selected.as_deref(), Some("c"));
    }
}
