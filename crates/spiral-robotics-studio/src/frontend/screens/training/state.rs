//! Durable state and commands for the Training screen.
//!
//! Training can be left while a run is active. The draft, selected history
//! item, and in-flight command guards therefore belong to the shell rather
//! than to the mounted Training workspace.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_contracts::api::{StartTrainingRequest, TrainingRun};

use crate::frontend::api::http as api;
use crate::frontend::app::notifications::MessageSource;
use crate::frontend::app::state::Studio;
use crate::frontend::screens::sql_draft::rebase_generated_sql;

#[derive(Clone, Copy)]
pub(crate) struct TrainingController {
    request: Signal<StartTrainingRequest>,
    generated_sql: Signal<String>,
    selected_run: Signal<Option<String>>,
    opened_runs: Signal<Vec<String>>,
    starting: Signal<bool>,
    stopping: Signal<Option<String>>,
    deleting: Signal<Option<String>>,
    confirming_delete: Signal<Option<String>>,
    automatic_run_name: Signal<bool>,
}

pub(crate) fn use_training_controller(
    namespace: &str,
    dataset_id: &str,
    default_sql: fn(&str, &str) -> String,
) -> TrainingController {
    let initial_sql = default_sql(namespace, dataset_id);
    let request_sql = initial_sql.clone();
    let request_dataset = dataset_id.to_owned();
    let controller = TrainingController {
        request: use_signal(move || StartTrainingRequest {
            run_name: "act-run-001".to_owned(),
            dataset_id: request_dataset,
            sql: request_sql,
            ..StartTrainingRequest::default()
        }),
        generated_sql: use_signal(move || initial_sql),
        selected_run: use_signal(|| None),
        opened_runs: use_signal(Vec::new),
        starting: use_signal(|| false),
        stopping: use_signal(|| None),
        deleting: use_signal(|| None),
        confirming_delete: use_signal(|| None),
        automatic_run_name: use_signal(|| true),
    };
    let namespace_dependency = namespace.to_owned();
    let dataset_dependency = dataset_id.to_owned();
    use_effect(use_reactive(
        (&namespace_dependency, &dataset_dependency),
        move |(namespace, dataset_id)| {
            controller.sync_dataset(&namespace, &dataset_id, default_sql)
        },
    ));
    use_context_provider(|| controller)
}

pub(crate) fn training_controller() -> TrainingController {
    use_context::<TrainingController>()
}

impl TrainingController {
    fn sync_dataset(
        mut self,
        namespace: &str,
        dataset_id: &str,
        default_sql: fn(&str, &str) -> String,
    ) {
        let previous = self.generated_sql.peek().clone();
        let next = default_sql(namespace, dataset_id);
        self.request.write().dataset_id = dataset_id.to_owned();
        if previous == next {
            return;
        }
        {
            let mut request = self.request.write();
            rebase_generated_sql(&mut request.sql, &previous, &next);
        }
        self.generated_sql.set(next);
    }

    pub(crate) fn request(&self) -> StartTrainingRequest {
        self.request.read().clone()
    }

    pub(crate) fn edit_request(mut self, edit: impl FnOnce(&mut StartTrainingRequest)) {
        edit(&mut self.request.write());
    }

    pub(crate) fn set_run_name(mut self, value: String) {
        self.automatic_run_name.set(false);
        self.request.write().run_name = value;
    }

    pub(crate) fn sync_automatic_run_name(mut self, existing_names: &[String]) {
        if self.automatic_run_name.peek().to_owned() {
            self.request.write().run_name = next_default_run_name(existing_names);
        }
    }

    pub(crate) fn selected_run(&self) -> Option<String> {
        self.selected_run.read().clone()
    }

    pub(crate) fn opened_runs(&self) -> Vec<String> {
        self.opened_runs.read().clone()
    }

    pub(crate) fn starting(&self) -> bool {
        self.starting.read().to_owned()
    }

    pub(crate) fn stopping(&self) -> Option<String> {
        self.stopping.read().clone()
    }

    pub(crate) fn deleting(&self) -> Option<String> {
        self.deleting.read().clone()
    }

    pub(crate) fn confirming_delete(&self) -> Option<String> {
        self.confirming_delete.read().clone()
    }

    pub(crate) fn confirm_delete(mut self, id: String) {
        if self.deleting.peek().is_none() {
            self.confirming_delete.set(Some(id));
        }
    }

    pub(crate) fn cancel_delete(mut self) {
        self.confirming_delete.set(None);
    }

    pub(crate) fn open_run(mut self, id: String) {
        self.confirming_delete.set(None);
        if !self.opened_runs.peek().contains(&id) {
            self.opened_runs.write().push(id.clone());
        }
        self.selected_run.set(Some(id));
    }

    pub(crate) fn select_run(mut self, id: String) {
        self.confirming_delete.set(None);
        self.selected_run.set(Some(id));
    }

