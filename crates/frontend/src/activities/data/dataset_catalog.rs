//! One dataset request policy and its recovery control across Studio activities.

use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSummary;

use crate::client::CommandError;

use super::DatasetClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::project::ProjectSessionSignals;

#[derive(Clone, Debug, Default)]
pub(super) enum DatasetCatalogState {
    #[default]
    NotRequested,
    Loading(Option<LoadedDatasets>),
    Ready(LoadedDatasets),
    Failed {
        error: CommandError,
        previous: Option<LoadedDatasets>,
    },
}

#[derive(Clone, Debug)]
pub(super) struct LoadedDatasets {
    datasets: Vec<DatasetSummary>,
}

impl DatasetCatalogState {
    fn snapshot(&self) -> Option<&LoadedDatasets> {
        match self {
            Self::NotRequested => None,
            Self::Ready(snapshot) => Some(snapshot),
            Self::Loading(previous) | Self::Failed { previous, .. } => previous.as_ref(),
        }
    }

    pub(super) fn datasets(&self) -> &[DatasetSummary] {
        self.snapshot().map_or(&[], |snapshot| &snapshot.datasets)
    }

    pub(super) fn loading(&self) -> bool {
        matches!(self, Self::Loading(_))
    }

    pub(super) fn error(&self) -> Option<&CommandError> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }

    pub(super) fn pending(&mut self) {
        if !self.loading() {
            *self = Self::Loading(self.snapshot().cloned());
        }
    }

    pub(super) fn accept(&mut self, result: Result<Vec<DatasetSummary>, CommandError>) {
        let previous = self.snapshot().cloned();
        *self = match result {
            Ok(datasets) => Self::Ready(LoadedDatasets { datasets }),
            Err(error) => Self::Failed { error, previous },
        };
    }
}

/// Each mounted workspace owns its own choice and completed catalog request.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DatasetChoices {
    selected: Signal<String>,
    catalog: Signal<DatasetCatalogState>,
}
impl DatasetChoices {
    pub(crate) fn use_choices() -> Self {
        let choices = use_hook(|| Self {
            selected: Signal::new("default".to_owned()),
            catalog: Signal::new(DatasetCatalogState::default()),
        });
        let sessions = ProjectSessionSignals::current();
        let available = use_memo(move || sessions.catalog_available());
        let request = QueryPolicy::on_demand(QueryDomain::Data).use_when(
            move || *available.read(),
            || DatasetClient::current().list_datasets(),
        );
        let mut catalog = choices.catalog;
        use_effect(move || {
            let result = request.read();
            if !request.finished() {
                catalog.write().pending();
            } else if let Some(result) = result.as_ref() {
                catalog.write().accept(result.clone());
            }
        });
        choices
    }
    pub(crate) fn selected(self) -> Signal<String> {
        self.selected
    }
    pub(crate) fn datasets(self) -> Vec<DatasetSummary> {
        self.catalog.read().datasets().to_vec()
    }
    fn dataset_catalog_error(self) -> Option<CommandError> {
        self.catalog.read().error().cloned()
    }
    fn datasets_loading(self) -> bool {
        self.catalog.read().loading()
    }
    fn retry_datasets(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Data]);
    }
}

/// Background failures stay beside the selector. Retrying is also kept inline,
/// so one catalog failure never creates a notification on every render.
#[component]
pub(crate) fn DatasetCatalogFeedback(data: DatasetChoices) -> Element {
    let available = ProjectSessionSignals::current().catalog_available();
    let error = data.dataset_catalog_error();
    let loading = data.datasets_loading();
    rsx! {
        if let Some(error) = error {
            div { class: "min-w-0 rounded bg-danger-soft px-2 py-1.5 text-[11px] text-danger", role: "status",
                strong { class: "block font-medium", "Could not load datasets" }
                div { class: "break-words", "{error}" }
                div { class: "mt-1", "{error.recovery_guidance()}" }
                if error.retryable() {
                    Action {
                        purpose: ActionPurpose::Retry,
                        variant: ActionVariant::Outline,
                        class: "mt-1",
                        disabled: !available,
                        aria_label: "Retry loading datasets",
                        onclick: move |_| data.retry_datasets(),
                        "Retry"
                    }
                    if !available {
                        div { class: "mt-1", "Retry when the project is available." }
                    }
                }
            }
        } else if loading {
            div { class: "text-[11px] text-ink-faint", role: "status", "Loading datasets…" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::command_failure::CommandFailureCode;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn failed_refresh_preserves_rows_then_recovers() {
        let mut catalog = DatasetCatalogState::default();
        catalog.accept(Ok(vec![DatasetSummary::new(
            spiral_robotics_common::datasets::DatasetSelection::new(vec!["dataset_default".into()])
                .unwrap(),
            ("resource".into(), "version".into()),
        )]));
        catalog.pending();
        assert!(catalog.loading());
        assert_eq!(catalog.datasets()[0].selection().label(), "dataset_default");
        catalog.accept(Err(CommandError::new(
            CommandFailureCode::Transport,
            "connection lost",
        )));
        assert_eq!(catalog.datasets()[0].selection().label(), "dataset_default");
        assert!(catalog.error().is_some());
        catalog.pending();
        catalog.accept(Ok(Vec::new()));
        assert!(catalog.error().is_none());
    }
}
