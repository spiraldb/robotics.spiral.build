//! Durable Training activity state and commands.
//!
//! Training can be left while a run is active. The draft, selected history
//! item, and in-flight command guards therefore belong to the shell rather
//! than to the mounted Training workspace.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{StartTrainingRequest, TrainingRun};

use crate::api;
use crate::app::state::Studio;

#[derive(Clone, Copy)]
pub(crate) struct TrainingController {
    request: Signal<StartTrainingRequest>,
    selected_run: Signal<Option<String>>,
    starting: Signal<bool>,
    stopping: Signal<Option<String>>,
}

pub(crate) fn use_training_controller(
    dataset: &str,
    default_sql: impl FnOnce(&str) -> String,
) -> TrainingController {
    let initial_sql = default_sql(dataset);
    let controller = TrainingController {
        request: use_signal(move || StartTrainingRequest {
            run_name: "act-run-001".to_owned(),
            sql: initial_sql,
            ..StartTrainingRequest::default()
        }),
        selected_run: use_signal(|| None),
        starting: use_signal(|| false),
        stopping: use_signal(|| None),
    };
    use_context_provider(|| controller)
}

pub(crate) fn training_controller() -> TrainingController {
    use_context::<TrainingController>()
}

impl TrainingController {
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
