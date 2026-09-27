//! One document bar for an activity that opens documents: the saved state as
//! a dot, the name, which is edited where it stands, the chevron that opens
//! the switcher — saved documents, each with its own delete, and the ways to
//! make one — then Discard and Save.
//!
//! What is the activity's own is a value the caller supplies: the words for
//! this kind of document ([`DocumentBarCopy`]), why an action is refused right
//! now ([`DocumentBarGate`]), the extra creation control inside the
//! switcher, and the extra actions after Save. Everything the operator
//! presses arrives back as one [`DocumentBarCommand`], because opening and
//! discarding reinstall whatever the activity keeps beside the document.

use crate::components::icons::CheckIcon;
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonSize, ButtonVariant, ChevronIcon, ConfirmationCopy,
    ConfirmationDismissal, ConfirmationResponse, DeleteIcon, InlineConfirmation,
};
use wasm_bindgen::JsCast as _;

/// The saved state the dot below reports now lives with the session that owns
/// it; the bars that import it from here keep working.
pub(crate) use crate::components::document_session::DocumentState;

/// Whether the document is saved, as one dot before its name: green when
/// Studio holds what is on screen, red when it does not. The words are for
/// the hover and screen readers. The status region stays mounted, because a
/// live region inserted afresh is not reliably announced, while new words
/// inside a standing one are.
#[component]
fn DocumentStateDot(state: DocumentState) -> Element {
    rsx! {
        span { class: "document-state {state.class_name()}", role: "status", title: "{state.label()}",
            span { class: "sr-only", "{state.label()}" }
        }
    }
}

/// The document's name, edited where it stands as a title is in any editor:
/// click it and type, Enter or leaving it renames, Escape puts the name back.
/// A blank name is never applied.
#[component]
fn DocumentName(
    id: String,
    label: String,
    name: String,
    /// Why the name cannot change right now; the field is read-only and says
    /// so on hover.
    refused: Option<String>,
    on_rename: EventHandler<String>,
) -> Element {
    let mut typed = use_signal(|| name.clone());
    // Another document opened, or a rename landed: the field shows it.
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
    let field = id.clone();
    rsx! {
        input {
            id: "{id}",
            class: "document-name-field",
            r#type: "text",
            "aria-label": "{label}",
            title: refused.clone().unwrap_or_else(|| format!("{label}; click to rename")),
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
                    DocumentNameFocus::release(&field);
                }
                Key::Escape => {
                    event.prevent_default();
                    typed.set(name.clone());
                    DocumentNameFocus::release(&field);
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
    fn release(id: &str) {
        if let Some(field) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(id))
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = field.blur();
        }
    }
}

/// The chevron after the name that opens the switcher.
#[component]
fn DocumentSwitch(
    id: String,
    hint: String,
    expanded: bool,
    on_toggle: EventHandler<()>,
) -> Element {
    rsx! {
        Button {
            id: "{id}",
            purpose: ActionPurpose::Select,
            variant: ButtonVariant::Quiet,
            class: "document-switch",
            aria_haspopup: "dialog",
            aria_expanded: expanded,
            aria_label: "{hint}",
            title: "{hint}",
            onclick: move |_| on_toggle.call(()),
            ChevronIcon { open: false }
        }
    }
}

/// One saved document as the switcher lists it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DocumentEntry {
    id: String,
    name: String,
}

impl DocumentEntry {
    pub(crate) fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

/// The words one kind of document is called by, and what its two buttons
/// promise. Every other line the bar speaks is built from `noun`; its plural
/// is that word plus an `s`, which is also the collection Studio serves it
/// under.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DocumentBarCopy {
    noun: &'static str,
    create: &'static str,
    save_hint: &'static str,
    discard_hint: &'static str,
    /// What a delete takes with it, said in the row that asks.
    delete_warning: &'static str,
}

impl DocumentBarCopy {
    pub(crate) const fn new(
        noun: &'static str,
        create: &'static str,
        save_hint: &'static str,
        discard_hint: &'static str,
    ) -> Self {
        Self {
            noun,
            create,
            save_hint,
            discard_hint,
            delete_warning: "It is gone for good.",
        }
    }
    /// Say what else a delete takes with it.
    pub(crate) const fn deleting(mut self, warning: &'static str) -> Self {
        self.delete_warning = warning;
        self
    }
    fn switch_hint(self) -> String {
        format!("Switch or create {}s", self.noun)
    }
    fn name_label(self) -> String {
        format!("{}{} name", self.noun[..1].to_uppercase(), &self.noun[1..])
    }
    fn dialog_label(self) -> String {
        format!("Saved {}s", self.noun)
    }
    fn empty(self) -> String {
        format!("No saved {}s yet.", self.noun)
    }
}

