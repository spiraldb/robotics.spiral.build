//! One dataset request policy and its recovery control across Studio activities.

use dioxus::prelude::*;
use spiral_robotics_common::recording::DatasetSummary;

use crate::client::CommandError;

use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::project::ProjectSessionSignals;
use crate::shell::state::studio;

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
    selection_initialized: bool,
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
            Ok(datasets) => Self::Ready(LoadedDatasets {
                datasets,
                selection_initialized: previous
                    .is_some_and(|snapshot| snapshot.selection_initialized),
            }),
            Err(error) => Self::Failed { error, previous },
        };
    }

    pub(super) fn initialize_selection(&mut self, current: &str) -> Option<String> {
        let snapshot = match self {
            Self::Ready(snapshot) => snapshot,
            Self::Loading(Some(snapshot))
            | Self::Failed {
                previous: Some(snapshot),
                ..
            } => snapshot,
            _ => return None,
        };
        if snapshot.selection_initialized {
            return None;
        }
        snapshot.selection_initialized = true;
        if snapshot
            .datasets
            .iter()
            .any(|dataset| dataset.id() == current)
        {
            return None;
        }
        snapshot
            .datasets
            .iter()
            .find(|dataset| dataset.id() == "default")
            .or_else(|| snapshot.datasets.first())
            .map(|dataset| dataset.id().to_owned())
    }
}

/// Background failures stay beside the selector. Retrying is also kept inline,
/// so one catalog failure never creates a notification on every render.
#[component]
pub(crate) fn DatasetCatalogFeedback() -> Element {
    let data = studio().data_controller();
    let available = ProjectSessionSignals::current().storage_available();
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
    #[test]
    fn failed_refresh_preserves_rows_and_selection_then_recovers() {
        let mut catalog = DatasetCatalogState::default();
        catalog.accept(Ok(vec![DatasetSummary::new("default", "Default", 0)]));
        assert_eq!(
            catalog.initialize_selection("missing"),
            Some("default".to_owned())
        );
        catalog.pending();
        assert!(catalog.loading());
        assert_eq!(catalog.datasets()[0].id(), "default");
        catalog.accept(Err(CommandError::new(
            CommandFailureCode::Transport,
            "connection lost",
        )));
        assert_eq!(catalog.datasets()[0].id(), "default");
        assert!(catalog.error().is_some());
        assert_eq!(catalog.initialize_selection("user-draft"), None);
        catalog.pending();
        catalog.accept(Ok(Vec::new()));
        assert!(catalog.error().is_none());
        assert_eq!(catalog.initialize_selection("user-draft"), None);
    }
}
