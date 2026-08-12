//! Compact, follower-only Rollout instruments.
//!
//! The backend owns checkpoint loading, native inference, safety, and motion.
//! This surface selects an artifact and renders that authoritative lifecycle.

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_core::api::{
    RobotSummary, RolloutPhase, RolloutSnapshot, StartRolloutRequest, TrainingRun, TrainingSnapshot,
};

use crate::frontend::api::http as api;
use crate::frontend::app::notifications::MessageSource;
use crate::frontend::app::state::studio;
use crate::frontend::screens::rollout::state::rollout_controller;

#[derive(Clone, Debug, PartialEq)]
struct CheckpointChoice {
    key: String,
    run_id: String,
    run_name: String,
    step: u64,
    validation_loss: Option<f64>,
    byte_size: u64,
    chunk_size: usize,
}

/// Rollout never asks the operator to choose a target. The configured follower
/// role is the only arm this activity is allowed to command.
pub(crate) fn follower(robots: &[RobotSummary]) -> Option<RobotSummary> {
    robots
        .iter()
        .find(|robot| {
            robot
                .robot_type
                .as_deref()
                .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
        })
        .cloned()
}

#[component]
pub(crate) fn RolloutWorkbench() -> Element {
    let studio = studio();
    let controller = rollout_controller();
    let mut requesting = use_signal(|| false);
    let snapshot = studio.snapshot.read().clone();
    let training = studio.training.read().clone();
    let rollout = studio.rollout.read().clone();
    let checkpoints = checkpoint_choices(&training);
    let selected_key = controller
        .checkpoint()
        .filter(|key| checkpoints.iter().any(|choice| &choice.key == key))
        .or_else(|| checkpoints.first().map(|choice| choice.key.clone()));
    let selected = selected_key
        .as_ref()
        .and_then(|key| checkpoints.iter().find(|choice| &choice.key == key));
    let follower = follower(&snapshot.robots);
    let arm_ready = follower.as_ref().is_some_and(|robot| {
        robot.connection.connected && robot.calibrated && robot.problem.is_none()
    });
    let camera = follower
        .as_ref()
        .and_then(|robot| robot.cameras.first())
        .cloned();
    let camera_ready = camera.as_ref().is_some_and(|binding| {
        snapshot
            .cameras
            .iter()
            .any(|status| status.id == binding.id && status.error.is_none())
    });
    let follower_ready = arm_ready && camera_ready;
    let follower_detail = follower.as_ref().map(|robot| {
        let connection = if robot.connection.connected {
            "connected"
        } else {
            "disconnected"
        };
        let calibration = if robot.calibrated {
            "calibrated"
        } else {
            "needs calibration"
        };
        let camera = match camera.as_ref() {
            Some(binding) if camera_ready => format!("{} available", binding.name),
            Some(binding) => format!("{} unavailable", binding.name),
            None => "no camera".to_owned(),
        };
        format!("Fixed target · {connection} · {calibration} · {camera}")
    });
    let precondition_blocker = rollout_precondition_blocker(
        selected.is_some(),
        follower.as_ref(),
        camera_ready,
        studio.controls_available(),
    );
    let active = rollout.phase.active();
    let stopping = rollout.phase == RolloutPhase::Stopping;
    let blocker = precondition_blocker.clone();
    let action_steps = selected
        .map(|choice| controller.action_steps().min(choice.chunk_size.max(1)))
        .unwrap_or_else(|| controller.action_steps());
    let control_rate = controller.control_rate_hz();
    let validation_loss = selected
        .and_then(|choice| choice.validation_loss)
        .map(format_loss)
        .unwrap_or_else(|| "—".to_owned());

    rsx! {
        div { class: "rollout-workbench studio-workbench-scroll",
            section { class: "rollout-section",
                label { class: "rollout-field",
                    span { "Checkpoint" }
                    select {
                        value: selected_key.clone().unwrap_or_default(),
                        disabled: checkpoints.is_empty() || active,
                        onchange: move |event| {
                            let value = event.value();
                            controller.select_checkpoint((!value.is_empty()).then_some(value));
                        },
                        if checkpoints.is_empty() {
                            option { value: "", "No checkpoints" }
                        }
                        for choice in &checkpoints {
                            option {
                                value: choice.key.clone(),
                                "{choice.run_name} · step {format_count(choice.step)}"
                            }
                        }
                    }
                }
                if let Some(choice) = selected {
                    div { class: "rollout-checkpoint-meta",
                        span { "ACT · vision" }
                        span { "SafeTensors" }
                        span { "Validation {validation_loss}" }
                        span { "{format_bytes(choice.byte_size)}" }
                    }
                } else {
                    p { class: "rollout-empty-copy", "Finish a training run to create one." }
                }
            }

            section { class: "rollout-section",
                header { class: "rollout-section-heading",
                    h3 { "Follower" }
                    span { class: if follower_ready { "rollout-state rollout-state-ready" } else { "rollout-state" },
                        if follower_ready { "Ready" } else { "Unavailable" }
                    }
                }
                if let Some(robot) = follower.as_ref() {
                    div { class: "rollout-target-row",
                        span {
                            class: "live-robot-accent",
                            style: "background: {crate::frontend::panels::scene::accent_css(studio.position_of(&robot.id))}",
                        }
                        div {
                            strong { "{robot.id}" }
                            small { "{follower_detail.clone().unwrap_or_default()}" }
                        }
                    }
                } else {
                    p { class: "rollout-empty-copy", "Configure a follower in Settings." }
                }
            }

            section { class: "rollout-section",
                header { class: "rollout-section-heading", h3 { "Execution" } }
                div { class: "rollout-field-grid",
                    label { class: "rollout-field",
                        span { "Action steps" }
                        input {
                            r#type: "number",
                            min: "1",
                            max: "{selected.map_or(256, |choice| choice.chunk_size)}",
                            value: action_steps,
                            disabled: active,
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.set_action_steps(value);
                            },
                        }
                    }
                    label { class: "rollout-field",
                        span { "Control rate" }
                        select {
                            value: control_rate,
                            disabled: active,
                            onchange: move |event| if let Ok(value) = event.value().parse() {
                                controller.set_control_rate(value);
                            },
                            option { value: "10", "10 Hz" }
                            option { value: "20", "20 Hz" }
                            option { value: "30", "30 Hz" }
                            option { value: "50", "50 Hz" }
                        }
                    }
                }
            }

            div { class: "rollout-action-area",
                div {
                    class: runtime_note_class(&rollout),
                    role: "status",
                    span { class: "rollout-runtime-dot", "aria-hidden": "true" }
                    div {
                        strong { "{runtime_title(&rollout)}" }
                        small { "{runtime_detail(&rollout)}" }
                    }
                }
                button {
                    r#type: "button",
                    class: "rollout-run-button",
                    disabled: *requesting.read() || stopping || (!active && blocker.is_some()),
                    title: if active {
                        "Stop policy and return the follower to idle".to_owned()
                    } else {
                        blocker.clone().unwrap_or_else(|| "Run policy on the follower".to_owned())
                    },
                    onclick: {
                        let selected = selected.cloned();
                        let follower = follower.clone();
                        move |_| {
                            if *requesting.peek() {
                                return;
                            }
                            requesting.set(true);
                            let mut requesting = requesting;
                            if active {
                                spawn(async move {
                                    if let Err(error) = api::post("/api/rollout/stop").await {
                                        studio.report_from(MessageSource::Rollout, error);
                                    }
                                    requesting.set(false);
                                });
                            } else if let (Some(checkpoint), Some(robot)) =
                                (selected.clone(), follower.clone())
                            {
                                let request = StartRolloutRequest {
                                    run_id: checkpoint.run_id,
                                    checkpoint_step: checkpoint.step,
                                    robot: robot.id,
                                    action_steps,
                                    control_rate_hz: control_rate,
                                };
                                spawn(async move {
                                    let result = api::send_json::<_, RolloutSnapshot>(
                                        Method::POST,
                                        "/api/rollout",
                                        Some(&request),
                                    )
                                    .await;
                                    if let Err(error) = result {
                                        studio.report_from(MessageSource::Rollout, error);
                                    }
                                    requesting.set(false);
                                });
                            } else {
                                requesting.set(false);
                            }
                        }
                    },
                    if stopping || *requesting.read() {
                        "Please wait…"
                    } else if active {
                        "Stop policy"
                    } else {
                        "Run policy"
                    }
                }
                if !active && let Some(reason) = precondition_blocker.as_ref() {
                    p { class: "rollout-blocker", "{reason}" }
                }
                if rollout.phase == RolloutPhase::Failed && let Some(reason) = rollout.message.as_ref() {
                    p { class: "rollout-blocker", "{reason}" }
                }
            }
        }
    }
}

