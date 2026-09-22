//! The Build half of Workflow's document bar: which graph is open, whether it
//! is saved, and the three things you can do to it. The identity chrome and
//! the switch/create/rename popover are the shared document components; what
//! is Build's alone is starting a graph from a dataset.
use dioxus::prelude::*;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, Select};

use super::super::state::{BuildController, BuildPendingCommand};
use crate::activities::data::DatasetClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::components::document_bar::{
    DocumentEntry, DocumentRenameForm, DocumentState, DocumentStateChip, DocumentSwitcher,
    DocumentTitle, SwitcherCopy, SwitcherGate,
};
use crate::shell::state::StudioProjectState;

const SWITCHER_ID: &str = "build-switcher";
const TITLE_ID: &str = "build-document-title";

impl BuildPendingCommand {
    /// What the bar says while Studio is busy with this document.
    const fn waiting_for(self) -> &'static str {
        match self {
            Self::Listing => "Reading saved builds…",
            Self::Opening => "Opening a build…",
            Self::Saving => "Saving this build…",
        }
    }
}

#[component]
pub(in crate::activities::workflow) fn BuildDocumentBar(controller: BuildController) -> Element {
    let mut renaming = use_signal(|| false);
    let mut switching = use_signal(|| false);
    let waiting = controller.command().map(BuildPendingCommand::waiting_for);
    let state = DocumentState::of(controller.conflict(), controller.unsaved_changes());
    let name = controller.name();
    let entries = controller
        .documents()
        .iter()
        .map(|item| DocumentEntry::new(item.id(), item.name()))
        .collect::<Vec<_>>();
    // The list is read when Build becomes the open view, which is exactly
    // when this bar mounts.
    use_future(move || async move { controller.refresh_documents() });
    rsx! {
        div { class: "document-identity",
            if renaming() {
                DocumentRenameForm {
                    title_id: TITLE_ID.to_owned(),
                    label: "Build name".to_owned(),
                    initial: name.clone(),
                    disabled: waiting.is_some(),
                    on_apply: move |name| { controller.set_name(name); renaming.set(false); },
                    on_cancel: move |_| renaming.set(false),
                }
            } else {
                DocumentTitle {
                    id: TITLE_ID.to_owned(),
                    name: name.clone(),
                    hint: waiting.map(str::to_owned).unwrap_or_else(|| "Switch, create, or rename builds".to_owned()),
                    expanded: switching(),
                    on_toggle: move |_| switching.toggle(),
                }
            }
            if switching() {
                DocumentSwitcher {
                    id: SWITCHER_ID.to_owned(),
                    title_id: TITLE_ID.to_owned(),
                    entries,
                    current: DocumentEntry::new(controller.id(), name),
                    copy: SwitcherCopy::new("Saved builds", "No saved builds yet.", "New blank build", "Rename this build"),
                    gate: SwitcherGate::new(waiting.map(str::to_owned), waiting.is_some()),
                    on_select: move |id: Option<String>| {
                        match id {
                            Some(id) => controller.open(id),
                            None => controller.create("Untitled build", None),
                        }
                        switching.set(false);
                    },
                    on_rename: move |_| { switching.set(false); renaming.set(true); },
                    on_close: move |_| switching.set(false),
                    BuildDatasetCreation { controller, oncreated: move |_| switching.set(false) }
                }
            }
        }
        BuildDocumentActions { controller, state }
    }
}

/// A new graph over a dataset, as Workflow creates one: the catalog's
/// datasets, and the chosen one names the document.
#[component]
fn BuildDatasetCreation(controller: BuildController, oncreated: EventHandler) -> Element {
    let sessions = crate::shell::project::ProjectSessionSignals::current();
    let datasets = QueryPolicy::on_demand(QueryDomain::Catalog).use_when(
        move || sessions.catalog_available(),
        || DatasetClient::current().list_datasets(),
    );
    let mut selected = use_signal(String::new);
    let response = datasets.read();
    match response.as_ref() {
        Some(Ok(items)) => {
            let source = items
                .iter()
                .find(|source| source.resource_id() == selected())
                .cloned();
            rsx! {
                div { class: "workflow-dataset-creation",
                    label { r#for: "build-dataset-creation", "From dataset" }
                    div {
                        Select {
                            id: "build-dataset-creation", value: "{selected}",
                            disabled: items.is_empty(),
                            onchange: move |value| selected.set(value),
                            option { value: "", disabled: true, if items.is_empty() { "No datasets available" } else { "Choose a dataset" } }
                            for source in items {
                                option { value: "{source.resource_id()}", "{source.selection().label()}" }
                            }
                        }
                        Button {
                            purpose: ActionPurpose::Create,
                            variant: ButtonVariant::Outline,
                            disabled: source.is_none() || controller.command().is_some(),
                            onclick: move |_| {
                                if let Some(source) = source.clone() {
                                    controller.create(source.selection().label(), Some(source.selection().clone()));
                                    oncreated.call(());
                                }
                            },
                            "Create"
                        }
                    }
                }
            }
        }
        Some(Err(error)) => {
            rsx! { p { class: "document-switcher-note", role: "alert", "Could not load datasets: {error}" } }
        }
        None => rsx! { p { class: "document-switcher-note", role: "status", "Loading datasets…" } },
    }
}

/// The state chip stands with the actions that change it, as in Workflow and
/// Curation. Build has no Run: each measurement runs from its own workspace.
#[component]
fn BuildDocumentActions(controller: BuildController, state: DocumentState) -> Element {
    let studio = StudioProjectState::current();
    let pending = controller.command();
    rsx! {
        div { class: "document-actions",
            DocumentStateChip { state }
            if controller.conflict() {
                Button { purpose: ActionPurpose::Sync, variant: ButtonVariant::Outline,
                    disabled: pending.is_some(),
                    title: "Load the version Studio stored, losing local edits",
                    onclick: move |_| controller.open(controller.id()), "Reload current" }
            } else if controller.unsaved_changes() {
                Button { purpose: ActionPurpose::Discard, variant: ButtonVariant::Link,
                    class: "document-discard-action",
                    disabled: pending.is_some(),
                    title: "Return to the saved graph",
                    onclick: move |_| controller.discard(), "Discard" }
            }
            Button { purpose: ActionPurpose::Apply, variant: ButtonVariant::Outline,
                disabled: pending.is_some(),
                aria_busy: pending == Some(BuildPendingCommand::Saving),
                title: "Save the graph and everything it has measured",
                onclick: move |_| controller.save(studio),
                if pending == Some(BuildPendingCommand::Saving) { "Saving…" } else { "Save" }
            }
        }
    }
}
