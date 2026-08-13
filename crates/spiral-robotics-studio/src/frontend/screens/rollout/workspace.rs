//! Checkpoint selection and follower rollout controls.

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_contracts::api::{
    CameraStatus, MAX_ROLLOUT_DURATION_SECONDS, RobotSummary, RolloutPhase, RolloutSnapshot,
    StartRolloutRequest, TrainingRun, TrainingSnapshot,
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
    run_active: bool,
    step: u64,
    validation_loss: Option<f64>,
    action_steps: usize,
    sample_hz: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RolloutCameraSummary {
    ready: bool,
    detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingRolloutAction {
    Start,
    Stop,
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
    let mut pending_action = use_signal(|| None::<PendingRolloutAction>);
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
    let camera_summary = rollout_camera_summary(follower.as_ref(), &snapshot.cameras);
    let follower_ready = arm_ready && camera_summary.ready;
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
        format!("{connection} · {calibration} · {}", camera_summary.detail)
    });
    let precondition_blocker = rollout_precondition_blocker(
        selected,
        follower.as_ref(),
        camera_summary.ready,
        studio.controls_available(),
    );
    let active = rollout.phase.active();
    let stopping = rollout.phase == RolloutPhase::Stopping;
    let pending = *pending_action.read();
    let duration_seconds = controller.duration_seconds();
    let duration_draft = if active {
        rollout
            .duration_seconds
            .map(|seconds| seconds.to_string())
            .unwrap_or_else(|| controller.duration_draft())
    } else {
        controller.duration_draft()
    };
    let duration_valid = duration_seconds.is_some();
    let blocker = precondition_blocker.clone().or_else(|| {
        (!duration_valid).then_some(format!(
            "Set a whole-number duration from 1 to {MAX_ROLLOUT_DURATION_SECONDS} seconds."
        ))
    });
    let action_steps = selected.map_or(0, |choice| choice.action_steps);
    let control_rate = selected.map_or(0.0, |choice| choice.sample_hz);
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
                                if choice.run_active { " · training" }
                            }
                        }
                    }
                }
                if let Some(choice) = selected {
                    div { class: "rollout-checkpoint-meta",
                        span { "ACT · vision" }
                        span { "Validation {validation_loss}" }
                        span {
                            class: if choice.run_active { "rollout-state" } else { "rollout-state rollout-state-ready" },
                            if choice.run_active { "Training" } else { "Ready" }
                        }
                    }
                } else if let Some(run) = training.active_run() {
                    p { class: "rollout-empty-copy",
                        "Checkpoints appear every {format_count(run.config.checkpoint_every)} steps."
                    }
                } else {
                    p { class: "rollout-empty-copy", "Train a policy to create a checkpoint." }
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
                    div { class: "rollout-field",
                        span { "Action steps" }
                        strong { "{action_steps}" }
                    }
                    div { class: "rollout-field",
                        span { "Control rate" }
                        strong { "{control_rate:.1} Hz" }
                    }
                    label { class: "rollout-field rollout-duration-field",
                        span { "Duration" }
                        div { class: "rollout-duration-control",
                            input {
                                r#type: "number",
                                min: "1",
                                max: "{MAX_ROLLOUT_DURATION_SECONDS}",
                                step: "1",
                                value: "{duration_draft}",
                                disabled: active || pending.is_some(),
                                title: "Policy control time; loading and warm-up are excluded",
                                "aria-describedby": "rollout-duration-help",
                                "aria-invalid": if duration_valid { "false" } else { "true" },
                                oninput: move |event| controller.set_duration_draft(event.value()),
                            }
                            small {
                                id: "rollout-duration-help",
                                class: if duration_valid { "" } else { "rollout-duration-error" },
                                if duration_valid {
                                    "seconds · then hold"
                                } else {
                                    "Enter 1–{MAX_ROLLOUT_DURATION_SECONDS} s"
                                }
                            }
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
                div { class: "rollout-action-buttons",
                    button {
                        r#type: "button",
                        class: "rollout-run-button",
                        disabled: pending.is_some() || active || blocker.is_some(),
                        title: blocker
                            .clone()
                            .unwrap_or_else(|| "Run policy on the follower".to_owned()),
                        onclick: {
                            let selected = selected.cloned();
                            let follower = follower.clone();
                            move |_| {
                                if pending_action.peek().is_some() {
                                    return;
                                }
                                let (Some(checkpoint), Some(robot), Some(duration_seconds)) =
                                    (selected.clone(), follower.clone(), duration_seconds)
                                else {
                                    return;
                                };
                                pending_action.set(Some(PendingRolloutAction::Start));
                                let mut pending_action = pending_action;
                                let request = StartRolloutRequest {
                                    run_id: checkpoint.run_id,
                                    checkpoint_step: checkpoint.step,
                                    robot: robot.id,
                                    duration_seconds,
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
                                    pending_action.set(None);
                                });
                            }
                        },
                        if pending == Some(PendingRolloutAction::Start) {
                            "Starting…"
                        } else {
                            "Run policy"
                        }
                    }
                    button {
                        r#type: "button",
                        class: "rollout-stop-button",
                        disabled: pending.is_some() || !active || stopping,
                        title: "Stop policy control and hold the follower at its current position",
                        "aria-label": "Stop policy and hold the follower",
                        onclick: move |_| {
                            if pending_action.peek().is_some() || !active || stopping {
                                return;
                            }
                            pending_action.set(Some(PendingRolloutAction::Stop));
                            let mut pending_action = pending_action;
                            spawn(async move {
                                if let Err(error) = api::post("/api/rollout/stop").await {
                                    studio.report_from(MessageSource::Rollout, error);
                                }
                                pending_action.set(None);
                            });
                        },
                        if pending == Some(PendingRolloutAction::Stop) || stopping {
                            "Stopping…"
                        } else {
                            "Stop & hold"
                        }
                    }
                }
                if !active && let Some(reason) = blocker.as_ref() {
                    p { class: "rollout-blocker", "{reason}" }
                }
                if rollout.phase == RolloutPhase::Failed && let Some(reason) = rollout.message.as_ref() {
                    p { class: "rollout-blocker", "{reason}" }
                }
            }
        }
    }
}

