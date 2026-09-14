//! Compact document selection, naming, and creation for the Workflow workspace.

use dioxus::prelude::*;

use super::state::WorkflowController;
use crate::activities::data::DatasetClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::state::StudioProjectState;

#[component]
pub(super) fn WorkflowDocumentControls(controller: WorkflowController) -> Element {
    let studio = StudioProjectState::current();
    let workflow = controller.spec();
    let reason = controller.workflow_selection_reason();
    let mut rename = use_signal(|| None::<String>);
    let mut creating = use_signal(|| false);
    let rename_disabled = controller.save_disabled_reason().is_some();

    rsx! {
        header { class: "workflow-picker",
            label { r#for: "workflow-picker", "Workflow" }
            if let Some(name) = rename() {
                form {
                    class: "workflow-rename",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if !rename_disabled
                            && let Some(name) = rename().filter(|name| !name.trim().is_empty()) {
                            controller.set_name(name.trim().to_owned());
                            rename.set(None);
                        }
                    },
                    onkeydown: move |event| {
                        if event.key() == Key::Escape {
                            event.prevent_default();
                            rename.set(None);
                        }
                    },
                    input {
                        id: "workflow-picker",
                        class: "workflow-name-input",
                        "aria-label": "Workflow name",
                        value: "{name}",
                        disabled: rename_disabled,
                        oninput: move |event| rename.set(Some(event.value())),
                        onmounted: move |event| async move { let _ = event.set_focus(true).await; },
                    }
                    Action {
                        purpose: ActionPurpose::Apply,
                        r#type: "submit",
                        disabled: rename_disabled || name.trim().is_empty(),
                        "Apply"
                    }
                    Action { purpose: ActionPurpose::Cancel, onclick: move |_| rename.set(None), "Cancel" }
                }
            } else {
                select {
                    id: "workflow-picker", value: "{workflow.id()}",
                    disabled: reason.is_some(),
                    onchange: move |event| controller.select_workflow(Some(event.value()), studio),
                    if !controller.workflows().iter().any(|item| item.id() == workflow.id()) {
                        option { value: "{workflow.id()}", selected: true, "{workflow.name()}" }
                    }
                    for item in controller.workflows() {
                        option {
                            value: "{item.id()}", selected: item.id() == workflow.id(),
                            if item.id() == workflow.id() { "{workflow.name()}" } else { "{item.name()}" }
                        }
                    }
                }
                Action {
                    purpose: ActionPurpose::Apply,
                    disabled: rename_disabled,
                    title: "Rename this workflow",
                    onclick: move |_| rename.set(Some(controller.spec().name().to_owned())),
                    "Rename"
                }
            }
            details {
                class: "workflow-create", open: creating(),
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        creating.set(false);
                    }
                },
                summary {
                    "aria-disabled": reason.is_some() || rename().is_some(),
                    onclick: move |event| {
                        event.prevent_default();
                        if controller.workflow_selection_reason().is_none() && rename().is_none() {
                            creating.set(!creating());
                        }
                    },
                    "New workflow"
                }
                if creating() {
                    div { class: "workflow-create-options",
                        Action {
                            purpose: ActionPurpose::Create,
                            variant: ActionVariant::Outline,
                            disabled: reason.is_some(),
                            onclick: move |_| {
                                controller.select_workflow(None, studio);
                                creating.set(false);
                            },
                            "Blank workflow"
                        }
                        WorkflowDatasetCreation { controller, oncreated: move |_| creating.set(false) }
                    }
                }
            }
            if let Some(reason) = reason.as_ref() { span { class: "workflow-selection-reason", title: "{reason}", role: "status", if controller.unsaved_changes() { "Save before switching" } else { "{reason}" } } }
            WorkflowDocumentActions { controller }
        }
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
                label { r#for: "workflow-dataset", "From dataset" }
                select {
                    id: "workflow-dataset", value: "{selected}",
                    disabled: items.is_empty(),
                    onchange: move |event| selected.set(event.value()),
                    option { value: "", disabled: true, if items.is_empty() { "No datasets available" } else { "Choose a dataset" } }
                    for source in items {
                        option { value: "{source.resource_id()}", "{source.selection().label()}" }
                    }
                }
                Action {
                    purpose: ActionPurpose::Create,
                    variant: ActionVariant::Outline,
                    disabled: source.is_none() || controller.workflow_selection_reason().is_some(),
                    onclick: move |_| {
                        if let Some(source) = source.clone() {
                            controller.create_from_dataset(source);
                            oncreated.call(());
                        }
                    },
                    "Create from dataset"
                }
            }
        }
        Some(Err(error)) => rsx! { p { role: "alert", "Could not load datasets: {error}" } },
        None => rsx! { p { role: "status", "Loading datasets…" } },
    }
}

#[component]
fn WorkflowDocumentActions(controller: WorkflowController) -> Element {
    let studio = StudioProjectState::current();
    let dirty = controller.unsaved_changes();
    let command = controller.command();
    let save_reason = controller.save_disabled_reason();
    let run_reason = controller.run_disabled_reason();
    let running = controller.snapshot().phase().active();
    rsx! {
        div { class: "workflow-document-actions",
            span { class: "workflow-document-state", role: "status",
                if dirty { "Draft not saved" } else { "Saved" }
            }
            if controller.revision_conflict() {
                Action { purpose: ActionPurpose::Sync, variant: ActionVariant::Outline,
                    onclick: move |_| controller.reload_current(), "Reload current" }
            } else if dirty {
                Action { purpose: ActionPurpose::Cancel, variant: ActionVariant::Link,
                    disabled: command.is_some() || controller.agent_state().session().phase().active(),
                    onclick: move |_| controller.discard(studio), "Discard" }
            }
            Action { purpose: ActionPurpose::Apply, variant: ActionVariant::Outline,
                disabled: save_reason.is_some(), title: save_reason.unwrap_or_else(|| "Save SQL and conversation".into()),
                onclick: move |_| controller.save(studio),
                if command == Some(super::state::WorkflowPendingCommand::Saving) { "Saving…" } else { "Save" }
            }
            Action { purpose: ActionPurpose::Start, variant: ActionVariant::Solid,
                disabled: run_reason.is_some(), title: run_reason.unwrap_or_else(|| "Run this SQL in Spiral".into()),
                onclick: move |_| controller.run(studio),
                if running { "Running…" } else { "Run" }
            }
        }
    }
}
