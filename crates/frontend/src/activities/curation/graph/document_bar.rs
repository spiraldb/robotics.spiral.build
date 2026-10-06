//! The document bar: which curation is open, whether it is saved, and what
//! may be done to it. It heads the graph pane, because the graph is the
//! document: the name, which is edited where it stands, and the chevron that
//! opens the switcher — saved curations, each with when it was made and its
//! own delete, and New — then, at the far end, unsaved changes as a dot
//! before Discard, Save, and Publish. There is no Run, because each node runs
//! from its own workspace.
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonSize, ButtonVariant, CheckIcon, ChevronIcon,
    ConfirmationCopy, ConfirmationDismissal, ConfirmationResponse, DeleteIcon, InlineConfirmation,
};
use wasm_bindgen::JsCast as _;

use super::super::session::{DocumentCommand, DocumentIdentity, DocumentState};
use super::super::state::CurationController;
use super::publish::CurationPublish;
use crate::shell::state::StudioProjectState;

const NAME_ID: &str = "curation-document-name";
const SWITCH_ID: &str = "curation-document-switch";
const SWITCHER_ID: &str = "curation-switcher";

#[component]
pub(in crate::activities::curation) fn CurationDocumentBar(
    controller: CurationController,
) -> Element {
    let studio = StudioProjectState::current();
    let command = controller.command();
    // Why the bar's actions are refused right now: a document command in
    // flight. The reason both disables an action and becomes its title.
    let busy = command.map(DocumentCommand::waiting_for);
    let saving = command == Some(DocumentCommand::Saving);
    let state = DocumentState::of(controller.unsaved_changes(), controller.is_stored());
    // The switcher remembers which curation it was opened over, so anything
    // that replaces the open one closes it.
    let mut opened_over = use_signal(|| Option::<String>::None);
    let current_id = controller.id();
    let switching = opened_over.read().as_deref() == Some(current_id.as_str());
    // The latest curation opens when Curation first mounts, which is exactly
    // when this bar does.
    use_future(move || async move { controller.open_latest() });
    rsx! {
        header { class: "document-bar",
            div { class: "document-identity",
                DocumentName {
                    name: controller.name(),
                    refused: busy.clone(),
                    on_rename: move |name| controller.rename(name, studio),
                }
                Button {
                    id: SWITCH_ID,
                    purpose: ActionPurpose::Select,
                    variant: ButtonVariant::Quiet,
                    class: "document-switch",
                    aria_haspopup: "dialog",
                    aria_expanded: switching,
                    aria_label: busy.clone().unwrap_or_else(|| "Switch or create curations".to_owned()),
                    title: busy.clone().unwrap_or_else(|| "Switch or create curations".to_owned()),
                    onclick: move |_| opened_over.set((!switching).then(|| current_id.clone())),
                    ChevronIcon { open: false }
                }
                // The switcher hangs from the header, under the name.
                if switching {
                    DocumentSwitcher {
                        controller,
                        busy: busy.clone(),
                        on_close: move |_| opened_over.set(None),
                    }
                }
            }
            // The state leads the actions that settle it, so the dot, Discard,
            // Save, and Publish read as one group.
            div { class: "document-actions",
                DocumentStateDot { state }
                if state == DocumentState::Draft {
                    Button { purpose: ActionPurpose::Discard, variant: ButtonVariant::Link,
                        class: "document-discard-action",
                        disabled: busy.is_some(),
                        title: "Return to the saved graph",
                        onclick: move |_| controller.discard(), "Discard" }
                }
                Button { purpose: ActionPurpose::Apply, variant: ButtonVariant::Outline,
                    disabled: busy.is_some(),
                    aria_busy: saving,
                    title: busy.clone().unwrap_or_else(|| "Save the graph".to_owned()),
                    onclick: move |_| controller.save(studio),
                    if saving { "Saving…" } else { "Save" }
                }
                CurationPublish { controller }
            }
        }
    }
}

