//! The Policy driver's rows under the Control modes: which checkpoint runs
//! for how long, and what the run is doing.

use dioxus::prelude::*;
use spiral_robotics_common::rollout::{
    MAX_ROLLOUT_DURATION_SECONDS, RolloutPhase, RolloutSnapshot,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Disclosure, Fact, FactLayout, FactList, Field,
    FieldLayout, Select, TextInput, TextInputKind,
};

use super::PolicyControl;
use crate::format::{GroupedCountPresentation, LossPresentation};

/// Shown once there is a checkpoint to run or a run to report; a bench
/// without either has nothing for these rows to say.
#[component]
pub(in crate::activities::rollout) fn PolicySection(
    policy: PolicyControl,
    blocker: Option<String>,
) -> Element {
    let checkpoints = policy.checkpoints();
    let run = policy.run().read().to_owned();
    if checkpoints.is_empty() && run.session_id().is_none() {
        return rsx! {};
    }
    let active = run.phase().active();
    let selected = policy.selected();
    let duration_draft = match run.duration_seconds() {
        Some(seconds) if active => seconds.to_string(),
        _ => policy.duration_draft(),
    };
    let duration_valid = active || policy.duration_seconds().is_some();
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
                    disabled: checkpoints.is_empty() || active,
                    onchange: move |value: String| {
                        policy.select_checkpoint((!value.is_empty()).then_some(value));
                    },
                    if checkpoints.is_empty() {
                        option { value: "", "No checkpoints" }
                    }
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
                    max: "{MAX_ROLLOUT_DURATION_SECONDS}",
                    step: "1",
                    value: "{duration_draft}",
                    disabled: active,
                    title: "Policy control time; loading and warm-up are excluded",
                    aria_invalid: if duration_valid { "false" } else { "true" },
                    oninput: move |value| policy.set_duration_draft(value),
                }
                small { class: if duration_valid { "" } else { "policy-invalid" },
                    if duration_valid { "s · then hold" } else { "Enter 1–{MAX_ROLLOUT_DURATION_SECONDS} s" }
                }
            }
            PolicyStatus { policy, run: run.clone() }
            if !active && policy.loaded() && let Some(reason) = blocker {
                p { class: "policy-blocker", "{reason}" }
            }
        }
    }
}

/// One line for the run: its phase and what it last said, with every
/// measurement of the policy loop behind it once there are any.
#[component]
fn PolicyStatus(policy: PolicyControl, run: RolloutSnapshot) -> Element {
    if let Some(error) = policy.error() {
        return rsx! {
            div { class: "policy-status policy-status-failed", role: "alert",
                span { class: "policy-status-dot", "aria-hidden": "true" }
                strong { "Policy status unavailable" }
                small { "{error}" }
                Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Link,
                    onclick: move |_| policy.retry(),
                    "Retry"
                }
            }
        };
    }
    // An idle bench that has not run is the resting state and names nothing.
    let Some(title) = run.status_title() else {
        return rsx! {};
    };
    let line = rsx! {
        span { class: "policy-status-dot", "aria-hidden": "true" }
        strong { "{title}" }
        small { "{run.status_detail()}" }
    };
    let metrics = run.metrics();
    let measured = metrics.inference_count() > 0;
    rsx! {
        div { class: "policy-status {run.status_tone()}", role: "status",
            if measured {
                Disclosure { class: "policy-metrics", summary: line,
                    PolicyMetrics { run: run.clone() }
                }
            } else {
                div { class: "policy-status-line", {line} }
            }
            if let Some(warning) = run.warning() {
                small { class: "policy-status-warning", "{warning}" }
            }
        }
    }
}

