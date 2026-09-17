//! The document bar: which curation is open, whether it is saved, and the
//! three things you can do to it. Identity chrome and the switch/create/
//! rename popover are the shared document components; the dataset is named
//! once, in the recipe's Source section.

use dioxus::prelude::*;

use super::state::{CurationController, CurationPendingCommand};
use crate::components::document_bar::{
    DocumentEntry, DocumentRenameForm, DocumentState, DocumentStateChip, DocumentSwitcher,
    DocumentTitle, SwitcherCopy, SwitcherGate,
};
use crate::components::icons::{CurationIcon, PlayIcon, SpinnerIcon};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::state::StudioProjectState;

const SWITCHER_ID: &str = "curation-switcher";
const TITLE_ID: &str = "curation-document-title";

#[component]
pub(super) fn CurationDocumentBar(controller: CurationController) -> Element {
    let curation = controller.spec();
    let reason = controller.selection_reason();
    let mut renaming = use_signal(|| false);
    let mut switching = use_signal(|| false);
    let rename_disabled = controller.save_disabled_reason().is_some();
    let state = DocumentState::of(controller.revision_conflict(), controller.unsaved_changes());
    let title_hint = reason
        .clone()
        .unwrap_or_else(|| "Switch, create, or rename curations".to_owned());
    let entries = controller
        .curations()
        .iter()
        .map(|item| DocumentEntry::new(item.id(), item.name()))
        .collect::<Vec<_>>();

    rsx! {
        header { class: "document-bar",
            span { class: "document-glyph", "aria-hidden": "true", CurationIcon {} }
            div { class: "document-identity",
                if renaming() {
                    DocumentRenameForm {
                        title_id: TITLE_ID.to_owned(),
                        label: "Curation name".to_owned(),
                        initial: curation.name().to_owned(),
                        disabled: rename_disabled,
                        on_apply: move |name| { controller.set_name(name); renaming.set(false); },
                        on_cancel: move |_| renaming.set(false),
                    }
                } else {
                    DocumentTitle {
                        id: TITLE_ID.to_owned(),
                        name: curation.name().to_owned(),
                        hint: title_hint.clone(),
                        expanded: switching(),
                        on_toggle: move |_| switching.toggle(),
                    }
                    DocumentStateChip { state }
                }
                if switching() {
                    DocumentSwitcher {
                        id: SWITCHER_ID.to_owned(),
                        title_id: TITLE_ID.to_owned(),
                        entries,
                        current: DocumentEntry::new(curation.id(), curation.name()),
                        copy: SwitcherCopy::new("Saved curations", "No saved curations yet.", "New curation", "Rename this curation"),
                        gate: SwitcherGate::new(reason.clone(), rename_disabled),
                        on_select: move |id: Option<String>| controller.select_curation(id),
                        on_rename: move |_| { switching.set(false); renaming.set(true); },
                        on_close: move |_| switching.set(false),
                    }
                }
            }
            CurationDocumentActions { controller }
        }
    }
}

#[component]
fn CurationDocumentActions(controller: CurationController) -> Element {
    let studio = StudioProjectState::current();
    let dirty = controller.unsaved_changes();
    let command = controller.command();
    let save_reason = controller.save_disabled_reason();
    let run_reason = controller.run_disabled_reason();
    let run_problem = controller.run_problem();
    let running =
        controller.snapshot().phase().active() || command == Some(CurationPendingCommand::Starting);
    rsx! {
        div { class: "document-actions",
            if controller.revision_conflict() {
                Action { purpose: ActionPurpose::Sync, variant: ActionVariant::Outline,
                    onclick: move |_| controller.reload_current(), "Reload current" }
            } else if dirty {
                Action { purpose: ActionPurpose::Discard, variant: ActionVariant::Link,
                    class: "document-discard-action",
                    disabled: command.is_some(),
                    title: "Return to the saved recipe",
                    onclick: move |_| controller.discard(), "Discard" }
            }
            Action { purpose: ActionPurpose::Apply, variant: ActionVariant::Outline,
                disabled: save_reason.is_some(),
                aria_busy: command == Some(CurationPendingCommand::Saving),
                title: save_reason.unwrap_or_else(|| "Save the recipe".into()),
                onclick: move |_| controller.save(studio),
                if command == Some(CurationPendingCommand::Saving) { "Saving…" } else { "Save" }
            }
            Action { purpose: ActionPurpose::Start, variant: ActionVariant::Solid,
                class: "document-run-action",
                disabled: run_reason.is_some(),
                aria_busy: running,
                title: run_reason.or(run_problem).unwrap_or_else(|| "Judge every episode with this recipe · ⌘↩".into()),
                onclick: move |_| controller.run(studio),
                if running { SpinnerIcon {} "Judging" } else { PlayIcon {} "Run" }
            }
        }
    }
}