/// Why each of the bar's actions is unavailable right now, in the operator's
/// words. `None` offers the action; a reason both disables it and becomes its
/// title, so the answer is one hover away.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DocumentBarGate {
    /// Blocks the switcher.
    switching: Option<String>,
    saving: Option<String>,
    /// Renaming answers only to itself: a document with nothing left to save
    /// is still a document the operator may want to call something else.
    renaming: Option<String>,
    /// Discard is refused while a command is running.
    discarding: bool,
}

impl DocumentBarGate {
    pub(crate) fn new(switching: Option<String>, saving: Option<String>, discarding: bool) -> Self {
        Self {
            switching,
            saving,
            renaming: None,
            discarding,
        }
    }
    /// Why the name cannot be edited right now. Unset — the default — offers
    /// the rename whatever the save gate says.
    pub(crate) fn renaming(mut self, reason: Option<String>) -> Self {
        self.renaming = reason;
        self
    }
}

/// The open document as the bar shows it: what it is called, what else can be
/// opened, and whether it matches what Studio stored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OpenDocument {
    current: DocumentEntry,
    saved: Vec<DocumentEntry>,
    state: DocumentState,
    /// A save is in flight: Save reads "Saving…" and reports itself busy.
    saving: bool,
}

impl OpenDocument {
    pub(crate) fn new(
        current: DocumentEntry,
        saved: Vec<DocumentEntry>,
        state: DocumentState,
    ) -> Self {
        Self {
            current,
            saved,
            state,
            saving: false,
        }
    }
    pub(crate) fn saving(mut self, saving: bool) -> Self {
        self.saving = saving;
        self
    }
}

/// What the operator asked of the open document. The activity answers it,
/// because opening and discarding reinstall what it keeps beside the
/// document — a conversation, a selection, a decoded graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DocumentBarCommand {
    /// Open this saved document, or start a new one when `None`.
    Open(Option<String>),
    Rename(String),
    /// Delete this saved document, open or not.
    Delete(String),
    Discard,
    Save,
}

#[derive(Props, Clone, PartialEq)]
pub(crate) struct DocumentBarProps {
    copy: DocumentBarCopy,
    document: OpenDocument,
    gate: DocumentBarGate,
    on_command: EventHandler<DocumentBarCommand>,
    /// Extra creation controls inside the switcher, such as "From dataset".
    creation: Element,
    /// Extra actions after Save, such as Publish.
    actions: Element,
}