fn rollout_camera_summary(
    follower: Option<&RobotSummary>,
    camera_statuses: &[CameraStatus],
) -> RolloutCameraSummary {
    let Some(follower) = follower else {
        return RolloutCameraSummary {
            ready: false,
            detail: "no cameras".to_owned(),
        };
    };
    let total = follower.cameras.len();
    if total == 0 {
        return RolloutCameraSummary {
            ready: false,
            detail: "no cameras".to_owned(),
        };
    }

    let available = follower
        .cameras
        .iter()
        .filter(|binding| {
            camera_statuses
                .iter()
                .any(|status| status.id == binding.id && status.error.is_none())
        })
        .count();
    let ready = available == total;
    let detail = match (total, available) {
        (1, 1) => format!("{} available", follower.cameras[0].name),
        (1, _) => format!("{} unavailable", follower.cameras[0].name),
        (_, available) if ready => format!("{available} cameras available"),
        _ => format!("{available} of {total} cameras available"),
    };
    RolloutCameraSummary { ready, detail }
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
                    p { "Starts when the policy sends its first action." }
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
        .filter(|checkpoint| checkpoint.inference_ready)
        .map(|checkpoint| CheckpointChoice {
            key: format!("{}::{}", run.id, checkpoint.step),
            run_id: run.id.clone(),
            run_name: run.config.run_name.clone(),
            run_active: run.phase.active(),
            step: checkpoint.step,
            validation_loss: checkpoint.validation_loss,
            action_steps: if checkpoint.recommended_action_steps == 0 {
                run.config.chunk_size
            } else {
                checkpoint
                    .recommended_action_steps
                    .min(run.config.chunk_size)
            },
            sample_hz: checkpoint.sample_hz,
        })
}