#[component]
pub(crate) fn InferencePanel() -> Element {
    let studio = studio();
    let rollout = studio.rollout.read().clone();
    let metrics = &rollout.metrics;
    let latency = format_metric(metrics.last_latency_ms);
    let model = format_metric(metrics.last_model_ms);
    let misses = metrics.missed_deadlines.to_string();
    rsx! {
        section { class: "rollout-inference",
            div { class: "rollout-metric-strip",
                InferenceMetric { label: "Latency", value: latency, unit: "ms" }
                InferenceMetric { label: "Model", value: model, unit: "ms" }
                InferenceMetric { label: "Misses", value: misses, unit: "" }
            }
            if metrics.inference_count == 0 {
                div { class: "rollout-inference-empty",
                    strong { "No measurements yet" }
                    p { "Stats appear after native ACT completes its first chunk." }
                }
            } else {
                div { class: "rollout-inference-empty",
                    strong { "{format_count(metrics.command_count)} commands" }
                    p {
                        "{format_count(metrics.inference_count)} chunks · mean {format_metric(metrics.mean_latency_ms)} ms · max {format_metric(metrics.max_latency_ms)} ms"
                    }
                }
            }
        }
    }
}

#[component]
fn InferenceMetric(label: &'static str, value: String, unit: &'static str) -> Element {
    rsx! {
        dl { class: "rollout-metric",
            dt { "{label}" }
            dd { span { "{value}" } if !unit.is_empty() { small { "{unit}" } } }
        }
    }
}

