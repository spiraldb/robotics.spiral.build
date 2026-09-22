//! One document bar for every activity that opens documents: the name that
//! opens the switcher, the saved state, the inline rename form, the
//! switch/create/rename popover, Discard and Save — and Curation's
//! executed-SQL disclosure.
//!
//! Workflow, Curation, and Explore differ in four things only, and each is a
//! value the caller supplies: the words for this kind of document
//! ([`DocumentBarCopy`]), why an action is refused right now
//! ([`DocumentBarGate`]), the extra creation control inside the switcher, and
//! the activity's own Run button. Everything the operator presses arrives back
//! as one [`DocumentBarCommand`], because opening and discarding reinstall
//! whatever the activity keeps beside the document.

use crate::components::icons::{CheckIcon, SqlIcon};
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonVariant, ChevronIcon, Disclosure, TextInput,
};

/// The saved state the chip below reports now lives with the session that owns
/// it; the bars that import it from here keep working.
pub(crate) use crate::components::document_session::DocumentState;

/// Keyed on the state so a change replays its settle motion once.
#[component]
fn DocumentStateChip(state: DocumentState) -> Element {
    rsx! {
        span {
            key: "{state.class_name()}",
            class: "document-state {state.class_name()}",
            role: "status",
            span { class: "document-state-dot", "aria-hidden": "true" }
            "{state.label()}"
        }
    }
}

/// The document's name as the button that opens its switcher.
#[component]
fn DocumentTitle(
    id: String,
    name: String,
    hint: String,
    expanded: bool,
    on_toggle: EventHandler<()>,
) -> Element {
    rsx! {
        Button {
            id: "{id}",
            purpose: ActionPurpose::Select,
            variant: ButtonVariant::Quiet,
            class: "document-title",
            aria_haspopup: "listbox",
            aria_expanded: expanded,
            title: "{hint}",
            onclick: move |_| on_toggle.call(()),
            span { class: "document-name", "{name}" }
            ChevronIcon { open: false }
        }
    }
}

