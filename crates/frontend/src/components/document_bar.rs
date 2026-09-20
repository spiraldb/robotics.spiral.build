//! The document bar's shared parts: a title that opens the switcher, the
//! saved state beside it, the inline rename form, the switch/create/rename
//! popover, and the executed-SQL disclosure every run strip carries. Each
//! activity keeps its own action row, because what Save and Run may do
//! differs; the identity chrome does not.

use crate::activities::workflow::highlight::SqlHighlight;
use crate::components::icons::{CheckIcon, SqlIcon};
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonVariant, ChevronIcon, Disclosure, TextInput,
};

/// Whether what is on screen matches what Studio has stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DocumentState {
    Saved,
    Draft,
    Conflict,
}

impl DocumentState {
    pub(crate) const fn of(conflict: bool, unsaved: bool) -> Self {
        if conflict {
            Self::Conflict
        } else if unsaved {
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

/// Keyed on the state so a change replays its settle motion once.
#[component]
pub(crate) fn DocumentStateChip(state: DocumentState) -> Element {
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
pub(crate) fn DocumentTitle(
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
pub(crate) fn DocumentRenameForm(
    title_id: String,
    label: String,
    initial: String,
    disabled: bool,
    on_apply: EventHandler<String>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut name = use_signal(|| initial);
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

/// The words a switcher speaks for its kind of document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SwitcherCopy {
    dialog_label: &'static str,
    empty: &'static str,
    create: &'static str,
    rename: &'static str,
}

impl SwitcherCopy {
    pub(crate) const fn new(
        dialog_label: &'static str,
        empty: &'static str,
        create: &'static str,
        rename: &'static str,
    ) -> Self {
        Self {
            dialog_label,
            empty,
            create,
            rename,
        }
    }
}

/// Why the list is disabled, and whether renaming is possible right now.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SwitcherGate {
    blocked: Option<String>,
    rename_disabled: bool,
}

impl SwitcherGate {
    pub(crate) fn new(blocked: Option<String>, rename_disabled: bool) -> Self {
        Self {
            blocked,
            rename_disabled,
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub(crate) struct DocumentSwitcherProps {
    id: String,
    title_id: String,
    entries: Vec<DocumentEntry>,
    current: DocumentEntry,
    copy: SwitcherCopy,
    gate: SwitcherGate,
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
pub(crate) fn DocumentSwitcher(props: DocumentSwitcherProps) -> Element {
    let switcher_id = props.id.clone();
    let disabled = props.gate.blocked.is_some();
    let initial_focus = props
        .entries
        .iter()
        .position(|item| item.id == props.current.id)
        .map(|index| format!("{switcher_id}-option-{index}"))
        .unwrap_or_else(|| format!("{switcher_id}-new"));
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
            "aria-label": "{props.copy.dialog_label}",
            onkeydown: {
                let close = close.clone();
                move |event: KeyboardEvent| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        close();
                    }
                }
            },
            if let Some(reason) = props.gate.blocked.as_ref() {
                p { class: "document-switcher-note is-blocking", role: "status", "{reason}" }
            }
            if props.entries.is_empty() {
                p { class: "document-switcher-empty", "{props.copy.empty}" }
            } else {
                ul { class: "document-switcher-list", role: "listbox", "aria-label": "{props.copy.dialog_label}",
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
                    disabled: props.gate.rename_disabled,
                    onclick: move |_| props.on_rename.call(()),
                    "{props.copy.rename}"
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
            pre { SqlHighlight { sql } }
        }
    }
}
