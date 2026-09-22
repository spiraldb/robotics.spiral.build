//! The guard on an Explore action that spends money or replaces measured
//! work.
//!
//! Studio confirmed removing an empty node and confirmed nothing about
//! re-annotating forty episodes or throwing away six categories and their two
//! hundred and seventy assignments. That is the risk exactly inverted: the
//! reversible edit asked, and the paid, irreversible one did not.
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, InlineConfirmation,
};

/// A quiet control that asks before it runs, naming what it would replace.
///
/// It stays quieter than the action beside it — Run, or opening a category —
/// because that one is what the operator came for and this one undoes work
/// they have already paid for. A link where something louder already holds the
/// region, an outline where the control would otherwise read as a warning.
#[component]
pub(super) fn ConfirmedRerun(
    /// Both the control's words and the confirming button's.
    label: String,
    /// What it would replace, in full, before it does it.
    question: String,
    id: String,
    #[props(default = ButtonVariant::Link)] variant: ButtonVariant,
    #[props(default)] disabled: bool,
    on_confirm: EventHandler<()>,
) -> Element {
    let mut asking = use_signal(|| false);
    rsx! {
        span { class: "explore-rerun",
            Button {
                id: "{id}",
                purpose: ActionPurpose::Replace,
                variant,
                disabled,
                aria_expanded: asking(),
                onclick: move |_| asking.toggle(),
                "{label}"
            }
            if asking() {
                InlineConfirmation {
                    id: "{id}-confirmation",
                    copy: ConfirmationCopy::new(question.clone(), label.clone(), "Keep"),
                    response: ConfirmationResponse::new(
                        ActionPurpose::Replace,
                        move |_| {
                            asking.set(false);
                            on_confirm.call(());
                        },
                        move |_| asking.set(false),
                    ),
                    dismissal: ConfirmationDismissal::new(id.clone()),
                }
            }
        }
    }
}