/// Whether the curation is saved, as one dot before Save: drawn only while
/// there are unsaved changes, since a saved or untouched graph is the resting
/// state. The words are for the hover and screen readers. The status region
/// stays mounted, because a live region inserted afresh is not reliably
/// announced, while new words inside a standing one are.
#[component]
fn DocumentStateDot(state: DocumentState) -> Element {
    rsx! {
        span { class: "document-state {state.class_name()}", role: "status", title: "{state.label()}",
            span { class: "sr-only", "{state.label()}" }
        }
    }
}

/// The curation's name, edited where it stands as a title is in any editor:
/// click it and type, Enter or leaving it renames, Escape puts the name back.
/// A blank name is never applied.
#[component]
fn DocumentName(
    name: String,
    /// Why the name cannot change right now; the field is read-only and says
    /// so on hover.
    refused: Option<String>,
    on_rename: EventHandler<String>,
) -> Element {
    let mut typed = use_signal(|| name.clone());
    // Another curation opened, or a rename landed: the field shows it.
    use_effect(use_reactive(&name, move |name| typed.set(name)));
    let current = name.clone();
    let commit = move || {
        let trimmed = typed.peek().trim().to_owned();
        if trimmed.is_empty() || trimmed == current {
            typed.set(current.clone());
        } else {
            on_rename.call(trimmed);
        }
    };
    rsx! {
        input {
            id: NAME_ID,
            class: "document-name-field",
            r#type: "text",
            "aria-label": "Curation name",
            title: refused.clone().unwrap_or_else(|| "Curation name; click to rename".to_owned()),
            readonly: refused.is_some(),
            spellcheck: "false",
            autocomplete: "off",
            // Sized to the name, so the chevron sits right after it.
            size: "{typed.read().chars().count().clamp(8, 60)}",
            value: "{typed}",
            oninput: move |event| typed.set(event.value()),
            onblur: {
                let mut commit = commit.clone();
                move |_| commit()
            },
            onkeydown: move |event: KeyboardEvent| match event.key() {
                Key::Enter => {
                    event.prevent_default();
                    DocumentNameFocus::release();
                }
                Key::Escape => {
                    event.prevent_default();
                    typed.set(name.clone());
                    DocumentNameFocus::release();
                }
                _ => {}
            },
        }
    }
}

/// Where focus goes once a name is done with.
struct DocumentNameFocus;

impl DocumentNameFocus {
    /// Leave the field, which is what applies the name.
    fn release() {
        if let Some(field) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(NAME_ID))
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = field.blur();
        }
    }
}

/// When a saved curation was made, read back from its id: the receipt at the
/// end of its switcher row. A new curation takes its dataset's name, so three
/// curations of one dataset are three rows of one name, told apart by this.
struct CreationStamp(js_sys::Date);

impl CreationStamp {
    const MONTHS: [&'static str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    fn of(id: &str) -> Option<Self> {
        let made = js_sys::Date::new(&DocumentIdentity::minted_at(id)?.into());
        (!made.get_time().is_nan()).then_some(Self(made))
    }
}

/// "Oct 3 14:22", on the operator's own clock.
impl std::fmt::Display for CreationStamp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let month = Self::MONTHS
            .get(self.0.get_month() as usize)
            .copied()
            .unwrap_or_default();
        write!(
            formatter,
            "{month} {} {:02}:{:02}",
            self.0.get_date(),
            self.0.get_hours(),
            self.0.get_minutes()
        )
    }
}

