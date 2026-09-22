//! The document bar: which view is open, which workflow this is, whether it
//! is saved, and the three things you can do to it. The identity chrome and
//! the switch/create/rename popover are the shared document components; this
//! file owns what is Workflow's alone: the view switch, creation from a
//! dataset, and what Save, Discard, and Run may do.

use dioxus::prelude::*;

use super::build::graph::{BuildDocumentBar, WorkflowView};
use super::build::state::BuildController;
use super::state::{WorkflowController, WorkflowPendingCommand};
use crate::activities::data::DatasetClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::components::document_bar::{
    DocumentEntry, DocumentRenameForm, DocumentState, DocumentStateChip, DocumentSwitcher,
    DocumentTitle, SwitcherCopy, SwitcherGate,
};
use crate::components::icons::{PlayIcon, SpinnerIcon, WorkflowIcon};
use crate::shell::state::StudioProjectState;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, ModeButton, ModeGroup, Select};

const SWITCHER_ID: &str = "workflow-switcher";
const TITLE_ID: &str = "workflow-document-title";

/// The bar both views share: which view is open, then that view's document.
#[component]
pub(super) fn WorkflowDocumentControls(
    controller: WorkflowController,
    build_controller: BuildController,
    /// Which view the bar names and the workspace mounts. The two views share
    /// this bar and nothing else.
    view: Signal<WorkflowView>,
) -> Element {
    let mut view = view;
    rsx! {
        header { class: "document-bar",
            span { class: "document-glyph", "aria-hidden": "true", WorkflowIcon {} }
            ModeGroup { label: "Workflow view".to_owned(), class: "workflow-mode-switch".to_owned(),
                ModeButton {
                    purpose: ActionPurpose::Select,
                    checked: view() == WorkflowView::Build,
                    title: "Assemble a measurement graph over one dataset",
                    onclick: move |_| view.set(WorkflowView::Build),
                    "Build"
                }
                ModeButton {
                    purpose: ActionPurpose::Select,
                    checked: view() == WorkflowView::Agent,
                    title: "Author SQL with the Workflow assistant",
                    onclick: move |_| view.set(WorkflowView::Agent),
                    "Agent"
                }
            }
            if view() == WorkflowView::Build {
                BuildDocumentBar { controller: build_controller }
            } else {
                WorkflowAgentDocument { controller }
            }
        }
    }
}

/// The Agent view's half of the bar: the workflow's identity, its saved
/// state, and what Save, Discard, and Run may do.
#[component]
fn WorkflowAgentDocument(controller: WorkflowController) -> Element {
    let studio = StudioProjectState::current();
    let workflow = controller.spec();
    let reason = controller.workflow_selection_reason();
    let mut renaming = use_signal(|| false);
    let mut switching = use_signal(|| false);
    let rename_disabled = controller.save_disabled_reason().is_some();
    let state = DocumentState::of(controller.revision_conflict(), controller.unsaved_changes());
    let title_hint = reason
        .clone()
        .unwrap_or_else(|| "Switch, create, or rename workflows".to_owned());
    let entries = controller
        .workflows()
        .iter()
        .map(|item| DocumentEntry::new(item.id(), item.name()))
        .collect::<Vec<_>>();

    rsx! {
        div { class: "document-identity",
            if renaming() {
                DocumentRenameForm {
                    title_id: TITLE_ID.to_owned(),
                    label: "Workflow name".to_owned(),
                    initial: workflow.name().to_owned(),
                    disabled: rename_disabled,
                    on_apply: move |name| { controller.set_name(name); renaming.set(false); },
                    on_cancel: move |_| renaming.set(false),
                }
            } else {
                DocumentTitle {
                    id: TITLE_ID.to_owned(),
                    name: workflow.name().to_owned(),
                    hint: title_hint.clone(),
                    expanded: switching(),
                    on_toggle: move |_| switching.toggle(),
                }
            }
            // The switcher hangs from the title it belongs to, wherever
            // the title sits in the bar.
            if switching() {
                DocumentSwitcher {
                    id: SWITCHER_ID.to_owned(),
                    title_id: TITLE_ID.to_owned(),
                    entries,
                    current: DocumentEntry::new(workflow.id(), workflow.name()),
                    copy: SwitcherCopy::new("Saved workflows", "No saved workflows yet.", "New blank workflow", "Rename this workflow"),
                    gate: SwitcherGate::new(reason.clone(), rename_disabled),
                    on_select: move |id: Option<String>| controller.select_workflow(id, studio),
                    on_rename: move |_| { switching.set(false); renaming.set(true); },
                    on_close: move |_| switching.set(false),
                    WorkflowDatasetCreation { controller, oncreated: move |_| switching.set(false) }
                }
            }
        }
        WorkflowDocumentActions { controller, state }
    }
}

#[component]
fn WorkflowDatasetCreation(controller: WorkflowController, oncreated: EventHandler) -> Element {
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
                    label { r#for: "workflow-dataset", "From dataset" }
                    div {
                        Select {
                            id: "workflow-dataset", value: "{selected}",
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
                            disabled: source.is_none() || controller.workflow_selection_reason().is_some(),
                            onclick: move |_| {
                                if let Some(source) = source.clone() {
                                    controller.create_from_dataset(source);
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

/// The state chip stands with the actions that change it, so "Saved" reads
/// beside Save rather than across the bar from it.
#[component]
fn WorkflowDocumentActions(controller: WorkflowController, state: DocumentState) -> Element {
    let studio = StudioProjectState::current();
    let dirty = controller.unsaved_changes();
    let command = controller.command();
    let save_reason = controller.save_disabled_reason();
    let run_reason = controller.run_disabled_reason();
    let running =
        controller.snapshot().phase().active() || command == Some(WorkflowPendingCommand::Starting);
    rsx! {
        div { class: "document-actions",
            DocumentStateChip { state }
            if controller.revision_conflict() {
                Button { purpose: ActionPurpose::Sync, variant: ButtonVariant::Outline,
                    onclick: move |_| controller.reload_current(), "Reload current" }
            } else if dirty {
                Button { purpose: ActionPurpose::Discard, variant: ButtonVariant::Link,
                    class: "document-discard-action",
                    disabled: command.is_some() || controller.agent_state().session().phase().active(),
                    title: "Return to the saved SQL and conversation",
                    onclick: move |_| controller.discard(studio), "Discard" }
            }
            Button { purpose: ActionPurpose::Apply, variant: ButtonVariant::Outline,
                disabled: save_reason.is_some(),
                aria_busy: command == Some(WorkflowPendingCommand::Saving),
                title: save_reason.unwrap_or_else(|| "Save SQL and conversation".into()),
                onclick: move |_| controller.save(studio),
                if command == Some(WorkflowPendingCommand::Saving) { "Saving…" } else { "Save" }
            }
            Button { purpose: ActionPurpose::Start, variant: ButtonVariant::Solid,
                class: "document-run-action",
                disabled: run_reason.is_some(),
                aria_busy: running,
                title: run_reason.unwrap_or_else(|| "Run this SQL in Spiral".into()),
                onclick: move |_| controller.run(studio),
                if running { SpinnerIcon {} "Running" } else { PlayIcon {} "Run" }
            }
        }
    }
}
