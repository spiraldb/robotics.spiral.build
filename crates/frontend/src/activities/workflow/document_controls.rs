//! The document bar: which view is open, which workflow this is, whether it
//! is saved, and the three things you can do to it. Switching, creating, and
//! renaming live in one popover behind the title so the bar itself stays a
//! single quiet line.

use dioxus::prelude::*;

use super::state::{WorkflowController, WorkflowPendingCommand};
use crate::activities::data::DatasetClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::components::icons::{CheckIcon, PlayIcon, SpinnerIcon, WorkflowIcon};
use crate::components::interaction::{
    Action, ActionPurpose, ActionVariant, BrowserFocus, ChevronIcon,
};
use crate::shell::state::StudioProjectState;

const SWITCHER_ID: &str = "workflow-switcher";
const TITLE_ID: &str = "workflow-document-title";

#[component]
pub(super) fn WorkflowDocumentControls(
    controller: WorkflowController,
    comparing: Signal<bool>,
) -> Element {
    let workflow = controller.spec();
    let reason = controller.workflow_selection_reason();
    let mut rename = use_signal(|| None::<String>);
    let mut switching = use_signal(|| false);
    let rename_disabled = controller.save_disabled_reason().is_some();
    let state = DocumentState::of(controller);
    let title_hint = reason
        .clone()
        .unwrap_or_else(|| "Switch, create, or rename workflows".to_owned());

    rsx! {
        header { class: "workflow-document-bar",
            span { class: "workflow-document-glyph", "aria-hidden": "true", WorkflowIcon {} }
            super::workspace::WorkflowViewSwitch { comparing }
            div { class: "workflow-document-identity",
                if let Some(name) = rename() {
                    form {
                        class: "workflow-rename",
                        onsubmit: move |event| {
                            event.prevent_default();
                            if !rename_disabled
                                && let Some(name) = rename().filter(|name| !name.trim().is_empty()) {
                                controller.set_name(name.trim().to_owned());
                                rename.set(None);
                                BrowserFocus::after_render(TITLE_ID.to_owned());
                            }
                        },
                        onkeydown: move |event| {
                            if event.key() == Key::Escape {
                                event.prevent_default();
                                rename.set(None);
                                BrowserFocus::after_render(TITLE_ID.to_owned());
                            }
                        },
                        input {
                            class: "workflow-name-input",
                            "aria-label": "Workflow name",
                            value: "{name}",
                            disabled: rename_disabled,
                            oninput: move |event| rename.set(Some(event.value())),
                            onmounted: move |event| async move { let _ = event.set_focus(true).await; },
                        }
                        Action {
                            purpose: ActionPurpose::Apply,
                            variant: ActionVariant::Outline,
                            r#type: "submit",
                            disabled: rename_disabled || name.trim().is_empty(),
                            "Apply"
                        }
                        Action {
                            purpose: ActionPurpose::Cancel,
                            variant: ActionVariant::Link,
                            onclick: move |_| {
                                rename.set(None);
                                BrowserFocus::after_render(TITLE_ID.to_owned());
                            },
                            "Cancel"
                        }
                    }
                } else {
                    Action {
                        id: TITLE_ID,
                        purpose: ActionPurpose::Select,
                        variant: ActionVariant::Quiet,
                        class: "workflow-document-title",
                        aria_haspopup: "listbox",
                        aria_expanded: switching(),
                        title: "{title_hint}",
                        onclick: move |_| switching.toggle(),
                        span { class: "workflow-document-name", "{workflow.name()}" }
                        ChevronIcon { open: false }
                    }
                    WorkflowDocumentState { state }
                }
                // The switcher hangs from the title it belongs to, wherever
                // the title sits in the bar.
                if switching() {
                    WorkflowSwitcher {
                        controller,
                        on_close: move |_| switching.set(false),
                        on_rename: move |_| {
                            switching.set(false);
                            rename.set(Some(controller.spec().name().to_owned()));
                        },
                    }
                }
            }
            WorkflowDocumentActions { controller }
        }
    }
}

/// Keyed on the state so a change replays its settle motion once.
#[component]
fn WorkflowDocumentState(state: DocumentState) -> Element {
    rsx! {
        span {
            key: "{state.class_name()}",
            class: "workflow-document-state {state.class_name()}",
            role: "status",
            span { class: "workflow-document-state-dot", "aria-hidden": "true" }
            "{state.label()}"
        }
    }
}

/// Whether what is on screen matches what Spiral has stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DocumentState {
    Saved,
    Draft,
    Conflict,
}

impl DocumentState {
    fn of(controller: WorkflowController) -> Self {
        if controller.revision_conflict() {
            Self::Conflict
        } else if controller.unsaved_changes() {
            Self::Draft
        } else {
            Self::Saved
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Saved => "Saved",
            Self::Draft => "Unsaved changes",
            Self::Conflict => "Changed in another session",
        }
    }

    const fn class_name(self) -> &'static str {
        match self {
            Self::Saved => "is-saved",
            Self::Draft => "is-draft",
            Self::Conflict => "is-conflict",
        }
    }
}