/// The popover behind the chevron: every saved curation, each with when it
/// was made and a delete at its end that asks in its own row first, then New.
/// A transparent backdrop closes it.
#[component]
fn DocumentSwitcher(
    controller: CurationController,
    /// Why switching is refused right now.
    busy: Option<String>,
    on_close: EventHandler<()>,
) -> Element {
    let studio = StudioProjectState::current();
    let entries = controller.documents();
    let current_id = controller.id();
    let current_name = controller.name();
    let disabled = busy.is_some();
    // The row asking whether to delete it, if any.
    let mut confirming = use_signal(|| Option::<String>::None);
    // Focus has to land inside the dialog or Escape and the backdrop are dead.
    // `.focus()` on a disabled button is a no-op, so a blocked switcher takes
    // its own container instead of an option nothing can reach.
    let initial_focus = if disabled {
        SWITCHER_ID.to_owned()
    } else {
        entries
            .iter()
            .position(|item| item.id() == current_id)
            .map(|index| format!("{SWITCHER_ID}-option-{index}"))
            .unwrap_or_else(|| format!("{SWITCHER_ID}-new"))
    };
    use_hook(move || BrowserFocus::after_render(initial_focus));
    let close = move || {
        on_close.call(());
        BrowserFocus::after_render(SWITCH_ID.to_owned());
    };
    rsx! {
        div { class: "popover-backdrop", onclick: move |_| close() }
        div {
            id: SWITCHER_ID,
            class: "document-switcher",
            role: "dialog",
            tabindex: "-1",
            "aria-label": "Saved curations",
            onkeydown: move |event: KeyboardEvent| {
                if event.key() == Key::Escape {
                    event.prevent_default();
                    close();
                }
            },
            if let Some(reason) = busy.as_ref() {
                p { class: "document-switcher-note is-blocking", role: "status", "{reason}" }
            }
            if entries.is_empty() {
                p { class: "document-switcher-empty", "No saved curations yet." }
            } else {
                ul { class: "document-switcher-list", "aria-label": "Saved curations",
                    for (index, item) in entries.iter().enumerate() {
                        {
                            let open = item.id() == current_id;
                            let name = if open { current_name.clone() } else { item.name().to_owned() };
                            let option_id = format!("{SWITCHER_ID}-option-{index}");
                            let delete_id = format!("{SWITCHER_ID}-delete-{index}");
                            let asking = confirming.read().as_deref() == Some(item.id());
                            let id = item.id().to_owned();
                            rsx! {
                                li { key: "{item.id()}", class: "document-switcher-row",
                                    if asking {
                                        InlineConfirmation {
                                            id: "{delete_id}-confirmation",
                                            class: "document-switcher-confirmation",
                                            copy: ConfirmationCopy::new(
                                                format!("Delete “{name}”? Its graph and every view it publishes are gone for good."),
                                                "Delete",
                                                "Keep",
                                            ),
                                            response: ConfirmationResponse::new(
                                                ActionPurpose::Delete,
                                                {
                                                    let id = id.clone();
                                                    move |_| {
                                                        confirming.set(None);
                                                        controller.delete(id.clone(), studio);
                                                        close();
                                                    }
                                                },
                                                move |_| confirming.set(None),
                                            ),
                                            dismissal: ConfirmationDismissal::new(delete_id.clone()),
                                        }
                                    } else {
                                        Button {
                                            id: "{option_id}",
                                            purpose: ActionPurpose::Select,
                                            variant: ButtonVariant::Row,
                                            class: "document-switcher-option",
                                            aria_current: open.then_some("true"),
                                            disabled,
                                            onclick: {
                                                let id = id.clone();
                                                move |_| {
                                                    controller.open(id.clone());
                                                    close();
                                                }
                                            },
                                            span { class: "document-switcher-name", "{name}" }
                                            if let Some(made) = CreationStamp::of(item.id()) {
                                                span { class: "document-switcher-made", title: "Created {made}", "{made}" }
                                            }
                                            span { class: "document-switcher-current",
                                                if open { CheckIcon {} }
                                            }
                                        }
                                        // Beside the row rather than in it: a button
                                        // inside a button is one nobody can reach.
                                        Button {
                                            id: "{delete_id}",
                                            purpose: ActionPurpose::Delete,
                                            variant: ButtonVariant::Quiet,
                                            size: ButtonSize::Compact,
                                            class: "document-switcher-delete",
                                            disabled,
                                            title: "Delete “{name}”",
                                            aria_label: "Delete “{name}”",
                                            onclick: move |_| confirming.set(Some(id.clone())),
                                            DeleteIcon {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "document-switcher-actions",
                Button {
                    id: "{SWITCHER_ID}-new",
                    purpose: ActionPurpose::Create,
                    variant: ButtonVariant::Outline,
                    disabled,
                    onclick: move |_| {
                        controller.create();
                        close();
                    },
                    "New blank curation"
                }
            }
        }
    }
}
