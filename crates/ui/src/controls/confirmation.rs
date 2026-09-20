//! Confirmation tiers. Tier one is an inline strip for a local permanent
//! action; tier two is a blocking alert dialog for scope-wide replacement and
//! physical consequences. Both focus the safe choice, cancel on Escape, and
//! return focus to the action that opened them.

use dioxus::prelude::*;

use crate::controls::{ActionPurpose, Button, ButtonVariant};
use crate::foundation::{BrowserFocus, ClassNames};

#[derive(Props, Clone, PartialEq)]
pub struct InlineConfirmationProps {
    id: String,
    copy: ConfirmationCopy,
    response: ConfirmationResponse,
    dismissal: ConfirmationDismissal,
    #[props(default)]
    class: Option<String>,
}

#[derive(Props, Clone, PartialEq)]
struct ConfirmationActionsProps {
    cancel_id: String,
    copy: ConfirmationCopy,
    response: ConfirmationResponse,
    dismissal: ConfirmationDismissal,
}

#[derive(Clone, PartialEq)]
pub struct ConfirmationCopy {
    message: String,
    confirm_label: String,
    cancel_label: String,
}

impl ConfirmationCopy {
    pub fn new(
        message: impl Into<String>,
        confirm_label: impl Into<String>,
        cancel_label: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            confirm_label: confirm_label.into(),
            cancel_label: cancel_label.into(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct ConfirmationResponse {
    purpose: ActionPurpose,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
}

impl ConfirmationResponse {
    pub fn new(
        purpose: ActionPurpose,
        on_confirm: impl FnMut(()) + 'static,
        on_cancel: impl FnMut(()) + 'static,
    ) -> Self {
        Self {
            purpose,
            on_confirm: EventHandler::new(on_confirm),
            on_cancel: EventHandler::new(on_cancel),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct ConfirmationDismissal {
    return_focus_id: String,
    pending: bool,
}

impl ConfirmationDismissal {
    pub fn new(return_focus_id: impl Into<String>) -> Self {
        Self {
            return_focus_id: return_focus_id.into(),
            pending: false,
        }
    }

    pub const fn with_pending(mut self, pending: bool) -> Self {
        self.pending = pending;
        self
    }
}

#[component]
fn ConfirmationActions(props: ConfirmationActionsProps) -> Element {
    rsx! {
        div { class: "ui-confirmation-actions",
            Button {
                id: props.cancel_id,
                purpose: ActionPurpose::Cancel,
                variant: ButtonVariant::Outline,
                disabled: props.dismissal.pending,
                onclick: move |_| BrowserFocus::dismiss_confirmation(
                    props.response.on_cancel,
                    &props.dismissal.return_focus_id,
                ),
                "{props.copy.cancel_label}"
            }
            Button {
                purpose: props.response.purpose,
                variant: ButtonVariant::Solid,
                disabled: props.dismissal.pending,
                aria_busy: props.dismissal.pending,
                onclick: move |_| props.response.on_confirm.call(()),
                "{props.copy.confirm_label}"
            }
        }
    }
}

/// Tier-one confirmation for a local destructive action. The safe choice takes
/// focus, Escape cancels, and cancellation returns to the originating action.
#[component]
pub fn InlineConfirmation(props: InlineConfirmationProps) -> Element {
    let cancel_id = format!("{}-cancel", props.id);
    let class = "ui-inline-confirmation".with_class(props.class.as_deref());
    let initial_focus_id = cancel_id.clone();
    use_effect(move || BrowserFocus::after_render(initial_focus_id.clone()));

    let cancel_from_key = props.response.on_cancel;
    let return_from_key = props.dismissal.return_focus_id.clone();
    let pending = props.dismissal.pending;
    rsx! {
        div {
            id: props.id,
            class,
            role: "group",
            "aria-label": "Confirmation",
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape && !pending {
                    event.prevent_default();
                    BrowserFocus::dismiss_confirmation(cancel_from_key, &return_from_key);
                }
            },
            p { "{props.copy.message}" }
            ConfirmationActions {
                cancel_id,
                copy: props.copy,
                response: props.response,
                dismissal: props.dismissal,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct BlockingConfirmationProps {
    id: String,
    title: String,
    consequence: String,
    copy: ConfirmationCopy,
    response: ConfirmationResponse,
    dismissal: ConfirmationDismissal,
}

/// Tier-two confirmation for scope-wide or physical consequences.
#[component]
pub fn BlockingConfirmation(props: BlockingConfirmationProps) -> Element {
    let dialog_id = props.id.clone();
    let title_id = format!("{}-title", props.id);
    let description_id = format!("{}-description", props.id);
    let cancel_id = format!("{}-cancel", props.id);
    let initial_focus_id = cancel_id.clone();
    use_effect(move || BrowserFocus::after_render(initial_focus_id.clone()));

    let cancel_from_key = props.response.on_cancel;
    let cancel_from_backdrop = props.response.on_cancel;
    let return_from_key = props.dismissal.return_focus_id.clone();
    let return_from_backdrop = props.dismissal.return_focus_id.clone();
    let pending = props.dismissal.pending;
    rsx! {
        div {
            class: "ui-dialog-backdrop",
            onclick: move |_| {
                if !pending {
                    BrowserFocus::dismiss_confirmation(cancel_from_backdrop, &return_from_backdrop);
                }
            },
        }
        section {
            id: props.id,
            class: "ui-dialog-surface ui-blocking-confirmation",
            role: "alertdialog",
            "aria-modal": "true",
            "aria-labelledby": title_id.clone(),
            "aria-describedby": description_id.clone(),
            tabindex: "-1",
            onclick: move |event| event.stop_propagation(),
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape && !pending {
                    event.prevent_default();
                    BrowserFocus::dismiss_confirmation(cancel_from_key, &return_from_key);
                } else if event.key() == Key::Tab
                    && BrowserFocus::trap_within(&dialog_id, event.modifiers().shift())
                {
                    event.prevent_default();
                }
            },
            header {
                h2 { id: title_id, "{props.title}" }
            }
            div { id: description_id, class: "ui-confirmation-copy",
                p { "{props.copy.message}" }
                p { class: "ui-confirmation-consequence", "{props.consequence}" }
            }
            footer {
                ConfirmationActions {
                    cancel_id,
                    copy: props.copy,
                    response: props.response,
                    dismissal: props.dismissal,
                }
            }
        }
    }
}