/// Every measured value of the policy loop.
#[component]
fn PolicyMetrics(run: RolloutSnapshot) -> Element {
    let metrics = run.metrics().to_owned();
    let missed = metrics.missed_deadlines();
    rsx! {
        FactList { layout: FactLayout::Strip, mono: true, class: "policy-metric-strip",
            LatencyMetric { label: "Latency", value: run.latency_label(RolloutLatency::Last) }
            LatencyMetric { label: "Mean", value: run.latency_label(RolloutLatency::Mean) }
            LatencyMetric { label: "Max", value: run.latency_label(RolloutLatency::Peak) }
            LatencyMetric { label: "Model", value: run.latency_label(RolloutLatency::Model) }
            Fact { label: "Misses", class: if missed > 0 { "policy-metric-missed" } else { "" },
                span { {missed.grouped_label()} }
            }
            Fact { label: "Chunks", span { {metrics.inference_count().grouped_label()} } }
            Fact { label: "Commands", span { {metrics.command_count().grouped_label()} } }
        }
    }
}

#[component]
fn LatencyMetric(label: &'static str, value: String) -> Element {
    rsx! {
        Fact { label,
            span { "{value}" }
            small { "ms" }
        }
    }
}

/// A run as the status line reads it aloud. `RolloutSnapshot` lives in
/// `crates/common`, so this is an extension trait rather than an inherent impl.
trait RunPresentation {
    /// The phase worth naming, if any: an idle bench that has not run names
    /// nothing.
    fn status_title(&self) -> Option<&'static str>;
    fn status_detail(&self) -> String;
    fn status_tone(&self) -> &'static str;
    /// A latency in milliseconds to one decimal, or an em dash when the policy
    /// has not reported one. The unit is printed beside the figure, not in it.
    fn latency_label(&self, measure: RolloutLatency) -> String;
}

/// Which of the four latencies the metric strip is asking for.
#[derive(Clone, Copy)]
enum RolloutLatency {
    Last,
    Model,
    Mean,
    Peak,
}

impl RunPresentation for RolloutSnapshot {
    fn status_title(&self) -> Option<&'static str> {
        match self.phase() {
            RolloutPhase::Idle if self.finished_ms().is_some() => Some("Policy stopped"),
            RolloutPhase::Idle => None,
            RolloutPhase::Loading => Some("Loading checkpoint"),
            RolloutPhase::WarmingUp => Some("Preparing policy"),
            RolloutPhase::Running => Some("Policy active"),
            RolloutPhase::Stopping => Some("Stopping policy"),
            RolloutPhase::Failed => Some("Policy failed"),
        }
    }

    fn status_detail(&self) -> String {
        self.message()
            .map(str::to_owned)
            .unwrap_or_else(|| "Safety limits and hold-on-stop are active.".to_owned())
    }

    fn status_tone(&self) -> &'static str {
        match self.phase() {
            RolloutPhase::Running if self.warning().is_some() => "policy-status-warning-tone",
            RolloutPhase::Running => "policy-status-active",
            RolloutPhase::Failed => "policy-status-failed",
            _ => "",
        }
    }

    fn latency_label(&self, measure: RolloutLatency) -> String {
        let metrics = self.metrics();
        match measure {
            RolloutLatency::Last => metrics.last_latency_ms(),
            RolloutLatency::Model => metrics.last_model_ms(),
            RolloutLatency::Mean => metrics.mean_latency_ms(),
            RolloutLatency::Peak => metrics.max_latency_ms(),
        }
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "—".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_common::rollout::{RolloutPhase, RolloutSnapshot};

    use super::RunPresentation;

    /// Idle is the resting state: the status line names no phase, so it can
    /// never claim a blocked bench is ready.
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn an_idle_bench_names_no_phase() {
        assert_eq!(RolloutSnapshot::default().status_title(), None);
        assert_eq!(
            RolloutSnapshot::default()
                .with_lifecycle(RolloutPhase::Running, None)
                .status_title(),
            Some("Policy active")
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_clamped_action_tones_the_running_line_as_a_caution() {
        let run = RolloutSnapshot::default()
            .with_lifecycle(RolloutPhase::Running, None)
            .with_warning(Some("Policy output was clamped.".to_owned()));
        assert_eq!(run.status_tone(), "policy-status-warning-tone");
    }
}
