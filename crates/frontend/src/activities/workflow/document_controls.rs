//! The document bar: which view is open, which workflow this is, whether it
//! is saved, and the three things you can do to it. The bar itself is shared;
//! this file owns what is Workflow's alone: the view switch, creation from a
//! dataset, and Run.

use dioxus::prelude::*;

use super::explore::graph::{ExplorationDocumentBar, WorkflowView};
use super::explore::state::ExplorationController;
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
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, ModeButton, ModeGroup};

const COPY: DocumentBarCopy = DocumentBarCopy::new(
    "workflow",
    "New blank workflow",
    "Save SQL and conversation",
    "Return to the saved SQL and conversation",
);

/// The bar both views share: which view is open, then that view's document.
#[component]
pub(super) fn WorkflowDocumentControls(
    controller: WorkflowController,
    exploration_controller: ExplorationController,
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
                    checked: view() == WorkflowView::Explore,
                    title: "Assemble a measurement graph over one dataset",
                    onclick: move |_| view.set(WorkflowView::Explore),
                    "Explore"
                }
                ModeButton {
                    purpose: ActionPurpose::Select,
                    checked: view() == WorkflowView::Agent,
                    title: "Author SQL with the Workflow assistant",
                    onclick: move |_| view.set(WorkflowView::Agent),
                    "Agent"
                }
            }
            if view() == WorkflowView::Explore {
                ExplorationDocumentBar { controller: exploration_controller }
            } else {
                WorkflowAgentDocument { controller }
            }
        }
    }
}

/// The Agent view's half of the bar: the workflow's identity, its saved state,
/// and what Save, Discard, and Run may do.
#[component]
fn WorkflowAgentDocument(controller: WorkflowController) -> Element {
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
                    if running { SpinnerIcon {} "Running" } else { PlayIcon {} "Run" }
                }
            },
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
