//! The document bar: which workflow this is, whether it is saved, and the
//! three things you can do to it. The bar itself is shared; this file owns
//! what is Workflow's alone: creation from a dataset, and Run.

use crate::components::page_keys::PageKeys;
use dioxus::prelude::*;

use super::state::WorkflowController;
use crate::components::dataset_picker::DatasetPicker;
use crate::components::document_bar::{
    DocumentBar, DocumentBarCommand, DocumentBarCopy, DocumentBarGate, DocumentEntry,
    DocumentState, OpenDocument,
};
use crate::components::document_session::DocumentCommand;
use crate::components::icons::{PlayIcon, SpinnerIcon, WorkflowIcon};
use crate::shell::state::StudioProjectState;
use spiral_robotics_common::datasets::DatasetSummary;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

const COPY: DocumentBarCopy = DocumentBarCopy::new(
    "workflow",
    "New blank workflow",
    "Save SQL and conversation",
    "Return to the saved SQL and conversation",
);

/// The workflow's identity, its saved state, and what Save, Discard, and Run
/// may do.
#[component]
pub(super) fn WorkflowDocumentControls(controller: WorkflowController) -> Element {
    let studio = StudioProjectState::current();
    let workflow = controller.spec();
    let saved = controller
        .workflows()
        .iter()
        .map(|item| DocumentEntry::new(item.id(), item.name()))
        .collect::<Vec<_>>();
    let running = controller.running();
    let run_reason = controller.run_disabled_reason();
    let run_blocked = run_reason.is_some();
    let run_title = run_reason.unwrap_or_else(|| "Run this SQL in Spiral".to_owned());
    rsx! {
        header { class: "document-bar",
            span { class: "document-glyph", "aria-hidden": "true", WorkflowIcon {} }
            DocumentBar {
                copy: COPY,
                document: OpenDocument::new(
                    DocumentEntry::new(workflow.id(), workflow.name()),
                    saved,
                    DocumentState::of(controller.unsaved_changes()),
                ).saving(controller.command() == Some(DocumentCommand::Saving)),
                gate: DocumentBarGate::new(
                    controller.workflow_selection_reason(),
                    controller.save_disabled_reason(),
                    controller.command().is_some()
                        || controller.agent_state().session().phase().active(),
                ).renaming(controller.command().map(|command| command.waiting_for("workflow"))),
                creation: rsx! { WorkflowDatasetCreation { controller } },
                on_command: move |command| match command {
                    DocumentBarCommand::Open(id) => controller.select_workflow(id, studio),
                    DocumentBarCommand::Rename(name) => controller.set_name(name),
                    DocumentBarCommand::Discard => controller.discard(studio),
                    DocumentBarCommand::Save => controller.save(studio),
                },
                run: rsx! {
                    Button { purpose: ActionPurpose::Start, variant: ButtonVariant::Solid,
                        class: "document-run-action",
                        disabled: run_blocked,
                        aria_busy: running,
                        title: "{run_title}",
                        onclick: move |_| controller.run(studio),
                        if running { SpinnerIcon {} "Running" } else {
                            PlayIcon {} "Run"
                            kbd { class: "ui-kbd", "aria-hidden": "true", "{PageKeys::command_symbol()}↵" }
                        }
                    }
                },
            }
        }
    }
}

/// A new workflow over a dataset: the chosen dataset names it and its episodes
/// become the query's source.
#[component]
fn WorkflowDatasetCreation(controller: WorkflowController) -> Element {
    let chosen = use_signal(|| None::<DatasetSummary>);
    rsx! {
        div { class: "workflow-dataset-creation",
            label { r#for: "workflow-dataset", "From dataset" }
            div {
                DatasetPicker {
                    id: "workflow-dataset".to_owned(),
                    chosen,
                    label: "Dataset for a new workflow",
                }
                Button {
                    purpose: ActionPurpose::Create,
                    variant: ButtonVariant::Outline,
                    disabled: chosen().is_none() || controller.workflow_selection_reason().is_some(),
                    onclick: move |_| {
                        if let Some(source) = chosen() {
                            controller.create_from_dataset(source);
                        }
                    },
                    "Create"
                }
            }
        }
    }
}
