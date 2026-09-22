//! The document bar: which curation is open, whether it is saved, and the
//! three things you can do to it. The bar itself is shared; what is Curation's
//! alone is Run, which judges every episode with the open recipe. The dataset
//! is named once, in the recipe's Source section.

use dioxus::prelude::*;

use super::state::CurationController;
use crate::components::document_bar::{
    DocumentBar, DocumentBarCommand, DocumentBarCopy, DocumentBarGate, DocumentEntry,
    DocumentState, OpenDocument,
};
use crate::components::document_session::DocumentCommand;
use crate::components::icons::{CurationIcon, PlayIcon, SpinnerIcon};
use crate::shell::state::StudioProjectState;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

const COPY: DocumentBarCopy = DocumentBarCopy::new(
    "curation",
    "New curation",
    "Save the recipe",
    "Return to the saved recipe",
);

#[component]
pub(super) fn CurationDocumentBar(controller: CurationController) -> Element {
    let studio = StudioProjectState::current();
    let curation = controller.spec();
    let saved = controller
        .curations()
        .iter()
        .map(|item| DocumentEntry::new(item.id(), item.name()))
        .collect::<Vec<_>>();
    let judging = controller.judging();
    let run_reason = controller.run_disabled_reason();
    let run_blocked = run_reason.is_some();
    let run_title = run_reason
        .or_else(|| controller.run_problem())
        .unwrap_or_else(|| "Judge every episode with this recipe · ⌘↩".to_owned());
    rsx! {
        header { class: "document-bar",
            span { class: "document-glyph", "aria-hidden": "true", CurationIcon {} }
            DocumentBar {
                copy: COPY,
                document: OpenDocument::new(
                    DocumentEntry::new(curation.id(), curation.name()),
                    saved,
                    DocumentState::of(controller.unsaved_changes()),
                ).saving(controller.command() == Some(DocumentCommand::Saving)),
                gate: DocumentBarGate::new(
                    controller.selection_reason(),
                    controller.save_disabled_reason(),
                    controller.command().is_some(),
                ).renaming(controller.command().map(|command| command.waiting_for("curation"))),
                // A curation names its dataset in the recipe, not here.
                creation: rsx! {},
                on_command: move |command| match command {
                    DocumentBarCommand::Open(id) => controller.select_curation(id),
                    DocumentBarCommand::Rename(name) => controller.set_name(name),
                    DocumentBarCommand::Discard => controller.discard(),
                    DocumentBarCommand::Save => controller.save(studio),
                },
                run: rsx! {
                    Button { purpose: ActionPurpose::Start, variant: ButtonVariant::Solid,
                        class: "document-run-action",
                        disabled: run_blocked,
                        aria_busy: judging,
                        title: "{run_title}",
                        onclick: move |_| controller.run(studio),
                        if judging { SpinnerIcon {} "Judging" } else { PlayIcon {} "Run" }
                    }
                },
            }
        }
    }
}
