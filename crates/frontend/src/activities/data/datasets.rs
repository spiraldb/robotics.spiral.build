//! Dataset navigation and inspection share selection, while each mounted panel owns its reads.
use super::DatasetClient;
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::project::ProjectSessionSignals;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSelection;

#[component]
pub(crate) fn DatasetInspector(selection: DatasetSelection) -> Element {
    let available = use_memo(move || ProjectSessionSignals::current().catalog_available());
    let requested = selection.clone();
    let mut inspection = crate::client::queries::QueryPolicy::on_demand(
        crate::client::queries::QueryDomain::Catalog,
    )
    .use_when(
        move || *available.read(),
        move || DatasetClient::current().inspect_dataset(requested.clone()),
    );
    let response = inspection.read();
    let pending = *inspection.state().read() == UseResourceState::Pending;
    let detail = response
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .cloned();
    rsx! {
        section { class: "data-inspector", "aria-label": "Dataset inspector",
            if pending { p { class: "data-empty-copy", role: "status", "Loading dataset…" } }
            if !*available.read() { p { class: "data-empty-copy", "Connect to SpiralDB to inspect this dataset." } }
            else if !selection.can_preview() { p { class: "data-empty-copy", "Preview is unavailable for this dataset name." } }
            else if let Some(Err(error)) = response.as_ref() {
                div { class: "data-empty-copy", role: "alert",
                    p { "{error}" }
                    Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Outline, onclick: move |_| inspection.restart(), "Retry dataset" }
                }
            } else if detail.as_ref().is_some_and(|detail| detail.members().is_none()) {
                p { class: "data-empty-copy", "This dataset has no episode population to browse." }
            } else {
                super::episode_browser::EpisodeBrowser { dataset: selection, inspection: detail }
            }
        }
    }
}