fn rollout_precondition_blocker(
    checkpoint: Option<&CheckpointChoice>,
    follower: Option<&RobotSummary>,
    cameras_ready: bool,
    controls_available: bool,
) -> Option<String> {
    let Some(checkpoint) = checkpoint else {
        return Some("Select a checkpoint.".to_owned());
    };
    if checkpoint.run_active {
        return Some("Stop or finish training before running this checkpoint.".to_owned());
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
    if !cameras_ready {
        return Some("Start every follower camera.".to_owned());
    }
    if !controls_available {
        return Some("Waiting for fresh telemetry.".to_owned());
    }
    None
}

fn runtime_title(rollout: &RolloutSnapshot) -> &'static str {
    match rollout.phase {
        RolloutPhase::Idle if rollout.finished_ms.is_some() => "Policy stopped",
        RolloutPhase::Idle => "Ready",
        RolloutPhase::Loading => "Loading checkpoint",
        RolloutPhase::WarmingUp => "Preparing policy",
        RolloutPhase::Running => "Policy active",
        RolloutPhase::Stopping => "Stopping policy",
        RolloutPhase::Failed => "Rollout stopped",
    }
}

fn runtime_detail(rollout: &RolloutSnapshot) -> String {
    rollout
        .message
        .clone()
        .unwrap_or_else(|| "Safety limits and hold-on-stop are active.".to_owned())
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

#[cfg(test)]
mod tests {
    use spiral_robotics_contracts::api::{
        CameraBinding, CameraStatus, ConnectionStatus, RobotSummary, TrainingCheckpoint,
        TrainingRun, TrainingRunPhase,
    };

    use super::{
        RolloutCameraSummary, checkpoints_for_run, follower, rollout_camera_summary,
        rollout_precondition_blocker,
    };

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

    #[test]
    fn rollout_requires_every_assigned_camera() {
        let mut follower = robot("follower", "so101_follower");
        follower.cameras = vec![
            CameraBinding {
                name: "gripper".to_owned(),
                id: "camera-1".to_owned(),
                mount: None,
            },
            CameraBinding {
                name: "top".to_owned(),
                id: "camera-2".to_owned(),
                mount: None,
            },
        ];
        let camera = |id: &str, error: Option<&str>| CameraStatus {
            id: id.to_owned(),
            name: id.to_owned(),
            width: 640,
            height: 360,
            fps: 30,
            streaming: true,
            frames: 1,
            error: error.map(str::to_owned),
        };

        assert_eq!(
            rollout_camera_summary(
                Some(&follower),
                &[camera("camera-1", None), camera("camera-2", None)],
            ),
            RolloutCameraSummary {
                ready: true,
                detail: "2 cameras available".to_owned(),
            }
        );
        assert_eq!(
            rollout_camera_summary(
                Some(&follower),
                &[
                    camera("camera-1", None),
                    camera("camera-2", Some("offline")),
                ],
            ),
            RolloutCameraSummary {
                ready: false,
                detail: "1 of 2 cameras available".to_owned(),
            }
        );
    }

    #[test]
    fn rollout_lists_inference_ready_checkpoints_from_an_active_run() {
        let run = TrainingRun {
            id: "active-run".to_owned(),
            phase: TrainingRunPhase::Running,
            checkpoints: vec![
                TrainingCheckpoint {
                    step: 20_000,
                    inference_ready: true,
                    ..TrainingCheckpoint::default()
                },
                TrainingCheckpoint {
                    step: 10_000,
                    inference_ready: false,
                    ..TrainingCheckpoint::default()
                },
            ],
            ..TrainingRun::default()
        };

        let choices = checkpoints_for_run(&run).collect::<Vec<_>>();

        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].step, 20_000);
        assert!(choices[0].run_active);
        assert_eq!(
            rollout_precondition_blocker(choices.first(), None, false, false).as_deref(),
            Some("Stop or finish training before running this checkpoint.")
        );
    }
}