/// Rename in place. Enter applies, Escape cancels, focus returns to the title.
#[component]
fn DocumentRenameForm(
    title_id: String,
    label: String,
    initial: String,
    disabled: bool,
    on_apply: EventHandler<String>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut name = use_signal(|| initial);
    let input_id = format!("{title_id}-input");
    // Chromium honours `autofocus` only during the initial parse, so a form
    // that appears later asks for focus itself — and selects the name, which
    // is the whole reason the operator opened it.
    use_hook({
        let input_id = input_id.clone();
        move || BrowserFocus::select_after_render(input_id)
    });
    let restore_focus = move || BrowserFocus::after_render(title_id.clone());
    rsx! {
        form {
            class: "document-rename",
            onsubmit: {
                let restore_focus = restore_focus.clone();
                move |event: FormEvent| {
                    event.prevent_default();
                    let trimmed = name().trim().to_owned();
                    if !disabled && !trimmed.is_empty() {
                        on_apply.call(trimmed);
                        restore_focus();
                    }
                }
            },
            onkeydown: {
                let restore_focus = restore_focus.clone();
                move |event: KeyboardEvent| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        on_cancel.call(());
                        restore_focus();
                    }
                }
            },
            TextInput {
                id: "{input_id}",
                class: "document-name-input",
                aria_label: "{label}",
                autofocus: true,
                value: "{name}",
                disabled,
                oninput: move |value| name.set(value),
            }
            Button {
                purpose: ActionPurpose::Apply,
                variant: ButtonVariant::Outline,
                r#type: "submit",
                disabled: disabled || name().trim().is_empty(),
                "Apply"
            }
            Button {
                purpose: ActionPurpose::Cancel,
                variant: ButtonVariant::Link,
                onclick: {
                    let restore_focus = restore_focus.clone();
                    move |_| {
                        on_cancel.call(());
                        restore_focus();
                    }
                },
                "Cancel"
            }
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
        }
    }
    fn title_hint(self) -> String {
        format!("Switch, create, or rename {}s", self.noun)
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
    fn rename(self) -> String {
        format!("Rename this {}", self.noun)
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
    /// Discard is refused while a command — or an assistant turn — is running.
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
    /// The activity's own Run button, or nothing: Explore has no Run, because
    /// each measurement runs from its own workspace.
    run: Element,
}

/// The identity half and the action half of one document bar. The glyph and
/// the surrounding `header` stay with the activity, because Workflow's two
/// views share one header and one glyph between them.
#[component]
pub(crate) fn DocumentBar(props: DocumentBarProps) -> Element {
    let mut renaming = use_signal(|| false);
    // The switcher remembers which document it was opened over, so anything
    // that replaces the open document — picking a saved one, creating one from
    // a dataset — closes it without the creation controls knowing it is there.
    let mut opened_over = use_signal(|| Option::<String>::None);
    let copy = props.copy;
    let title_id = format!("{}-document-title", copy.noun);
    let document = props.document;
    let gate = props.gate;
    let on_command = props.on_command;
    let renaming_refused = gate.renaming.clone();
    let name = document.current.name.clone();
    let current_id = document.current.id.clone();
    let switching = opened_over.read().as_deref() == Some(current_id.as_str());
    rsx! {
        div { class: "document-identity",
            if renaming() {
                DocumentRenameForm {
                    title_id: title_id.clone(),
                    label: copy.name_label(),
                    initial: name.clone(),
                    disabled: renaming_refused.is_some(),
                    on_apply: move |name| { on_command.call(DocumentBarCommand::Rename(name)); renaming.set(false); },
                    on_cancel: move |_| renaming.set(false),
                }
            } else {
                DocumentTitle {
                    id: title_id.clone(),
                    name: name.clone(),
                    hint: gate.switching.clone().unwrap_or_else(|| copy.title_hint()),
                    expanded: switching,
                    on_toggle: move |_| {
                        let opened = (!switching).then(|| current_id.clone());
                        opened_over.set(opened);
                    },
                }
            }
            // The switcher hangs from the title it belongs to, wherever that
            // title sits in the bar.
            if switching {
                DocumentSwitcher {
                    id: format!("{}-switcher", copy.noun),
                    title_id,
                    copy,
                    gate: gate.clone(),
                    entries: document.saved.clone(),
                    current: document.current.clone(),
                    on_select: move |id| on_command.call(DocumentBarCommand::Open(id)),
                    on_rename: move |_| { opened_over.set(None); renaming.set(true); },
                    on_close: move |_| opened_over.set(None),
                    {props.creation}
                }
            }
        }
        div { class: "document-actions",
            DocumentStateChip { state: document.state }
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
            {props.run}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct DocumentSwitcherProps {
    id: String,
    title_id: String,
    entries: Vec<DocumentEntry>,
    current: DocumentEntry,
    copy: DocumentBarCopy,
    gate: DocumentBarGate,
    on_select: EventHandler<Option<String>>,
    on_rename: EventHandler<()>,
    on_close: EventHandler<()>,
    /// Extra creation controls, such as Workflow's "From dataset".
    #[props(default)]
    children: Element,
}

/// The popover behind the title: saved documents, New, Rename, and whatever
/// else the activity offers for creation. A transparent backdrop closes it.
#[component]
fn DocumentSwitcher(props: DocumentSwitcherProps) -> Element {
    let switcher_id = props.id.clone();
    let disabled = props.gate.switching.is_some();
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
    let title_id = props.title_id.clone();
    let close = move || {
        on_close.call(());
        BrowserFocus::after_render(title_id.clone());
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
                ul { class: "document-switcher-list", role: "listbox", "aria-label": "{props.copy.dialog_label()}",
                    for (index, item) in props.entries.iter().enumerate() {
                        li { key: "{item.id}", role: "none",
                            Button {
                                id: "{switcher_id}-option-{index}",
                                role: "option",
                                purpose: ActionPurpose::Select,
                                variant: ButtonVariant::Row,
                                class: "document-switcher-option",
                                aria_selected: item.id == props.current.id,
                                disabled,
                                onclick: {
                                    let id = item.id.clone();
                                    let close = close.clone();
                                    move |_| {
                                        props.on_select.call(Some(id.clone()));
                                        close();
                                    }
                                },
                                span { class: "document-switcher-name",
                                    if item.id == props.current.id { "{props.current.name}" } else { "{item.name}" }
                                }
                                if item.id == props.current.id { CheckIcon {} }
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
                            props.on_select.call(None);
                            close();
                        }
                    },
                    "{props.copy.create}"
                }
                {props.children}
                Button {
                    purpose: ActionPurpose::Apply,
                    variant: ButtonVariant::Link,
                    class: "document-switcher-rename",
                    disabled: props.gate.renaming.is_some(),
                    title: props.gate.renaming.clone().unwrap_or_else(|| props.copy.rename()),
                    onclick: move |_| props.on_rename.call(()),
                    "{props.copy.rename()}"
                }
            }
        }
    }
}

/// The exact SQL a run executed, folded behind one line of the run strip.
#[component]
pub(crate) fn ExecutedSql(sql: String) -> Element {
    rsx! {
        Disclosure { class: "executed-sql", summary: rsx! { SqlIcon {} "Executed SQL" },
            pre { code { "{sql}" } }
        }
    }
}
