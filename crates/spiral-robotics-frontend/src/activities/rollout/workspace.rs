//! Checkpoint selection and follower rollout controls.

use crate::components::interaction::{Action, ActionPurpose, ActionSize, ActionVariant};
use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_api::robots::{CameraStatus, RobotSummary};
use spiral_robotics_api::rollout::{
    MAX_ROLLOUT_ACTION_STEPS, MAX_ROLLOUT_DURATION_SECONDS, RolloutPhase, RolloutSnapshot,
    StartRolloutRequest,
};
use spiral_robotics_api::training::{TrainingCheckpoint, TrainingRun, TrainingSnapshot};

use crate::client::http as api;
use crate::format::LossPresentation;
use crate::shell::notifications::MessageSource;
use crate::shell::state::studio;

#[derive(Clone, Debug, PartialEq)]
struct CheckpointChoice {
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
    sample_hz: f64,
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
pub(crate) trait RobotFleet {
    fn follower(&self) -> Option<RobotSummary>;
}

impl RobotFleet for [RobotSummary] {
    fn follower(&self) -> Option<RobotSummary> {
        self.iter()
            .find(|robot| {
                robot
                    .robot_type()
                    .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
            })
            .cloned()
    }
}

#[component]
pub(crate) fn RolloutWorkbench() -> Element {
    let studio = studio();
    let controller = studio.rollout_controller();
    let mut pending_action = use_signal(|| None::<PendingRolloutAction>);
    let snapshot = studio.snapshot().read().to_owned();
    let training = studio.training().read().to_owned();
    let rollout = studio.rollout().read().to_owned();
    let checkpoints = training.checkpoint_choices();
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
    let arm_ready = follower.as_ref().is_some_and(|robot| {
        robot.connection().connected() && robot.calibrated() && robot.problem().is_none()
    });
    let camera_summary = RolloutCameraSummary::inspect(
        follower.as_ref(),
        selected.map(|choice| choice.policy.camera_roles.as_slice()),
        snapshot.cameras(),
    );
    let follower_ready = arm_ready && camera_summary.ready;
    let follower_detail = follower.as_ref().map(|robot| {
        let connection = if robot.connection().connected() {
            "connected"
        } else {
            "disconnected"
        };
        let calibration = if robot.calibrated() {
            "calibrated"
        } else {
            "needs calibration"
        };
        format!("{connection} · {calibration} · {}", camera_summary.detail)
    });
    let precondition_blocker = RolloutPreconditions {
        checkpoint: selected,
        follower: follower.as_ref(),
        cameras_ready: camera_summary.ready,
        controls_available: studio.controls_available(),
    }
    .blocker();
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
    let blocker = precondition_blocker.clone().or_else(|| {
        (!duration_valid).then_some(format!(
            "Set a whole-number duration from 1 to {MAX_ROLLOUT_DURATION_SECONDS} seconds."
        ))
    });
    let action_steps = selected.map_or(0, |choice| choice.policy.action_steps);
    let control_rate = selected.map_or(0.0, |choice| choice.policy.sample_hz);
    let validation_loss = selected
        .and_then(|choice| choice.validation_loss)
        .map(LossPresentation::loss_label)
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
                                value: choice.identity.key.clone(),
                                "{choice.identity.run_name} · step {choice.step.rollout_count()}"
                                if choice.identity.run_active { " · training" }
                            }
                        }
                    }
                }
                if let Some(choice) = selected {
                    div { class: "rollout-checkpoint-meta",
                        span { "ACT · vision" }
                        span { "Validation {validation_loss}" }
                        span {
                            class: if choice.identity.run_active { "rollout-state" } else { "rollout-state rollout-state-ready" },
                            if choice.identity.run_active { "Training" } else { "Ready" }
                        }
                    }
                } else if let Some(run) = training.active_run() {
                    p { class: "rollout-empty-copy",
                        "Checkpoints appear every {run.config().checkpoint_every().rollout_count()} steps."
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
                            style: "background: {crate::panels::scene::ScenePalette::accent_css(studio.position_of(&robot.id()))}",
                        }
                        div {
                            strong { "{robot.id()}" }
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
                    class: rollout.runtime_note_class(),
                    role: "status",
                    span { class: "rollout-runtime-dot", "aria-hidden": "true" }
                    div {
                        div { class: "rollout-runtime-summary",
                            strong { "{rollout.runtime_title(blocker.as_deref())}" }
                            small { "{rollout.runtime_detail()}" }
                        }
                        if let Some(warning) = rollout.warning().as_ref() {
                            small { class: "rollout-runtime-warning", "{warning}" }
                        }
                    }
                }
                div { class: "rollout-actions",
                    Action {
                        purpose: ActionPurpose::Start,
                        variant: ActionVariant::Solid,
                        size: ActionSize::Prominent,
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
                                let request = StartRolloutRequest::new(
                                    checkpoint.identity.run_id,
                                    checkpoint.step,
                                    robot.id(),
                                )
                                .with_duration(duration_seconds);
                                spawn(async move {
                                    let result = api::Http::send_json::<_, RolloutSnapshot>(
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
                    Action {
                        purpose: ActionPurpose::Stop,
                        variant: ActionVariant::Outline,
                        size: ActionSize::Prominent,
                        class: "rollout-stop-action",
                        disabled: pending.is_some() || !active || stopping,
                        aria_busy: pending == Some(PendingRolloutAction::Stop) || stopping,
                        title: "Stop policy control and hold the follower at its current position",
                        aria_label: "Stop policy and hold the follower",
                        onclick: move |_| {
                            if pending_action.peek().is_some() || !active || stopping {
                                return;
                            }
                            pending_action.set(Some(PendingRolloutAction::Stop));
                            let mut pending_action = pending_action;
                            spawn(async move {
                                if let Err(error) = api::Http::post("/api/rollout/stop").await {
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
                if rollout.phase() == RolloutPhase::Failed && let Some(reason) = rollout.message().as_ref() {
                    p { class: "rollout-blocker", "{reason}" }
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
            .unwrap_or_else(|| {
                follower
                    .cameras()
                    .iter()
                    .map(|binding| binding.name().to_owned())
                    .collect()
            });
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
                    .cameras()
                    .iter()
                    .find(|binding| binding.name() == role.as_str())
                    .is_some_and(|binding| {
                        camera_statuses
                            .iter()
                            .any(|status| status.id() == binding.id() && status.error().is_none())
                    })
            })
            .count();
        let ready = available == total;
        let detail = match (total, available) {
            (1, 1) => format!("{} available", camera_roles[0]),
            (1, _) => format!("{} unavailable", camera_roles[0]),
            (_, available) if ready => format!("{available} cameras available"),
            _ => format!("{available} of {total} cameras available"),
        };
        Self { ready, detail }
    }
}

#[component]
pub(crate) fn InferencePanel() -> Element {
    let studio = studio();
    let rollout = studio.rollout().read().to_owned();
    let metrics = &rollout.metrics();
    let latency = metrics.last_latency_ms().rollout_metric();
    let model = metrics.last_model_ms().rollout_metric();
    let misses = metrics.missed_deadlines().to_string();
    rsx! {
        section { class: "rollout-inference",
            div { class: "rollout-metric-strip",
                InferenceMetric { label: "Latency", value: latency, unit: "ms" }
                InferenceMetric { label: "Model", value: model, unit: "ms" }
                InferenceMetric { label: "Misses", value: misses, unit: "" }
            }
            if metrics.inference_count() == 0 {
                div { class: "rollout-inference-empty",
                    strong { "No measurements yet" }
                    p { "Starts when the policy sends its first action." }
                }
            } else {
                div { class: "rollout-inference-empty",
                    strong { "{metrics.command_count().rollout_count()} commands" }
                    p {
                        "{metrics.inference_count().rollout_count()} chunks · mean {metrics.mean_latency_ms().rollout_metric()} ms · max {metrics.max_latency_ms().rollout_metric()} ms"
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

pub(crate) trait TrainingSnapshotRollout {
    fn selected_checkpoint_camera_roles(&self, selected_key: Option<&str>) -> Vec<String>;
}

trait TrainingSnapshotCheckpointChoices {
    fn checkpoint_choices(&self) -> Vec<CheckpointChoice>;
}

impl TrainingSnapshotCheckpointChoices for TrainingSnapshot {
    fn checkpoint_choices(&self) -> Vec<CheckpointChoice> {
        let mut choices = self
            .runs()
            .iter()
            .flat_map(TrainingRunRollout::checkpoint_choices)
            .collect::<Vec<_>>();
        choices.sort_by_key(|choice| std::cmp::Reverse(choice.step));
        choices
    }
}

impl TrainingSnapshotRollout for TrainingSnapshot {
    fn selected_checkpoint_camera_roles(&self, selected_key: Option<&str>) -> Vec<String> {
        let choices = self.checkpoint_choices();
        selected_key
            .and_then(|key| choices.iter().find(|choice| choice.identity.key == key))
            .or_else(|| choices.first())
            .map(|choice| choice.policy.camera_roles.clone())
            .unwrap_or_default()
    }
}

trait TrainingRunRollout {
    fn checkpoint_choices(&self) -> Vec<CheckpointChoice>;
}

impl TrainingRunRollout for TrainingRun {
    fn checkpoint_choices(&self) -> Vec<CheckpointChoice> {
        self.checkpoints()
            .iter()
            .filter_map(|checkpoint| CheckpointChoice::for_checkpoint(self, checkpoint))
            .collect()
    }
}

impl CheckpointChoice {
    fn for_checkpoint(run: &TrainingRun, checkpoint: &TrainingCheckpoint) -> Option<Self> {
        checkpoint.inference_ready().then(|| Self {
            identity: CheckpointIdentity {
                key: format!("{}::{}", run.id(), checkpoint.step()),
                run_id: run.id().to_owned(),
                run_name: run.config().run_name().to_owned(),
                run_active: run.phase().active(),
            },
            step: checkpoint.step(),
            validation_loss: checkpoint.validation_loss(),
            policy: CheckpointPolicy {
                action_steps: (if checkpoint.recommended_action_steps() == 0 {
                    run.config().chunk_size()
                } else {
                    checkpoint
                        .recommended_action_steps()
                        .min(run.config().chunk_size())
                })
                .min(MAX_ROLLOUT_ACTION_STEPS),
                sample_hz: checkpoint.sample_hz(),
                camera_roles: run.config().act().camera_streams().camera_roles(),
            },
        })
    }
}

trait CameraStreamRoles {
    fn camera_roles(&self) -> Vec<String>;
}

impl CameraStreamRoles for [String] {
    fn camera_roles(&self) -> Vec<String> {
        let mut roles = Vec::with_capacity(self.len());
        for stream in self {
            let role = stream.rsplit('.').next().unwrap_or(stream).to_owned();
            if !roles.contains(&role) {
                roles.push(role);
            }
        }
        roles
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
        let Some(checkpoint) = self.checkpoint else {
            return Some("Select a checkpoint.".to_owned());
        };
        if checkpoint.identity.run_active {
            return Some("Stop or finish training before running this checkpoint.".to_owned());
        }
        let Some(follower) = self.follower else {
            return Some("Configure a follower in Settings.".to_owned());
        };
        if follower.problem().is_some() {
            return Some("Fix the follower in Settings.".to_owned());
        }
        if !follower.connection().connected() {
            return Some("Connect the follower.".to_owned());
        }
        if !follower.calibrated() {
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

trait RolloutRuntimePresentation {
    fn runtime_title(&self, blocker: Option<&str>) -> &'static str;
    fn runtime_detail(&self) -> String;
    fn runtime_note_class(&self) -> &'static str;
}

impl RolloutRuntimePresentation for RolloutSnapshot {
    fn runtime_title(&self, blocker: Option<&str>) -> &'static str {
        match self.phase() {
            RolloutPhase::Idle if self.finished_ms().is_some() => "Policy stopped",
            RolloutPhase::Idle if blocker.is_some() => "Rollout unavailable",
            RolloutPhase::Idle => "Ready",
            RolloutPhase::Loading => "Loading checkpoint",
            RolloutPhase::WarmingUp => "Preparing policy",
            RolloutPhase::Running => "Policy active",
            RolloutPhase::Stopping => "Stopping policy",
            RolloutPhase::Failed => "Rollout stopped",
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
}

trait RolloutMetricPresentation {
    fn rollout_metric(self) -> String;
}

impl RolloutMetricPresentation for Option<f64> {
    fn rollout_metric(self) -> String {
        self.filter(|value| value.is_finite())
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| "—".to_owned())
    }
}

trait RolloutCountPresentation {
    fn rollout_count(self) -> String;
}

impl RolloutCountPresentation for u64 {
    fn rollout_count(self) -> String {
        let text = self.to_string();
        text.as_bytes()
            .rchunks(3)
            .rev()
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_api::robots::{
        CameraBinding, CameraStatus, CameraStreamFormat, CameraStreamState, ConnectionStatus,
        RobotAvailability, RobotSummary,
    };
    use spiral_robotics_api::rollout::{MAX_ROLLOUT_ACTION_STEPS, RolloutPhase, RolloutSnapshot};
    use spiral_robotics_api::training::{
        StartTrainingRequest, TrainingCheckpoint, TrainingRun, TrainingRunPhase, TrainingSource,
    };

    use super::{
        CameraStreamRoles, RobotFleet, RolloutCameraSummary, RolloutPreconditions,
        RolloutRuntimePresentation, TrainingRunRollout,
    };

    fn robot(id: &str, role: &str) -> RobotSummary {
        RobotSummary::new(
            id,
            Some(role.to_owned()),
            RobotAvailability::new(true, None, ConnectionStatus::new(true, None)),
        )
    }

    #[test]
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

    #[test]
    fn blocked_idle_rollout_never_claims_to_be_ready() {
        let rollout = RolloutSnapshot::default();
        assert_eq!(
            rollout.runtime_title(Some("Select a checkpoint.")),
            "Rollout unavailable"
        );
        assert_eq!(rollout.runtime_title(None), "Ready");
    }

    #[test]
    fn an_active_clamp_warning_uses_the_caution_runtime_state() {
        let rollout = RolloutSnapshot::default()
            .with_lifecycle(RolloutPhase::Running, None)
            .with_warning(Some("Policy output was clamped.".to_owned()));

        assert_eq!(
            rollout.runtime_note_class(),
            "rollout-runtime-note rollout-runtime-note-warning"
        );
    }

    #[test]
    fn rollout_requires_every_assigned_camera() {
        let mut follower = robot("follower", "so101_follower");
        follower = follower.with_cameras(vec![
            CameraBinding::new("gripper", "camera-1", None),
            CameraBinding::new("top", "camera-2", None),
        ]);
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
                detail: "2 cameras available".to_owned(),
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

    #[test]
    fn rollout_camera_roles_follow_the_recorded_stream_contract() {
        assert_eq!(
            [
                "follower.camera.gripper".to_owned(),
                "follower.camera.top".to_owned(),
            ]
            .camera_roles(),
            ["gripper", "top"]
        );
    }

    #[test]
    fn rollout_lists_inference_ready_checkpoints_from_an_active_run() {
        let run = TrainingRun::starting("active-run", StartTrainingRequest::default())
            .with_source(TrainingSource::default())
            .with_phase(TrainingRunPhase::Running, 0)
            .with_checkpoints(vec![
                TrainingCheckpoint::new(20_000, 0).with_artifact(0, true),
                TrainingCheckpoint::new(10_000, 0),
            ]);

        let choices = run.checkpoint_choices();

        assert_eq!(choices.len(), 1);
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