    pub(crate) fn close_run(mut self, id: String) {
        self.confirming_delete.set(None);
        let selected = self.selected_run.peek().clone();
        let next_selected = {
            let mut opened = self.opened_runs.write();
            selection_after_close(&mut opened, selected.as_deref(), &id)
        };
        if selected.as_deref() == Some(id.as_str()) {
            self.selected_run.set(next_selected);
        }
    }

    pub(crate) fn start(mut self, studio: Studio) {
        if self.starting.peek().to_owned() {
            return;
        }
        self.starting.set(true);
        let request = (self.request)();
        let mut starting = self.starting;
        let mut selected_run = self.selected_run;
        let mut opened_runs = self.opened_runs;
        let mut request_signal = self.request;
        let mut automatic_run_name = self.automatic_run_name;
        spawn(async move {
            let result = api::send_json::<_, TrainingRun>(
                Method::POST,
                "/api/training/runs",
                Some(&request),
            )
            .await;
            starting.set(false);
            match result {
                Ok(run) => {
                    let mut existing_names = studio
                        .training
                        .read()
                        .runs
                        .iter()
                        .map(|existing| existing.config.run_name.clone())
                        .collect::<Vec<_>>();
                    if !existing_names.contains(&run.config.run_name) {
                        existing_names.push(run.config.run_name.clone());
                    }
                    request_signal.write().run_name = next_default_run_name(&existing_names);
                    automatic_run_name.set(true);
                    if !opened_runs.peek().contains(&run.id) {
                        opened_runs.write().push(run.id.clone());
                    }
                    selected_run.set(Some(run.id));
                }
                Err(error) => studio.report_from(MessageSource::Training, error),
            }
        });
    }

    pub(crate) fn stop(mut self, studio: Studio, id: String) {
        if self.stopping.peek().is_some() {
            return;
        }
        self.stopping.set(Some(id.clone()));
        let mut stopping = self.stopping;
        studio.run_utility("Stop requested.", async move {
            let result = api::post(&format!("/api/training/runs/{id}/stop")).await;
            stopping.set(None);
            result
        });
    }

    pub(crate) fn delete(mut self, studio: Studio, id: String, name: String) {
        if self.deleting.peek().is_some() {
            return;
        }
        self.confirming_delete.set(None);
        self.deleting.set(Some(id.clone()));
        let path = format!("/api/training/runs/{id}");
        let deleted_id = id.clone();
        let mut deleting = self.deleting;
        let mut selected_run = self.selected_run;
        let mut opened_runs = self.opened_runs;
        studio.run_utility(format!("Deleted run {name}."), async move {
            let result = api::send::<()>(Method::DELETE, &path, None).await;
            deleting.set(None);
            if result.is_ok() {
                let selected = selected_run.peek().clone();
                let next_selected = selection_after_close(
                    &mut opened_runs.write(),
                    selected.as_deref(),
                    &deleted_id,
                );
                if selected.as_deref() == Some(deleted_id.as_str()) {
                    selected_run.set(next_selected);
                }
            }
            result
        });
    }
}

fn next_default_run_name(existing_names: &[String]) -> String {
    const PREFIX: &str = "act-run-";

    let used = existing_names
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut number = existing_names
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

fn selection_after_close(
    opened: &mut Vec<String>,
    selected: Option<&str>,
    closing: &str,
) -> Option<String> {
    let Some(index) = opened.iter().position(|id| id == closing) else {
        return selected.map(str::to_owned);
    };
    opened.remove(index);
    if selected != Some(closing) {
        return selected.map(str::to_owned);
    }
    opened
        .get(index)
        .or_else(|| index.checked_sub(1).and_then(|index| opened.get(index)))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::{next_default_run_name, selection_after_close};

    #[test]
    fn default_run_names_advance_from_history_without_colliding() {
        let names = vec![
            "act-run-001".to_owned(),
            "experiment".to_owned(),
            "act-run-007".to_owned(),
            "act-run-008".to_owned(),
        ];
        assert_eq!(next_default_run_name(&names), "act-run-009");
        assert_eq!(next_default_run_name(&[]), "act-run-001");
    }

    #[test]
    fn closing_selected_run_prefers_the_tab_to_its_right() {
        let mut opened = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let selected = selection_after_close(&mut opened, Some("b"), "b");
        assert_eq!(opened, ["a", "c"]);
        assert_eq!(selected.as_deref(), Some("c"));
    }

    #[test]
    fn closing_last_selected_run_falls_back_left_then_to_empty() {
        let mut opened = vec!["a".to_owned(), "b".to_owned()];
        let selected = selection_after_close(&mut opened, Some("b"), "b");
        assert_eq!(selected.as_deref(), Some("a"));
        let selected = selection_after_close(&mut opened, selected.as_deref(), "a");
        assert!(opened.is_empty());
        assert_eq!(selected, None);
    }

    #[test]
    fn closing_background_run_preserves_selection() {
        let mut opened = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let selected = selection_after_close(&mut opened, Some("c"), "a");
        assert_eq!(opened, ["b", "c"]);
        assert_eq!(selected.as_deref(), Some("c"));
    }
}
