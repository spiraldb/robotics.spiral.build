//! Durable state and commands for the Training screen.
//!
//! Training can be left while a run is active. The draft, selected history
//! item, and in-flight command guards therefore belong to the shell rather
//! than to the mounted Training workspace.

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_core::api::{StartTrainingRequest, TrainingRun};

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

    pub(crate) fn open_run(mut self, id: String) {
        if !self.opened_runs.peek().contains(&id) {
            self.opened_runs.write().push(id.clone());
        }
        self.selected_run.set(Some(id));
    }

    pub(crate) fn select_run(mut self, id: String) {
        self.selected_run.set(Some(id));
    }

    pub(crate) fn close_run(mut self, id: String) {
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
    use super::selection_after_close;

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
