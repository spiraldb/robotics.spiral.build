//! Checkpoint selection and follower rollout controls.

use crate::activities::rollout::RolloutClient;
use dioxus::prelude::*;
use spiral_robotics_common::robots::{CameraStatus, RobotSummary};
use spiral_robotics_common::rollout::{
    MAX_ROLLOUT_DURATION_SECONDS, RolloutConfig, RolloutPhase, RolloutSnapshot,
};
use spiral_robotics_common::training::{TrainingCheckpoint, TrainingRun};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, EmptyState, Fact, FactLayout, FactList,
    Field, FieldLayout, Select, TextInput, TextInputKind,
};

use crate::format::{GroupedCountPresentation, LossPresentation};
use crate::robots::HardwareViewState;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;

const MAX_ROLLOUT_ACTION_STEPS: usize = 20;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CheckpointChoice {
    identity: CheckpointIdentity,
    step: u64,
    validation_loss: Option<f64>,
    policy: CheckpointPolicy,
}

#[derive(Clone, Debug, PartialEq)]
struct CheckpointIdentity {
    key: String,
    run_id: String,
    run_name: String,
    run_active: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct CheckpointPolicy {
    action_steps: usize,
    sample_hz: Option<f64>,
    camera_roles: Vec<String>,
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
pub(super) trait RobotFleet {
    fn follower(&self) -> Option<RobotSummary>;
}

impl RobotFleet for [RobotSummary] {
    fn follower(&self) -> Option<RobotSummary> {
        self.iter()
            .find(|robot| {
                robot
                    .config()
                    .robot_type()
                    .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
            })
            .cloned()
    }
}

#[component]
pub(super) fn RolloutWorkbench(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    controller: super::state::RolloutController,
    training_history: crate::activities::training::history::TrainingHistory,
) -> Element {
    let mut pending_action = use_signal(|| None::<PendingRolloutAction>);
    let snapshot = telemetry.snapshot().read().to_owned();
    let training = training_history.snapshot().read().to_owned();
    let rollout = controller.snapshot().read().to_owned();
    let checkpoints = training_history.checkpoint_choices();
    let selected_key = controller
        .checkpoint()
        .filter(|key| checkpoints.iter().any(|choice| &choice.identity.key == key))
        .or_else(|| {
            checkpoints
                .first()
                .map(|choice| choice.identity.key.clone())
        });
    let selected = selected_key.as_ref().and_then(|key| {
        checkpoints
            .iter()
            .find(|choice| &choice.identity.key == key)
    });
    let follower = snapshot.robots().follower();
    let arm_ready = follower
        .as_ref()
        .is_some_and(|robot| robot.connection().connected() && robot.config().calibrated());
    let camera_summary = RolloutCameraSummary::inspect(
        follower.as_ref(),
        selected.map(|choice| choice.policy.camera_roles.as_slice()),
        snapshot.cameras(),
    );
    let follower_ready = arm_ready && camera_summary.ready;
    // A ready follower says only which cameras the policy sees; anything
    // else lists what stands in the way.
    let follower_detail = follower.as_ref().map(|robot| {
        if follower_ready {
            return camera_summary.detail.clone();
        }
        let connection = (!robot.connection().connected()).then_some("disconnected");
        let calibration = (!robot.config().calibrated()).then_some("needs calibration");
        let cameras = (!camera_summary.ready).then_some(camera_summary.detail.as_str());
        [connection, calibration, cameras]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    });
    let precondition_blocker = RolloutPreconditions {
        checkpoint: selected,
        follower: follower.as_ref(),
        cameras_ready: camera_summary.ready,
        controls_available: telemetry.controls_available(),
    }
    .blocker();
    let stop_id = rollout.session_id().map(str::to_owned);
    let active = rollout.phase().active();
    let stopping = rollout.phase() == RolloutPhase::Stopping;
    let pending = *pending_action.read();
    let duration_seconds = controller.duration_seconds();
    let duration_draft = if active {
        rollout
            .duration_seconds()
            .map(|seconds| seconds.to_string())
            .unwrap_or_else(|| controller.duration_draft())
    } else {
        controller.duration_draft()
    };
    let duration_valid = duration_seconds.is_some();
    let load_error = controller.error();
    let load_issue = load_error
        .clone()
        .or_else(|| (!controller.loaded()).then(|| "Loading rollout…".to_owned()));
    // The runtime line names a phase only when there is one to name; an idle
    // bench says just what protects the follower.
    let runtime_title = if load_error.is_some() {
        Some("Rollout status unavailable")
    } else if controller.loaded() {
        rollout.runtime_title()
    } else {
        None
    };
    let runtime_note_class = if load_error.is_some() {
        "rollout-runtime-note rollout-runtime-note-failed"
    } else {
        rollout.runtime_note_class()
    };
    let blocker = load_issue
        .clone()
        .or(precondition_blocker.clone())
        .or_else(|| {
            (!duration_valid).then_some(format!(
                "Set a whole-number duration from 1 to {MAX_ROLLOUT_DURATION_SECONDS} seconds."
            ))
        });
    let action_steps = selected
        .map(|choice| choice.policy.action_steps.to_string())
        .unwrap_or_else(|| "—".to_owned());
    let control_rate = selected
        .and_then(|choice| choice.policy.sample_hz)
        .map(|hz| format!("{hz:.1} Hz"))
        .unwrap_or_else(|| "—".to_owned());
    let validation_loss = selected
        .and_then(|choice| choice.validation_loss)
        .map(LossPresentation::loss_label)
        .unwrap_or_else(|| "—".to_owned());

    rsx! {
        div { class: "rollout-workbench studio-workbench-scroll",
            section { class: "rollout-section",
                Field { label: "Checkpoint",
                    Select {
                        value: selected_key.clone().unwrap_or_default(),
                        disabled: checkpoints.is_empty() || active,
                        onchange: move |value: String| {
                            controller.select_checkpoint((!value.is_empty()).then_some(value));
                        },
                        if checkpoints.is_empty() {
                            option { value: "", "No checkpoints" }
                        }
                        for choice in &checkpoints {
                            option {
                                value: choice.identity.key.clone(),
                                "{choice.identity.run_name} · step {choice.step.grouped_label()}"
                                if choice.identity.run_active { " · training" }
                            }
                        }
                    }
                }
                if selected.is_some() {
                    div { class: "rollout-checkpoint-meta",
                        span { "ACT · vision" }
                        span { "Validation {validation_loss}" }
                    }
                } else if let Some(run) = training.active_run() {
                    p { class: "rollout-empty-copy",
                        "Checkpoints appear every {run.config().checkpoint_every().grouped_label()} steps."
                    }
                }
            }

            section { class: "rollout-section",
                header { class: "rollout-section-heading", h3 { "Follower" } }
                if let Some(robot) = follower.as_ref() {
                    div { class: "rollout-target-row",
                        span {
                            class: "live-robot-accent",
                            style: "background: {crate::panels::scene::ScenePalette::accent_css(crate::panels::scene::ScenePalette::robot_index(robot))}",
                        }
                        strong { "{robot.id()}" }
                        small {
                            class: if follower_ready { "" } else { "rollout-target-problem" },
                            "{follower_detail.clone().unwrap_or_default()}"
                        }
                    }
                } else {
                    p { class: "rollout-empty-copy rollout-target-problem", "No follower configured" }
                }
            }

            section { class: "rollout-section",
                header { class: "rollout-section-heading", h3 { "Execution" } }
                FactList { mono: true, class: "rollout-receipt",
                    Fact { label: "Action steps", "{action_steps}" }
                    Fact { label: "Control rate", "{control_rate}" }
                }
                Field { label: "Duration", layout: FieldLayout::Inline, class: "rollout-duration-field",
                    TextInput {
                        kind: TextInputKind::Number,
                        min: "1",
                        max: "{MAX_ROLLOUT_DURATION_SECONDS}",
                        step: "1",
                        value: "{duration_draft}",
                        disabled: active || pending.is_some(),
                        title: "Policy control time; loading and warm-up are excluded",
                        aria_describedby: "rollout-duration-help",
                        aria_invalid: if duration_valid { "false" } else { "true" },
                        oninput: move |value| controller.set_duration_draft(value),
                    }
                    small {
                        id: "rollout-duration-help",
                        class: if duration_valid { "" } else { "rollout-duration-error" },
                        if duration_valid {
                            "s · then hold"
                        } else {
                            "Enter 1–{MAX_ROLLOUT_DURATION_SECONDS} s"
                        }
                    }
                }
            }

            div { class: "rollout-action-area",
                div {
                    class: runtime_note_class,
                    role: if load_error.is_some() { "alert" } else { "status" },
                    div { class: "rollout-runtime-summary",
                        if let Some(title) = runtime_title {
                            span { class: "rollout-runtime-dot", "aria-hidden": "true" }
                            strong { "{title}" }
                        }
                        small { "{load_issue.clone().unwrap_or_else(|| rollout.runtime_detail())}" }
                    }
                    if load_error.is_some() {
                        Button {
                            purpose: ActionPurpose::Retry,
                            onclick: move |_| controller.retry(),
                            "Retry rollout status"
                        }
                    }
                    if let Some(warning) = rollout.warning().as_ref() {
                        small { class: "rollout-runtime-warning", "{warning}" }
                    }
                }
                div { class: "rollout-actions",
                    Button {
                        purpose: ActionPurpose::Start,
                        variant: ButtonVariant::Solid,
                        size: ButtonSize::Prominent,
                        class: "rollout-run-action",
                        disabled: pending.is_some() || active || blocker.is_some(),
                        aria_busy: pending == Some(PendingRolloutAction::Start),
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
                                let config = RolloutConfig::new(
                                    checkpoint.identity.run_id,
                                    checkpoint.step,
                                    robot.id(),
                                )
                                .with_duration(duration_seconds);
                                let client = RolloutClient::current();
                                spawn(async move {
                                    let result = client.start_rollout(config)
                                        .await
                                        .map_err(|error| error.to_string());
                                    if let Err(error) = result {
                                        project.report_from(MessageSource::Rollout, error);
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
                    Button {
                        purpose: ActionPurpose::Stop,
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Prominent,
                        class: "rollout-stop-action",
                        disabled: pending.is_some() || !active || stopping,
                        aria_busy: pending == Some(PendingRolloutAction::Stop) || stopping,
                        title: "Stop policy control and hold the follower at its current position",
                        aria_label: "Stop policy and hold the follower",
                        onclick: move |_| {
                            if pending_action.peek().is_some() || !active || stopping {
                                return;
                            }
                            let Some(id) = stop_id.clone() else { return; };
                            pending_action.set(Some(PendingRolloutAction::Stop));
                            let mut pending_action = pending_action;
                            spawn(async move {
                                if let Err(error) = RolloutClient::current().stop_rollout(id)
                                    .await
                                    .map_err(|error| error.to_string())
                                {
                                    project.report_from(MessageSource::Rollout, error);
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
                if !active && load_issue.is_none() && let Some(reason) = blocker.as_ref() {
                    p { class: "rollout-blocker", "{reason}" }
                }
                if rollout.phase() == RolloutPhase::Failed && let Some(reason) = rollout.message().as_ref() {
                    p { class: "rollout-blocker rollout-failure", "{reason}" }
                }
            }
        }
    }
}

impl RolloutCameraSummary {
    fn inspect(
        follower: Option<&RobotSummary>,
        expected_roles: Option<&[String]>,
        camera_statuses: &[CameraStatus],
    ) -> Self {
        let Some(follower) = follower else {
            return Self {
                ready: false,
                detail: "no cameras".to_owned(),
            };
        };
        let camera_roles = expected_roles
            .filter(|roles| !roles.is_empty())
            .map(|roles| roles.to_vec())
            .unwrap_or_else(|| follower.config().cameras().keys().cloned().collect());
        let total = camera_roles.len();
        if total == 0 {
            return Self {
                ready: false,
                detail: "no cameras".to_owned(),
            };
        }

        let available = camera_roles
            .iter()
            .filter(|role| {
                follower
                    .config()
                    .cameras()
                    .get(role.as_str())
                    .is_some_and(|camera| {
                        camera_statuses
                            .iter()
                            .any(|status| status.id() == camera.id() && status.error().is_none())
                    })
            })
            .count();
        let ready = available == total;
        // Every camera answering is the resting state, said as a count; a
        // missing one is said as the gap.
        let detail = match (total, available) {
            (1, 1) => format!("{} camera", camera_roles[0]),
            (1, _) => format!("{} unavailable", camera_roles[0]),
            (_, available) if ready => format!("{available} cameras"),
            _ => format!("{available} of {total} cameras available"),
        };
        Self { ready, detail }
    }
}

/// Every measured value of the policy loop on one strip, em dashes until
/// the policy reports its first action.
#[component]
pub(super) fn InferencePanel(controller: super::state::RolloutController) -> Element {
    let rollout = controller.snapshot().read().to_owned();
    let metrics = rollout.metrics().to_owned();
    let measured = metrics.inference_count() > 0;
    let count = |value: u64| {
        if measured {
            value.grouped_label()
        } else {
            "—".to_owned()
        }
    };
    let missed = metrics.missed_deadlines();
    rsx! {
        section { class: "rollout-inference",
            FactList { layout: FactLayout::Strip, mono: true, class: "rollout-metric-strip",
                LatencyMetric { label: "Latency", value: rollout.latency_label(RolloutLatency::Last) }
                LatencyMetric { label: "Mean", value: rollout.latency_label(RolloutLatency::Mean) }
                LatencyMetric { label: "Max", value: rollout.latency_label(RolloutLatency::Peak) }
                LatencyMetric { label: "Model", value: rollout.latency_label(RolloutLatency::Model) }
                Fact { label: "Misses", class: if missed > 0 { "rollout-metric-missed" } else { "" },
                    span { {count(missed)} }
                }
                Fact { label: "Chunks", span { {count(metrics.inference_count())} } }
                Fact { label: "Commands", span { {count(metrics.command_count())} } }
            }
            if !measured {
                EmptyState { p { "No measurements yet" } }
            }
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

/// Training history, read for the one thing Rollout wants from it: the
/// checkpoints a policy can actually be run from, newest step first.
pub(super) trait TrainingHistoryRollout {
    fn checkpoint_choices(&self) -> Vec<CheckpointChoice>;
    /// The cameras the chosen checkpoint's policy expects, which is what
    /// Rollout docks camera panels for. Falls back to the newest checkpoint.
    fn selected_checkpoint_camera_roles(&self, selected_key: Option<&str>) -> Vec<String>;
}

impl TrainingHistoryRollout for crate::activities::training::history::TrainingHistory {
    fn checkpoint_choices(&self) -> Vec<CheckpointChoice> {
        CheckpointChoice::listed(self.snapshot().read().runs(), &self.checkpoints().read())
    }

    fn selected_checkpoint_camera_roles(&self, selected_key: Option<&str>) -> Vec<String> {
        let choices = self.checkpoint_choices();
        selected_key
            .and_then(|key| choices.iter().find(|choice| choice.identity.key == key))
            .or_else(|| choices.first())
            .map(|choice| choice.policy.camera_roles.clone())
            .unwrap_or_default()
    }
}

impl CheckpointChoice {
    /// Every checkpoint whose run Studio still records, newest step first.
    fn listed(runs: &[TrainingRun], checkpoints: &[TrainingCheckpoint]) -> Vec<Self> {
        let mut choices = checkpoints
            .iter()
            .filter_map(|checkpoint| {
                let run = runs.iter().find(|run| run.id() == checkpoint.run_id())?;
                Some(Self {
                    identity: CheckpointIdentity {
                        key: format!("{}::{}", run.id(), checkpoint.step()),
                        run_id: run.id().to_owned(),
                        run_name: run.config().run_name().to_owned(),
                        run_active: run.phase().active(),
                    },
                    step: checkpoint.step(),
                    validation_loss: checkpoint.validation_loss(),
                    policy: CheckpointPolicy::from(checkpoint),
                })
            })
            .collect::<Vec<_>>();
        choices.sort_by_key(|choice| std::cmp::Reverse(choice.step));
        choices
    }
}

impl From<&TrainingCheckpoint> for CheckpointPolicy {
    fn from(checkpoint: &TrainingCheckpoint) -> Self {
        let policy = checkpoint.policy();
        // A recorded camera stream is named `{robot}.camera.{role}`; the policy
        // was trained on the roles, in first-seen order.
        let mut camera_roles = Vec::new();
        for stream in policy.camera_streams() {
            let role = stream.rsplit('.').next().unwrap_or(stream).to_owned();
            if !camera_roles.contains(&role) {
                camera_roles.push(role);
            }
        }
        Self {
            action_steps: policy.action_steps().min(MAX_ROLLOUT_ACTION_STEPS),
            sample_hz: policy.sample_hz(),
            camera_roles,
        }
    }
}

struct RolloutPreconditions<'a> {
    checkpoint: Option<&'a CheckpointChoice>,
    follower: Option<&'a RobotSummary>,
    cameras_ready: bool,
    controls_available: bool,
}

impl RolloutPreconditions<'_> {
    fn blocker(&self) -> Option<String> {
        // The newest checkpoint is chosen by default, so none chosen means
        // none exist.
        let Some(checkpoint) = self.checkpoint else {
            return Some("Train a policy to create a checkpoint.".to_owned());
        };
        if checkpoint.identity.run_active {
            return Some("Stop or finish training before running this checkpoint.".to_owned());
        }
        let Some(follower) = self.follower else {
            return Some("Configure a follower in Settings.".to_owned());
        };
        if !follower.connection().connected() {
            return Some("Connect the follower.".to_owned());
        }
        if !follower.config().calibrated() {
            return Some("Calibrate the follower.".to_owned());
        }
        if !self.cameras_ready {
            return Some("Make every policy camera available.".to_owned());
        }
        if !self.controls_available {
            return Some("Waiting for fresh telemetry.".to_owned());
        }
        None
    }
}

/// A rollout as this screen reads it aloud: what the runtime line says, how it
/// is toned, and what the inference strip prints. `RolloutSnapshot` lives in
/// `crates/common`, so this is an extension trait rather than an inherent impl;
/// it is one trait for one subject, not one per method.
trait RolloutPresentation {
    /// The phase worth naming, if any: an idle bench that has not run is the
    /// resting state and names nothing, and its blocker sits beside Run.
    fn runtime_title(&self) -> Option<&'static str>;
    fn runtime_detail(&self) -> String;
    fn runtime_note_class(&self) -> &'static str;
    /// A latency in milliseconds to one decimal, or an em dash when the policy
    /// has not reported one. The unit is printed beside the figure, not in it.
    fn latency_label(&self, measure: RolloutLatency) -> String;
}

/// Which of the four latencies the inference strip is asking for.
#[derive(Clone, Copy)]
enum RolloutLatency {
    Last,
    Model,
    Mean,
    Peak,
}

impl RolloutPresentation for RolloutSnapshot {
    fn runtime_title(&self) -> Option<&'static str> {
        match self.phase() {
            RolloutPhase::Idle if self.finished_ms().is_some() => Some("Policy stopped"),
            RolloutPhase::Idle => None,
            RolloutPhase::Loading => Some("Loading checkpoint"),
            RolloutPhase::WarmingUp => Some("Preparing policy"),
            RolloutPhase::Running => Some("Policy active"),
            RolloutPhase::Stopping => Some("Stopping policy"),
            RolloutPhase::Failed => Some("Rollout stopped"),
        }
    }

    fn runtime_detail(&self) -> String {
        self.message()
            .map(str::to_owned)
            .unwrap_or_else(|| "Safety limits and hold-on-stop are active.".to_owned())
    }

    fn runtime_note_class(&self) -> &'static str {
        match self.phase() {
            RolloutPhase::Running if self.warning().is_some() => {
                "rollout-runtime-note rollout-runtime-note-warning"
            }
            RolloutPhase::Running => "rollout-runtime-note rollout-runtime-note-active",
            RolloutPhase::Failed => "rollout-runtime-note rollout-runtime-note-failed",
            _ => "rollout-runtime-note",
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
    use spiral_robotics_common::robots::config::{MotorRange, RobotConfig};
    use spiral_robotics_common::robots::{
        CameraStatus, CameraStreamFormat, CameraStreamState, ConnectionStatus, RobotSummary,
    };
    use spiral_robotics_common::rollout::{RolloutPhase, RolloutSnapshot};
    use spiral_robotics_common::training::{
        CheckpointPolicy as TrainedPolicy, TrainingCheckpoint, TrainingConfig, TrainingRun,
        TrainingRunIdentity, TrainingRunPhase, TrainingRunStatus,
    };

    use super::{
        CheckpointChoice, MAX_ROLLOUT_ACTION_STEPS, RobotFleet, RolloutCameraSummary,
        RolloutPreconditions, RolloutPresentation,
    };

    /// A connected, calibrated SO-101 of the given role.
    fn config(id: &str, role: &str) -> RobotConfig {
        let config = RobotConfig::so101(id, role).unwrap();
        let range = MotorRange::new(0, 1, 2).unwrap();
        let ranges = config
            .motors()
            .keys()
            .map(|name| (name.clone(), range))
            .collect();
        config.with_calibration(ranges).unwrap()
    }

    fn robot(id: &str, role: &str) -> RobotSummary {
        RobotSummary::new(config(id, role), 1, ConnectionStatus::new(true, None))
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn rollout_target_is_always_the_configured_follower() {
        let robots = [
            robot("leader", "so101_leader"),
            robot("follower", "so101_follower"),
        ];
        assert_eq!(
            robots.follower().map(|robot| robot.id().to_owned()),
            Some("follower".to_owned())
        );
    }

    /// Idle is the resting state: the runtime line names no phase, so it can
    /// never claim a blocked bench is ready.
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn idle_rollout_names_no_phase() {
        assert_eq!(RolloutSnapshot::default().runtime_title(), None);
        assert_eq!(
            RolloutSnapshot::default()
                .with_lifecycle(RolloutPhase::Running, None)
                .runtime_title(),
            Some("Policy active")
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn an_active_clamp_warning_uses_the_caution_runtime_state() {
        let rollout = RolloutSnapshot::default()
            .with_lifecycle(RolloutPhase::Running, None)
            .with_warning(Some("Policy output was clamped.".to_owned()));

        assert_eq!(
            rollout.runtime_note_class(),
            "rollout-runtime-note rollout-runtime-note-warning"
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn rollout_requires_every_assigned_camera() {
        let cameras = config("follower", "so101_follower")
            .with_camera("gripper", "camera-1")
            .and_then(|config| config.with_camera("top", "camera-2"))
            .unwrap();
        let follower = RobotSummary::new(cameras, 1, ConnectionStatus::new(true, None));
        let camera = |id: &str, error: Option<&str>| {
            CameraStatus::new(id, id, CameraStreamFormat::new(640, 360, 30))
                .with_state(CameraStreamState::new(true, 1, error.map(str::to_owned)))
        };

        assert_eq!(
            RolloutCameraSummary::inspect(
                Some(&follower),
                Some(&["gripper".to_owned(), "top".to_owned()]),
                &[camera("camera-1", None), camera("camera-2", None)],
            ),
            RolloutCameraSummary {
                ready: true,
                detail: "2 cameras".to_owned(),
            }
        );
        assert_eq!(
            RolloutCameraSummary::inspect(
                Some(&follower),
                Some(&["gripper".to_owned(), "top".to_owned()]),
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

    /// A recorded camera stream is `{robot}.camera.{role}`; the policy names
    /// the roles, once each.
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn rollout_lists_recorded_runs_checkpoints_newest_first() {
        let run = TrainingRun::new(
            TrainingRunIdentity::new("active-run", TrainingConfig::default(), 1),
            TrainingRunStatus::new(TrainingRunPhase::Running, 0, None),
        );
        let policy = TrainedPolicy::new(
            vec![
                "follower.camera.gripper".to_owned(),
                "follower.camera.top".to_owned(),
                "leader.camera.top".to_owned(),
            ],
            100,
            Some(30.0),
        );
        let checkpoints = [
            TrainingCheckpoint::new("active-run", 10_000).with_policy(policy.clone()),
            TrainingCheckpoint::new("active-run", 20_000).with_policy(policy),
            TrainingCheckpoint::new("deleted-run", 30_000),
        ];

        let choices = CheckpointChoice::listed(&[run], &checkpoints);

        assert_eq!(choices[0].policy.camera_roles, ["gripper", "top"]);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].step, 20_000);
        assert!(choices[0].identity.run_active);
        assert_eq!(choices[0].policy.action_steps, MAX_ROLLOUT_ACTION_STEPS);
        assert_eq!(
            RolloutPreconditions {
                checkpoint: choices.first(),
                follower: None,
                cameras_ready: false,
                controls_available: false,
            }
            .blocker()
            .as_deref(),
            Some("Stop or finish training before running this checkpoint.")
        );
    }
}
