//! The document bar: which curation is open, whether it is saved, and what
//! may be done to it. What is Curation's own is starting a graph from a
//! dataset and publishing its outputs; there is no Run, because each node
//! runs from its own workspace.
use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSummary;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

use super::super::state::CurationController;
use super::publish::CurationPublish;
use crate::components::dataset_picker::DatasetPicker;
use crate::components::document_bar::{
    DocumentBar, DocumentBarCommand, DocumentBarCopy, DocumentBarGate, DocumentEntry,
    DocumentState, OpenDocument,
};
use crate::components::document_session::DocumentCommand;
use crate::components::icons::CurationIcon;
use crate::shell::state::StudioProjectState;

const UNTITLED: &str = "Untitled curation";

const COPY: DocumentBarCopy = DocumentBarCopy::new(
    "curation",
    "New blank curation",
    "Save the graph",
    "Return to the saved graph",
);

#[component]
pub(in crate::activities::curation) fn CurationDocumentBar(
    controller: CurationController,
) -> Element {
    let studio = StudioProjectState::current();
    let command = controller.command();
    let waiting = command.map(|command| command.waiting_for("curation"));
    let saved = controller
        .documents()
        .iter()
        .map(|item| DocumentEntry::new(item.id(), item.name()))
        .collect::<Vec<_>>();
    // The list is read when Curation first mounts, which is exactly when
    // this bar does.
    use_future(move || async move { controller.refresh_documents() });
    rsx! {
        header { class: "document-bar",
            span { class: "document-glyph", "aria-hidden": "true", CurationIcon {} }
            DocumentBar {
                copy: COPY,
                document: OpenDocument::new(
                    DocumentEntry::new(controller.id(), controller.name()),
                    saved,
                    DocumentState::of(controller.unsaved_changes(), controller.is_stored()),
                ).saving(command == Some(DocumentCommand::Saving)),
                gate: DocumentBarGate::new(waiting.clone(), waiting.clone(), command.is_some())
                    .renaming(waiting),
                creation: rsx! { CurationDatasetCreation { controller } },
                on_command: move |command| match command {
                    DocumentBarCommand::Open(Some(id)) => controller.open(id),
                    DocumentBarCommand::Open(None) => controller.create(UNTITLED, None),
                    DocumentBarCommand::Rename(name) => controller.set_name(name),
                    DocumentBarCommand::Discard => controller.discard(),
                    DocumentBarCommand::Save => controller.save(studio),
                },
            }
            CurationPublish { controller }
        }
    }
}

/// A new graph over a dataset: the chosen dataset
/// names the document and becomes its source node.
#[component]
fn CurationDatasetCreation(controller: CurationController) -> Element {
    let chosen = use_signal(|| None::<DatasetSummary>);
    rsx! {
        div { class: "workflow-dataset-creation",
            label { r#for: "curation-dataset-creation", "From dataset" }
            div {
                DatasetPicker {
                    id: "curation-dataset-creation".to_owned(),
                    chosen,
                    label: "Dataset for a new curation",
                }
                Button {
                    purpose: ActionPurpose::Create,
                    variant: ButtonVariant::Outline,
                    disabled: chosen().is_none() || controller.command().is_some(),
                    onclick: move |_| {
                        if let Some(source) = chosen() {
                            controller.create(source.selection().display_name(), Some(source.selection().clone()));
                        }
                    },
                    "Create"
                }
            }
        }
    }
}