/// The identity half and the action half of one document bar. The
/// surrounding `header` stays with the activity.
#[component]
pub(crate) fn DocumentBar(props: DocumentBarProps) -> Element {
    // The switcher remembers which document it was opened over, so anything
    // that replaces the open document — picking a saved one, creating one from
    // a dataset — closes it without the creation controls knowing it is there.
    let mut opened_over = use_signal(|| Option::<String>::None);
    let copy = props.copy;
    let switch_id = format!("{}-document-switch", copy.noun);
    let document = props.document;
    let gate = props.gate;
    let on_command = props.on_command;
    let current_id = document.current.id.clone();
    let switching = opened_over.read().as_deref() == Some(current_id.as_str());
    rsx! {
        div { class: "document-identity",
            DocumentStateDot { state: document.state }
            DocumentName {
                id: format!("{}-document-name", copy.noun),
                label: copy.name_label(),
                name: document.current.name.clone(),
                refused: gate.renaming.clone(),
                on_rename: move |name| on_command.call(DocumentBarCommand::Rename(name)),
            }
            DocumentSwitch {
                id: switch_id.clone(),
                hint: gate.switching.clone().unwrap_or_else(|| copy.switch_hint()),
                expanded: switching,
                on_toggle: move |_| {
                    let opened = (!switching).then(|| current_id.clone());
                    opened_over.set(opened);
                },
            }
            // The switcher hangs from the activity's header, under the name.
            if switching {
                DocumentSwitcher {
                    id: format!("{}-switcher", copy.noun),
                    return_id: switch_id,
                    copy,
                    gate: gate.clone(),
                    entries: document.saved.clone(),
                    current: document.current.clone(),
                    on_command: move |command| {
                        opened_over.set(None);
                        on_command.call(command);
                    },
                    on_close: move |_| opened_over.set(None),
                    {props.creation}
                }
            }
        }
        div { class: "document-actions",
            if document.state == DocumentState::Draft {
                Button { purpose: ActionPurpose::Discard, variant: ButtonVariant::Link,
                    class: "document-discard-action",
                    disabled: gate.discarding,
                    title: "{copy.discard_hint}",
                    onclick: move |_| on_command.call(DocumentBarCommand::Discard), "Discard" }
            }
            Button { purpose: ActionPurpose::Apply, variant: ButtonVariant::Outline,
                disabled: gate.saving.is_some(),
                aria_busy: document.saving,
                title: gate.saving.clone().unwrap_or_else(|| copy.save_hint.to_owned()),
                onclick: move |_| on_command.call(DocumentBarCommand::Save),
                if document.saving { "Saving…" } else { "Save" }
            }
            {props.actions}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct DocumentSwitcherProps {
    id: String,
    /// What takes focus back when the switcher closes: the chevron.
    return_id: String,
    entries: Vec<DocumentEntry>,
    current: DocumentEntry,
    copy: DocumentBarCopy,
    gate: DocumentBarGate,
    /// Open a saved document or a new one, or delete a saved one.
    on_command: EventHandler<DocumentBarCommand>,
    on_close: EventHandler<()>,
    /// Extra creation controls, such as "From dataset".
    #[props(default)]
    children: Element,
}

/// The popover behind the chevron: every saved document, each with a delete
/// at its end that asks in its own row first, then New and whatever else the
/// activity offers for creation. A transparent backdrop closes it.
#[component]
fn DocumentSwitcher(props: DocumentSwitcherProps) -> Element {
    let switcher_id = props.id.clone();
    let disabled = props.gate.switching.is_some();
    // The row asking whether to delete it, if any.
    let mut confirming = use_signal(|| Option::<String>::None);
    // Focus has to land inside the dialog or Escape and the backdrop are dead.
    // `.focus()` on a disabled button is a no-op, so a blocked switcher takes
    // its own container instead of an option nothing can reach.
    let initial_focus = if disabled {
        switcher_id.clone()
    } else {
        props
            .entries
            .iter()
            .position(|item| item.id == props.current.id)
            .map(|index| format!("{switcher_id}-option-{index}"))
            .unwrap_or_else(|| format!("{switcher_id}-new"))
    };
    use_hook(move || BrowserFocus::after_render(initial_focus));
    let on_close = props.on_close;
    let on_command = props.on_command;
    let return_id = props.return_id.clone();
    let close = move || {
        on_close.call(());
        BrowserFocus::after_render(return_id.clone());
    };
    rsx! {
        div { class: "popover-backdrop", onclick: { let close = close.clone(); move |_| close() } }
        div {
            id: "{switcher_id}",
            class: "document-switcher",
            role: "dialog",
            tabindex: "-1",
            "aria-label": "{props.copy.dialog_label()}",
            onkeydown: {
                let close = close.clone();
                move |event: KeyboardEvent| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        close();
                    }
                }
            },
            if let Some(reason) = props.gate.switching.as_ref() {
                p { class: "document-switcher-note is-blocking", role: "status", "{reason}" }
            }
            if props.entries.is_empty() {
                p { class: "document-switcher-empty", "{props.copy.empty()}" }
            } else {
                ul { class: "document-switcher-list", "aria-label": "{props.copy.dialog_label()}",
                    for (index, item) in props.entries.iter().enumerate() {
                        {
                            let open = item.id == props.current.id;
                            let name = if open { props.current.name.clone() } else { item.name.clone() };
                            let option_id = format!("{switcher_id}-option-{index}");
                            let delete_id = format!("{switcher_id}-delete-{index}");
                            let asking = confirming.read().as_deref() == Some(item.id.as_str());
                            rsx! {
                                li { key: "{item.id}", class: "document-switcher-row",
                                    if asking {
                                        InlineConfirmation {
                                            id: "{delete_id}-confirmation",
                                            class: "document-switcher-confirmation",
                                            copy: ConfirmationCopy::new(
                                                format!("Delete “{name}”? {}", props.copy.delete_warning),
                                                "Delete",
                                                "Keep",
                                            ),
                                            response: ConfirmationResponse::new(
                                                ActionPurpose::Delete,
                                                {
                                                    let id = item.id.clone();
                                                    let close = close.clone();
                                                    move |_| {
                                                        confirming.set(None);
                                                        on_command.call(DocumentBarCommand::Delete(id.clone()));
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
                                                let id = item.id.clone();
                                                let close = close.clone();
                                                move |_| {
                                                    on_command.call(DocumentBarCommand::Open(Some(id.clone())));
                                                    close();
                                                }
                                            },
                                            span { class: "document-switcher-name", "{name}" }
                                            if open { CheckIcon {} }
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
                                            onclick: {
                                                let id = item.id.clone();
                                                move |_| confirming.set(Some(id.clone()))
                                            },
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
                    id: "{switcher_id}-new",
                    purpose: ActionPurpose::Create,
                    variant: ButtonVariant::Outline,
                    disabled,
                    onclick: {
                        let close = close.clone();
                        move |_| {
                            on_command.call(DocumentBarCommand::Open(None));
                            close();
                        }
                    },
                    "{props.copy.create}"
                }
                {props.children}
            }
        }
    }
}
