//! Dataset navigation and inspection share selection, while each mounted panel owns its reads.
use super::DatasetClient;
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::project::ProjectSessionSignals;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSelection;

#[component]
pub(crate) fn DatasetInspector(selection: DatasetSelection) -> Element {
    let available = use_memo(move || ProjectSessionSignals::current().catalog_available());
    let mut inspection = crate::client::queries::QueryPolicy::on_demand(
        crate::client::queries::QueryDomain::Catalog,
    )
    .use_when(
        move || *available.read(),
        move || DatasetClient::current().inspect_dataset(selection.clone()),
    );
    let response = inspection.read();
    let pending = *inspection.state().read() == UseResourceState::Pending;
    rsx! {
        section { class: "data-inspector", "aria-label": "Dataset inspector",
            if pending { p { class: "data-empty-copy", role: "status", "Loading dataset…" } }
            else { match &*response {
                Some(Ok(detail)) if !detail.dataset().selection().can_preview() => rsx! { p { class: "data-empty-copy", "Preview is unavailable for this dataset name." } },
                Some(Ok(detail)) if detail.members().is_none() => rsx! { p { class: "data-empty-copy", "This dataset has no episode population to browse." } },
                Some(Ok(detail)) => rsx! { super::episode_browser::EpisodeBrowser { inspection: detail.clone() } },
                Some(Err(error)) => rsx! { div { class: "data-empty-copy", role: "alert",
                    p { "{error}" }
                    Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Outline, onclick: move |_| inspection.restart(), "Retry dataset" }
                } },
                None => rsx! { p { class: "data-empty-copy", "Connect to SpiralDB to inspect this dataset." } },
            } }
        }
    }
}
