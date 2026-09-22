//! Naming a dataset, once, for every surface that asks for one.
//!
//! Curation's recipe, Workflow's and Explore's "from dataset" creation, the
//! dataset node's empty workspace, and Training's and Live's session setup all
//! ask the same question and all need the same three answers: the catalog is
//! still loading, the catalog could not be read, or here are the datasets.
//!
//! [`DatasetCatalog`] owns the read — including keeping the last good rows on
//! screen through a failed refresh — and [`DatasetPicker`] is the field.

use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSummary;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, Select};

use crate::activities::data::DatasetClient;
use crate::client::CommandError;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::shell::project::ProjectSessionSignals;

/// The project's datasets as one surface sees them.
///
/// Read once per mounted field. A refresh that fails leaves the rows it had:
/// naming a dataset is how most of Studio's work starts, and a momentary
/// catalog failure should not empty the field the operator is typing into.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DatasetCatalog {
    /// The datasets the last successful read returned.
    listed: Signal<Vec<DatasetSummary>>,
    /// Why the last read failed, if it did. Cleared by the next success.
    problem: Signal<Option<CommandError>>,
    reading: Signal<bool>,
}

impl DatasetCatalog {
    pub(crate) fn use_catalog() -> Self {
        let catalog = use_hook(|| Self {
            listed: Signal::new(Vec::new()),
            problem: Signal::new(None),
            reading: Signal::new(false),
        });
        let sessions = ProjectSessionSignals::current();
        let available = use_memo(move || sessions.catalog_available());
        let request = QueryPolicy::on_demand(QueryDomain::Catalog).use_when(
            move || *available.read(),
            || DatasetClient::current().list_datasets(),
        );
        let mut catalog = catalog;
        use_effect(move || {
            let response = request.read();
            if !request.finished() {
                catalog.reading.set(true);
                return;
            }
            catalog.reading.set(false);
            match response.as_ref() {
                Some(Ok(datasets)) => {
                    catalog.listed.set(datasets.clone());
                    catalog.problem.set(None);
                }
                Some(Err(error)) => catalog.problem.set(Some(error.clone())),
                None => {}
            }
        });
        catalog
    }

    pub(crate) fn datasets(self) -> Vec<DatasetSummary> {
        self.listed.read().clone()
    }

    pub(crate) fn reading(self) -> bool {
        *self.reading.read()
    }

    pub(crate) fn problem(self) -> Option<CommandError> {
        self.problem.read().clone()
    }

    /// Ask for the catalog again. Every field reading it gets the new answer.
    pub(crate) fn reread(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Catalog]);
    }
}

/// One dataset, chosen from the project's catalog.
///
/// `chosen` is the whole catalog entry, because every caller wants both its
/// name and its selection. Preset it to open the field on a dataset already
/// named — a saved recipe's source, say; when that dataset is no longer in the
/// catalog the field keeps naming it and says so, rather than silently
/// forgetting what the document is about.
///
/// A failed read stays beside the field with its own Retry, so one catalog
/// failure never raises a notification on every render.
#[component]
pub(crate) fn DatasetPicker(
    /// The `select` element's id; the field's label points at it.
    id: String,
    chosen: Signal<Option<DatasetSummary>>,
    /// What this field is called, for the label a screen reader reads.
    label: &'static str,
) -> Element {
    let catalog = DatasetCatalog::use_catalog();
    let mut chosen = chosen;
    let datasets = catalog.datasets();
    let current = chosen();
    let unlisted = current.clone().filter(|entry| {
        !datasets
            .iter()
            .any(|row| row.resource_id() == entry.resource_id())
    });
    let choose = move |resource_id: String| {
        chosen.set(
            catalog
                .datasets()
                .into_iter()
                .find(|row| row.resource_id() == resource_id),
        );
    };
    let available = ProjectSessionSignals::current().catalog_available();
    rsx! {
        label { r#for: "{id}", class: "sr-only", "{label}" }
        Select {
            id: "{id}",
            value: current.as_ref().map(|entry| entry.resource_id().to_owned()).unwrap_or_default(),
            disabled: datasets.is_empty() && unlisted.is_none(),
            onchange: choose,
            option {
                value: "",
                disabled: true,
                selected: current.is_none(),
                if datasets.is_empty() { "No datasets available" } else { "Choose a dataset" }
            }
            if let Some(entry) = unlisted.as_ref() {
                option { value: "{entry.resource_id()}", selected: true,
                    "{entry.selection().label()} (not in catalog)" }
            }
            for row in datasets.iter() {
                option {
                    key: "{row.resource_id()}",
                    value: "{row.resource_id()}",
                    selected: current.as_ref().is_some_and(|entry| entry.resource_id() == row.resource_id()),
                    "{row.selection().label()}"
                }
            }
        }
        if let Some(problem) = catalog.problem() {
            div { class: "min-w-0 rounded bg-danger-soft px-2 py-1.5 text-[11px] text-danger", role: "status",
                strong { class: "block font-medium", "Could not load datasets" }
                div { class: "break-words", "{problem}" }
                div { class: "mt-1", "{problem.recovery_guidance()}" }
                Button {
                    purpose: ActionPurpose::Retry,
                    variant: ButtonVariant::Outline,
                    class: "mt-1",
                    disabled: !available,
                    aria_label: "Retry loading datasets",
                    onclick: move |_| catalog.reread(),
                    "Retry"
                }
                if !available {
                    div { class: "mt-1", "Retry when the project is available." }
                }
            }
        } else if catalog.reading() {
            div { class: "text-[11px] text-ink-faint", role: "status", "Loading datasets…" }
        }
    }
}
