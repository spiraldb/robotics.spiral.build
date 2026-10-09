//! The Policy control source's rows under the Control modes: which checkpoint runs
//! for how long.

use dioxus::prelude::*;
use spiral_robotics_ui::{Field, FieldLayout, Select, TextInput, TextInputKind};

use super::PolicyControl;
use crate::format::LossPresentation;

/// Shown once there is a checkpoint to run; a bench without one has nothing
/// for these rows to say.
#[component]
pub(in crate::activities::rollout) fn PolicySection(
    policy: PolicyControl,
    blocker: Option<String>,
) -> Element {
    let checkpoints = policy.checkpoints();
    if checkpoints.is_empty() {
        return rsx! {};
    }
    let selected = policy.selected();
    let duration_draft = policy.duration_draft();
    let duration_valid = policy.duration_seconds().is_some();
    let max_seconds = PolicyControl::MAX_DURATION_SECONDS;
    let receipt = selected.as_ref().map(|checkpoint| {
        let loss = checkpoint
            .validation_loss()
            .map(LossPresentation::loss_label)
            .unwrap_or_else(|| "—".to_owned());
        let rate = checkpoint
            .sample_hz()
            .map(|hz| format!(" · {hz:.1} Hz"))
            .unwrap_or_default();
        format!("val {loss} · {} steps{rate}", checkpoint.action_steps())
    });

    rsx! {
        div { class: "policy-rows",
            Field { label: "Checkpoint", layout: FieldLayout::Inline, class: "policy-field",
                Select {
                    value: selected.as_ref().map(|choice| choice.key().to_owned()).unwrap_or_default(),
                    onchange: move |value: String| {
                        policy.select_checkpoint((!value.is_empty()).then_some(value));
                    },
                    for choice in &checkpoints {
                        option { value: choice.key().to_owned(), "{choice.label()}" }
                    }
                }
            }
            if let Some(receipt) = receipt {
                p { class: "policy-receipt", "{receipt}" }
            }
            Field { label: "Duration", layout: FieldLayout::Inline, class: "policy-field policy-duration",
                TextInput {
                    kind: TextInputKind::Number,
                    min: "1",
                    max: "{max_seconds}",
                    step: "1",
                    value: "{duration_draft}",
                    title: "Policy control time; loading and warm-up are excluded",
                    aria_invalid: if duration_valid { "false" } else { "true" },
                    oninput: move |value| policy.set_duration_draft(value),
                }
                small { class: if duration_valid { "" } else { "policy-invalid" },
                    if duration_valid { "s · then hold" } else { "Enter 1–{max_seconds} s" }
                }
            }
            if let Some(reason) = blocker {
                p { class: "policy-blocker", "{reason}" }
            }
        }
    }
}
