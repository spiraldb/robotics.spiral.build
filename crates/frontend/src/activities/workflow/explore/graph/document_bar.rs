//! The Explore half of Workflow's document bar: which graph is open, whether
//! it is saved, and what may be done to it. The bar itself is shared; what is
//! Explore's alone is starting a graph from a dataset, and having no Run —
//! each measurement runs from its own workspace.
use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSummary;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

use super::super::state::ExplorationController;
use crate::components::dataset_picker::DatasetPicker;
use crate::components::document_bar::{
    DocumentBar, DocumentBarCommand, DocumentBarCopy, DocumentBarGate, DocumentEntry,
    DocumentState, OpenDocument,
};
use crate::components::document_session::DocumentCommand;
use crate::shell::state::StudioProjectState;

const UNTITLED: &str = "Untitled exploration";

const COPY: DocumentBarCopy = DocumentBarCopy::new(
    "exploration",
    "New blank exploration",
    "Save the graph and everything it has measured",
    "Return to the saved graph",
);

#[component]
pub(in crate::activities::workflow) fn ExplorationDocumentBar(
    controller: ExplorationController,
) -> Element {
    let studio = StudioProjectState::current();
    let command = controller.command();
    let waiting = command.map(|command| command.waiting_for("exploration"));
    let saved = controller
        .documents()
        .iter()
        .map(|item| DocumentEntry::new(item.id(), item.name()))
        .collect::<Vec<_>>();
    // The list is read when Explore becomes the open view, which is exactly
    // when this bar mounts.
    use_future(move || async move { controller.refresh_documents() });
    rsx! {
        DocumentBar {
            copy: COPY,
            document: OpenDocument::new(
                DocumentEntry::new(controller.id(), controller.name()),
                saved,
                DocumentState::of(controller.unsaved_changes()),
            ).saving(command == Some(DocumentCommand::Saving)),
            gate: DocumentBarGate::new(waiting.clone(), waiting.clone(), command.is_some())
                .renaming(waiting),
            creation: rsx! { ExploreDatasetCreation { controller } },
            run: rsx! {},
            on_command: move |command| match command {
                DocumentBarCommand::Open(Some(id)) => controller.open(id),
                DocumentBarCommand::Open(None) => controller.create(UNTITLED, None),
                DocumentBarCommand::Rename(name) => controller.set_name(name),
                DocumentBarCommand::Discard => controller.discard(),
                DocumentBarCommand::Save => controller.save(studio),
            },
        }
    }
}

/// A new graph over a dataset, as Workflow creates one: the chosen dataset
/// names the document and becomes its source node.
#[component]
fn ExploreDatasetCreation(controller: ExplorationController) -> Element {
    let chosen = use_signal(|| None::<DatasetSummary>);
    rsx! {
        div { class: "workflow-dataset-creation",
            label { r#for: "exploration-dataset-creation", "From dataset" }
            div {
                DatasetPicker {
                    id: "exploration-dataset-creation".to_owned(),
                    chosen,
                    label: "Dataset for a new exploration",
                }
                Button {
                    purpose: ActionPurpose::Create,
                    variant: ButtonVariant::Outline,
                    disabled: chosen().is_none() || controller.command().is_some(),
                    onclick: move |_| {
                        if let Some(source) = chosen() {
                            controller.create(source.selection().label(), Some(source.selection().clone()));
                        }
                    },
                    "Create"
                }
            }
        }
    }
}
