//! One dataset request policy and its recovery control across Studio activities.

use dioxus::prelude::*;
use spiral_robotics_common::recording::DatasetSummary;

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
        message: String,
        previous: Option<LoadedDatasets>,
    },
}

#[derive(Clone, Debug)]
pub(super) struct LoadedDatasets {
    datasets: Vec<DatasetSummary>,
    selection_initialized: bool,
}

#[derive(Clone, Copy)]
pub(super) enum DatasetLoadRequest {
    Automatic,
    Reload,
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

    pub(super) fn loaded(&self) -> bool {
        self.snapshot().is_some()
    }

    pub(super) fn loading(&self) -> bool {
        matches!(self, Self::Loading(_))
    }

    pub(super) fn error(&self) -> Option<&str> {
        match self {
            Self::Failed { message, .. } => Some(message),
            _ => None,
        }
    }

    pub(super) fn can_request(&self, request: DatasetLoadRequest, available: bool) -> bool {
        available
            && match request {
                DatasetLoadRequest::Automatic => matches!(self, Self::NotRequested),
                DatasetLoadRequest::Reload => !self.loading(),
            }
    }

    /// Claim synchronously, before spawning: another mounted consumer cannot
    /// launch the same request while its future is waiting to be polled.
    pub(super) fn begin(&mut self, request: DatasetLoadRequest, available: bool) -> bool {
        if !self.can_request(request, available) {
            return false;
        }
        let previous = match std::mem::take(self) {
            Self::NotRequested => None,
            Self::Ready(snapshot) => Some(snapshot),
            Self::Failed { previous, .. } => previous,
            Self::Loading(_) => unreachable!("an in-flight dataset request cannot be replaced"),
        };
        *self = Self::Loading(previous);
        true
    }

    pub(super) fn finish(
        &mut self,
        result: Result<Vec<DatasetSummary>, String>,
    ) -> Result<(), String> {
        let Self::Loading(previous) = std::mem::take(self) else {
            unreachable!("only the admitted dataset request can complete");
        };
        match result {
            Ok(datasets) => {
                *self = Self::Ready(LoadedDatasets {
                    datasets,
                    selection_initialized: previous
                        .is_some_and(|snapshot| snapshot.selection_initialized),
                });
                Ok(())
            }
            Err(message) => {
                let error = format!("Could not load datasets: {message}");
                *self = Self::Failed { message, previous };
                Err(error)
            }
        }
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
        } else if loading {
            div { class: "text-[11px] text-ink-faint", role: "status", "Loading datasets…" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_loading_waits_for_readiness_and_claims_only_one_request() {
        let mut catalog = DatasetCatalogState::default();
        assert!(!catalog.begin(DatasetLoadRequest::Automatic, false));
        assert!(catalog.begin(DatasetLoadRequest::Automatic, true));
        // Other consumers and rapid Retry clicks cannot overlap this request.
        assert!(!catalog.begin(DatasetLoadRequest::Automatic, true));
        assert!(!catalog.begin(DatasetLoadRequest::Reload, true));
        catalog.finish(Ok(Vec::new())).unwrap();
        assert!(catalog.loaded());
        assert!(!catalog.begin(DatasetLoadRequest::Automatic, true));
    }

    #[test]
    fn failure_survives_new_consumers_and_readiness_changes_until_explicit_retry() {
        let mut catalog = DatasetCatalogState::default();
        assert!(catalog.begin(DatasetLoadRequest::Automatic, true));
        assert!(
            catalog
                .finish(Err("catalog unavailable".to_owned()))
                .is_err()
        );
        for available in [true, true, false, true] {
            assert!(!catalog.begin(DatasetLoadRequest::Automatic, available));
            assert_eq!(catalog.error(), Some("catalog unavailable"));
        }
        assert!(!catalog.begin(DatasetLoadRequest::Reload, false));
        assert!(catalog.begin(DatasetLoadRequest::Reload, true));
        catalog.finish(Ok(Vec::new())).unwrap();
        assert!(catalog.loaded());
        assert_eq!(catalog.error(), None);
    }

    #[test]
    fn failed_refresh_preserves_datasets_and_the_users_selection() {
        let mut catalog = DatasetCatalogState::default();
        assert!(catalog.begin(DatasetLoadRequest::Automatic, true));
        catalog
            .finish(Ok(vec![DatasetSummary::new("default", "Default", 0)]))
            .unwrap();
        assert_eq!(
            catalog.initialize_selection("missing"),
            Some("default".to_owned())
        );
        assert!(catalog.begin(DatasetLoadRequest::Reload, true));
        assert_eq!(catalog.datasets()[0].id(), "default");
        assert!(catalog.finish(Err("connection lost".to_owned())).is_err());
        assert!(catalog.loaded());
        assert_eq!(catalog.datasets()[0].id(), "default");
        assert_eq!(catalog.initialize_selection("user-draft"), None);
        assert!(catalog.begin(DatasetLoadRequest::Reload, true));
        catalog.finish(Ok(Vec::new())).unwrap();
        assert_eq!(catalog.initialize_selection("user-draft"), None);
    }
}