#[component]
fn WorkflowSwitcher(
    controller: WorkflowController,
    on_close: EventHandler<()>,
    on_rename: EventHandler<()>,
) -> Element {
    let studio = StudioProjectState::current();
    let current = controller.spec();
    let workflows = controller.workflows();
    let reason = controller.workflow_selection_reason();
    let disabled = reason.is_some();
    let rename_disabled = controller.save_disabled_reason().is_some();
    let initial_focus = workflows
        .iter()
        .position(|item| item.id() == current.id())
        .map(|index| format!("{SWITCHER_ID}-option-{index}"))
        .unwrap_or_else(|| format!("{SWITCHER_ID}-new"));
    use_hook(move || BrowserFocus::after_render(initial_focus));
    let close = move || {
        on_close.call(());
        BrowserFocus::after_render(TITLE_ID.to_owned());
    };
    rsx! {
        div { class: "workflow-popover-backdrop", onclick: move |_| close() }
        div {
            id: SWITCHER_ID,
            class: "workflow-switcher",
            role: "dialog",
            "aria-label": "Workflows",
            onkeydown: move |event| {
                if event.key() == Key::Escape {
                    event.prevent_default();
                    close();
                }
            },
            if let Some(reason) = reason.as_ref() {
                p { class: "workflow-switcher-note is-blocking", role: "status", "{reason}" }
            }
            if workflows.is_empty() {
                p { class: "workflow-switcher-empty", "No saved workflows yet." }
            } else {
                ul { class: "workflow-switcher-list", role: "listbox", "aria-label": "Saved workflows",
                    for (index, item) in workflows.iter().enumerate() {
                        li { key: "{item.id()}", role: "none",
                            Action {
                                id: "{SWITCHER_ID}-option-{index}",
                                role: "option",
                                purpose: ActionPurpose::Select,
                                variant: ActionVariant::Row,
                                class: "workflow-switcher-option",
                                aria_selected: item.id() == current.id(),
                                disabled,
                                onclick: {
                                    let id = item.id().to_owned();
                                    move |_| {
                                        controller.select_workflow(Some(id.clone()), studio);
                                        close();
                                    }
                                },
                                span { class: "workflow-switcher-name",
                                    if item.id() == current.id() { "{current.name()}" } else { "{item.name()}" }
                                }
                                if item.id() == current.id() { CheckIcon {} }
                            }
                        }
                    }
                }
            }
            div { class: "workflow-switcher-actions",
                Action {
                    id: "{SWITCHER_ID}-new",
                    purpose: ActionPurpose::Create,
                    variant: ActionVariant::Outline,
                    disabled,
                    onclick: move |_| {
                        controller.select_workflow(None, studio);
                        close();
                    },
                    "New blank workflow"
                }
                WorkflowDatasetCreation { controller, oncreated: move |_| close() }
                Action {
                    purpose: ActionPurpose::Apply,
                    variant: ActionVariant::Link,
                    class: "workflow-switcher-rename",
                    disabled: rename_disabled,
                    onclick: move |_| on_rename.call(()),
                    "Rename this workflow"
                }
            }
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
                div { class: "workflow-dataset-creation",
                    label { r#for: "workflow-dataset", "From dataset" }
                    div {
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
                            "Create"
                        }
                    }
                }
            }
        }
        Some(Err(error)) => {
            rsx! { p { class: "workflow-switcher-note", role: "alert", "Could not load datasets: {error}" } }
        }
        None => rsx! { p { class: "workflow-switcher-note", role: "status", "Loading datasets…" } },
    }
}

#[component]
fn WorkflowDocumentActions(controller: WorkflowController) -> Element {
    let studio = StudioProjectState::current();
    let dirty = controller.unsaved_changes();
    let command = controller.command();
    let save_reason = controller.save_disabled_reason();
    let run_reason = controller.run_disabled_reason();
    let running =
        controller.snapshot().phase().active() || command == Some(WorkflowPendingCommand::Starting);
    rsx! {
        div { class: "workflow-document-actions",
            if controller.revision_conflict() {
                Action { purpose: ActionPurpose::Sync, variant: ActionVariant::Outline,
                    onclick: move |_| controller.reload_current(), "Reload current" }
            } else if dirty {
                Action { purpose: ActionPurpose::Discard, variant: ActionVariant::Link,
                    class: "workflow-discard-action",
                    disabled: command.is_some() || controller.agent_state().session().phase().active(),
                    title: "Return to the saved SQL and conversation",
                    onclick: move |_| controller.discard(studio), "Discard" }
            }
            Action { purpose: ActionPurpose::Apply, variant: ActionVariant::Outline,
                disabled: save_reason.is_some(),
                aria_busy: command == Some(WorkflowPendingCommand::Saving),
                title: save_reason.unwrap_or_else(|| "Save SQL and conversation".into()),
                onclick: move |_| controller.save(studio),
                if command == Some(WorkflowPendingCommand::Saving) { "Saving…" } else { "Save" }
            }
            Action { purpose: ActionPurpose::Start, variant: ActionVariant::Solid,
                class: "workflow-run-action",
                disabled: run_reason.is_some(),
                aria_busy: running,
                title: run_reason.unwrap_or_else(|| "Run this SQL in Spiral".into()),
                onclick: move |_| controller.run(studio),
                if running { SpinnerIcon {} "Running" } else { PlayIcon {} "Run" }
            }
        }
    }
}