fn checkpoint_choices(snapshot: &TrainingSnapshot) -> Vec<CheckpointChoice> {
    let mut choices = snapshot
        .runs
        .iter()
        .flat_map(checkpoints_for_run)
        .collect::<Vec<_>>();
    choices.sort_by_key(|choice| std::cmp::Reverse(choice.step));
    choices
}

fn checkpoints_for_run(run: &TrainingRun) -> impl Iterator<Item = CheckpointChoice> + '_ {
    run.checkpoints
        .iter()
        .filter(|checkpoint| !run.phase.active() && checkpoint.inference_ready)
        .map(|checkpoint| CheckpointChoice {
            key: format!("{}::{}", run.id, checkpoint.step),
            run_id: run.id.clone(),
            run_name: run.config.run_name.clone(),
            step: checkpoint.step,
            validation_loss: checkpoint.validation_loss,
            byte_size: checkpoint.byte_size,
            chunk_size: run.config.chunk_size,
        })
}

fn rollout_precondition_blocker(
    has_checkpoint: bool,
    follower: Option<&RobotSummary>,
    camera_ready: bool,
    controls_available: bool,
) -> Option<String> {
    if !has_checkpoint {
        return Some("Select a checkpoint.".to_owned());
    }
    let Some(follower) = follower else {
        return Some("Configure a follower in Settings.".to_owned());
    };
    if follower.problem.is_some() {
        return Some("Fix the follower in Settings.".to_owned());
    }
    if !follower.connection.connected {
        return Some("Connect the follower.".to_owned());
    }
    if !follower.calibrated {
        return Some("Calibrate the follower.".to_owned());
    }
    if !camera_ready {
        return Some("Start the follower camera.".to_owned());
    }
    if !controls_available {
        return Some("Waiting for fresh telemetry.".to_owned());
    }
    None
}

fn runtime_title(rollout: &RolloutSnapshot) -> &'static str {
    match rollout.phase {
        RolloutPhase::Idle => "Inference runtime available",
        RolloutPhase::Loading => "Loading checkpoint",
        RolloutPhase::WarmingUp => "Warming native ACT",
        RolloutPhase::Running => "Policy active",
        RolloutPhase::Stopping => "Stopping policy",
        RolloutPhase::Failed => "Rollout stopped",
    }
}

fn runtime_detail(rollout: &RolloutSnapshot) -> String {
    rollout.message.clone().unwrap_or_else(|| {
        "SafeTensors · native Burn backend · safety-supervised control".to_owned()
    })
}

fn runtime_note_class(rollout: &RolloutSnapshot) -> &'static str {
    match rollout.phase {
        RolloutPhase::Running => "rollout-runtime-note rollout-runtime-note-active",
        RolloutPhase::Failed => "rollout-runtime-note rollout-runtime-note-failed",
        _ => "rollout-runtime-note",
    }
}

fn format_metric(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "—".to_owned())
}

fn format_count(value: u64) -> String {
    let text = value.to_string();
    text.as_bytes()
        .rchunks(3)
        .rev()
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(",")
}

fn format_loss(value: f64) -> String {
    if value >= 0.01 {
        format!("{value:.4}")
    } else {
        format!("{value:.3e}")
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KiB", bytes as f64 / 1_024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_core::api::{ConnectionStatus, RobotSummary};

    use super::follower;

    fn robot(id: &str, role: &str) -> RobotSummary {
        RobotSummary {
            id: id.to_owned(),
            file: format!("{id}.json"),
            robot_type: Some(role.to_owned()),
            port: None,
            motors: Vec::new(),
            cameras: Vec::new(),
            calibrated: true,
            problem: None,
            connection: ConnectionStatus {
                connected: true,
                error: None,
            },
        }
    }

    #[test]
    fn rollout_target_is_always_the_configured_follower() {
        let robots = [
            robot("leader", "so101_leader"),
            robot("follower", "so101_follower"),
        ];
        assert_eq!(
            follower(&robots).map(|robot| robot.id),
            Some("follower".to_owned())
        );
    }
}
