//! Durable state and commands for the Training screen.
//!
//! Training can be left while a run is active. The draft, selected history
//! item, and in-flight command guards therefore belong to the shell rather
//! than to the mounted Training workspace.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{StartTrainingRequest, TrainingRun};

use crate::frontend::api::http as api;
use crate::frontend::app::state::Studio;
use crate::frontend::screens::sql_draft::rebase_generated_sql;

#[derive(Clone, Copy)]
pub(crate) struct TrainingController {
    request: Signal<StartTrainingRequest>,
    generated_sql: Signal<String>,
    selected_run: Signal<Option<String>>,
    starting: Signal<bool>,
    stopping: Signal<Option<String>>,
}

pub(crate) fn use_training_controller(
    dataset: &str,
    default_sql: fn(&str) -> String,
) -> TrainingController {
    let initial_sql = default_sql(dataset);
    let request_sql = initial_sql.clone();
    let controller = TrainingController {
        request: use_signal(move || StartTrainingRequest {
            run_name: "act-run-001".to_owned(),
            sql: request_sql,
            ..StartTrainingRequest::default()
        }),
        generated_sql: use_signal(move || initial_sql),
        selected_run: use_signal(|| None),
        starting: use_signal(|| false),
        stopping: use_signal(|| None),
    };
    let dataset_dependency = dataset.to_owned();
    use_effect(use_reactive((&dataset_dependency,), move |(dataset,)| {
        controller.sync_dataset(&dataset, default_sql)
    }));
    use_context_provider(|| controller)
}

pub(crate) fn training_controller() -> TrainingController {
    use_context::<TrainingController>()
}

impl TrainingController {
    fn sync_dataset(mut self, dataset: &str, default_sql: fn(&str) -> String) {
        let previous = self.generated_sql.peek().clone();
        let next = default_sql(dataset);
        if previous == next {
            return;
        }
        {
            let mut request = self.request.write();
            rebase_generated_sql(&mut request.sql, &previous, &next);
        }
        self.generated_sql.set(next);
    }

    pub fn request(&self) -> StartTrainingRequest {
        self.request.read().clone()
    }

    pub fn edit_request(mut self, edit: impl FnOnce(&mut StartTrainingRequest)) {
        edit(&mut self.request.write());
    }

    pub fn selected_run(&self) -> Option<String> {
        self.selected_run.read().clone()
    }

    pub fn starting(&self) -> bool {
        self.starting.read().to_owned()
    }

    pub fn stopping(&self) -> Option<String> {
        self.stopping.read().clone()
    }

    pub fn select_run(mut self, id: String) {
        self.selected_run.set(Some(id));
    }

    pub fn start(mut self, studio: Studio) {
        if self.starting.peek().to_owned() {
            return;
        }
        self.starting.set(true);
        let request = (self.request)();
        let mut starting = self.starting;
        studio.run_utility("Training started.", async move {
            let result = api::send_json::<_, TrainingRun>(
                Method::POST,
                "/api/training/runs",
                Some(&request),
            )
            .await
            .map(|_| ());
            starting.set(false);
            result
        });
    }

    pub fn stop(mut self, studio: Studio, id: String) {
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
