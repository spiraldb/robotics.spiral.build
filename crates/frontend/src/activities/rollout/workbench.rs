//! Rollout's workbench: who controls the arms, where takes go, and the
//! selected arm's motors.

use dioxus::prelude::*;
use spiral_robotics_common::robots::ModeSelection;
use spiral_robotics_ui::{
    ActionPurpose, BlockingConfirmation, Button, ButtonVariant, ConfirmationCopy,
    ConfirmationDismissal, ConfirmationResponse, Field, FieldLayout, ModeButton, ModeGroup,
    TextInput, Toggle,
};

use super::next_take::NextTake;
use super::policy::PolicySection;
use super::session::RolloutSession;
use crate::activities::rollout::actions::RolloutAction;
use crate::activities::rollout::joints::JointPanel;
use crate::activities::rollout::recording_client::{RecordingTransfer, StoringTakes};
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::robots::RobotSelection;
use crate::robots::Selection;
use crate::shell::routes::StudioRoute;
use crate::shell::state::StudioProjectState;
use spiral_robotics_common::recording::RecordingProgress;

/// The complete left-hand workbench for operating the bench.
#[component]
pub(super) fn RolloutWorkbench(session: RolloutSession) -> Element {
    let (project, hardware, selection) =
        (session.project(), session.telemetry(), session.selection());
    let robots = hardware.snapshot().read().robots().to_owned();
    let on_action = move |action| session.dispatch(action);
    let navigator = use_navigator();

    rsx! {
        div { class: "studio-workbench-scroll rollout-workbench",
            ControlSection { session }
            CaptureSection { next_take: session.next_take(), recording: hardware.recording() }
            WaitingTakes { project, storing: session.storing_takes() }

            section { class: "workbench-section rollout-motors",
                header { class: "workbench-section-heading",
                    h3 { "Motors" }
                    Button {
                        purpose: ActionPurpose::Navigate,
                        variant: ButtonVariant::Link,
                        class: "workbench-heading-action",
                        title: "Ports, cameras, calibration, and the robot connector",
                        onclick: move |_| { navigator.push(StudioRoute::RolloutSetup {}); },
                        "Set up"
                    }
                }
                RobotRoster {
                    selection,
                    robots,
                    on_select: move |selection| on_action(RolloutAction::Select(selection)),
                }
                JointPanel {
                    project, hardware, robot_selection: selection,
                    on_select: move |selection| on_action(RolloutAction::Select(selection)),
                    on_point: move |selection| on_action(RolloutAction::Point(selection)),
                }
            }
        }
    }
}

#[component]
fn CaptureSection(next_take: NextTake, recording: Option<RecordingProgress>) -> Element {
    let mut dataset = next_take.dataset();
    let mut video = next_take.video();
    let dataset_ids = next_take.suggestions();
    let catalog = next_take.catalog();

    rsx! {
        section { class: "workbench-section capture-destination",
            header { class: "workbench-section-heading",
                h3 { "Capture destination" }
                if let Some(recording) = recording.as_ref() {
                    span { "Recording {recording.episode()}" }
                }
            }
            Field { label: "Dataset", layout: FieldLayout::Inline, class: "capture-dataset-field",
                TextInput {
                    list: "rollout-dataset-options",
                    value: dataset.read().to_owned(),
                    disabled: recording.is_some(),
                    maxlength: "96",
                    pattern: "[A-Za-z0-9_.-]+",
                    placeholder: "pick-place-v1",
                    oninput: move |value| dataset.set(value),
                }
            }
            datalist { id: "rollout-dataset-options",
                for id in dataset_ids { option { value: "{id}" } }
            }
            Toggle {
                label: "Record video",
                checked: video(),
                disabled: recording.is_some(),
                onchange: move |value| video.set(value),
            }
            if let Some(problem) = catalog.problem() {
                div { class: "min-w-0 rounded bg-danger-soft px-2 py-1.5 text-[11px] text-danger", role: "status",
                    strong { class: "block font-medium", "Could not load datasets" }
                    div { class: "break-words", "{problem}" }
                    div { class: "mt-1", "{problem.recovery_guidance()}" }
                    Button {
                        purpose: ActionPurpose::Retry,
                        variant: ButtonVariant::Outline,
                        class: "mt-1",
                        aria_label: "Retry loading datasets",
                        onclick: move |_| catalog.reread(),
                        "Retry"
                    }
                }
            } else if catalog.reading() {
                div { class: "text-[11px] text-ink-faint", role: "status", "Loading datasets…" }
            }
        }
    }
}

/// Finished takes the robot connector still holds because carrying them to
/// Studio failed, with a way to try again. A take on its way is not one of
/// them.
#[component]
fn WaitingTakes(project: StudioProjectState, storing: StoringTakes) -> Element {
    let waiting = QueryPolicy::on_demand(QueryDomain::Recording)
        .use_resource(|| RecordingTransfer::current().waiting());
    let mut carrying = use_signal(|| false);
    let takes = match &*waiting.read() {
        Some(Ok(takes)) => takes
            .iter()
            .filter(|take| !storing.includes(take.episode_id()))
            .cloned()
            .collect::<Vec<_>>(),
        _ => return rsx! {},
    };
    if takes.is_empty() {
        return rsx! {};
    }
    let count = takes.len();
    rsx! {
        section { class: "workbench-section waiting-takes", role: "status",
            p {
                if count == 1 {
                    "1 take not stored yet"
                } else {
                    "{count} takes not stored yet"
                }
            }
            Button {
                purpose: ActionPurpose::Retry,
                variant: ButtonVariant::Outline,
                disabled: carrying(),
                aria_busy: carrying(),
                onclick: move |_| {
                    carrying.set(true);
                    let takes = takes.clone();
                    let transfer = RecordingTransfer::current();
                    spawn(async move {
                        for take in &takes {
                            if let Err(error) = transfer.carry(take.dataset_id(), take.episode_id()).await {
                                project.report_from(
                                    crate::shell::notifications::MessageSource::Rollout,
                                    format!("Could not store take {} in Studio: {error}", take.episode_id()),
                                );
                                break;
                            }
                        }
                        carrying.set(false);
                    });
                },
                if carrying() { "Storing…" } else { "Store in Studio" }
            }
        }
    }
}

#[component]
fn RobotRoster(
    selection: RobotSelection,
    robots: Vec<spiral_robotics_common::robots::RobotSummary>,
    on_select: EventHandler<Option<Selection>>,
) -> Element {
    let focused = selection
        .focused(&robots)
        .map(|robot| robot.id().to_owned());
    let empty = robots.is_empty();

    rsx! {
        section { class: "rollout-robot-roster", role: "tablist", "aria-label": "Robot arms",
            for robot in robots {
                {
                    let id = robot.id().to_owned();
                    let selected = focused.as_deref() == Some(robot.id());
                    let ready = robot.connection().connected() && robot.config().calibrated();
                    let role = robot.config().robot_type().map(str::role_label).unwrap_or("arm");
                    let show_role = !robot.id().eq_ignore_ascii_case(role);
                    let state = if !robot.connection().connected() {
                        "Disconnected"
                    } else if !robot.config().calibrated() {
                        "Calibration needed"
                    } else {
                        "Connected and calibrated"
                    };
                    let accessible_label = if show_role {
                        format!("{}, {role}, {state}", robot.id())
                    } else {
                        format!("{}, {state}", robot.id())
                    };
                    let accent =
                        crate::panels::scene::ScenePalette::accent_css(
                            crate::panels::scene::ScenePalette::robot_index(&robot),
                        );
                    rsx! {
                        Button {
                            key: "{robot.id()}",
                            purpose: ActionPurpose::Select,
                            variant: ButtonVariant::Row,
                            class: "rollout-robot-row",
                            role: "tab",
                            aria_pressed: selected,
                            aria_selected: selected,
                            aria_label: "{accessible_label}",
                            title: "{state}",
                            onclick: move |_| on_select.call(Some(Selection::Robot(id.clone()))),
                            span {
                                class: "rollout-robot-accent",
                                style: "background: {accent}",
                            }
                            span { class: "rollout-robot-copy",
                                strong { "{robot.id()}" }
                                if show_role { small { "{role}" } }
                            }
                            // A ready arm says nothing; one that cannot be driven
                            // names why, so the exception is what finds the eye.
                            if !ready {
                                span { class: "rollout-robot-problem", "aria-hidden": "true", "{state}" }
                            }
                        }
                    }
                }
            }
            if empty {
                p { class: "workbench-empty", "Set up an arm to begin." }
            }
        }
    }
}

/// Who controls the arms: Studio holds or releases the selected arm, or the
/// leader drives the follower. Hold ends teleoperation. A checkpoint cannot
/// drive yet.
#[component]
fn ControlSection(session: RolloutSession) -> Element {
    let (hardware, selection, policy) =
        (session.telemetry(), session.selection(), session.policy());
    let on_action = move |action| session.dispatch(action);
    let focused = selection.focused(hardware.snapshot().read().robots());
    let telemetry = focused
        .as_ref()
        .and_then(|robot| hardware.robot_telemetry(robot.id()));
    let holding = telemetry.as_ref().is_some_and(|robot| robot.torque());
    let teleop =
        matches!(hardware.mode(), ModeSelection::Teleoperate { .. }) && hardware.mode_active();
    let pair = hardware.snapshot().read().default_teleoperation_pair();
    let policy_blocker = session.policy_blocker();
    let mut confirm_release = use_signal(|| false);

    rsx! {
        section { class: "workbench-section control-section",
            header { class: "workbench-section-heading",
                h3 { "Control" }
                if teleop {
                    span { "Leader → follower" }
                }
            }
            ModeGroup { class: "control-segmented", label: "Who controls the arms",
                ModeButton {
                    purpose: ActionPurpose::Hold,
                    checked: holding && !teleop,
                    class: "control-segment",
                    disabled: focused.is_none() || telemetry.is_none(),
                    title: "Hold the arm where it is",
                    onclick: {
                        let id = focused.as_ref().map(|robot| robot.id().to_owned());
                        move |_| if (!holding || teleop)
                            && let Some(id) = id.clone()
                        {
                            on_action(RolloutAction::Hold(id));
                        }
                    },
                    "Hold"
                }
                ModeButton {
                    id: "motor-mode-release",
                    purpose: ActionPurpose::Release,
                    // An arm the Studio cannot hear from is in no known mode.
                    checked: telemetry.is_some() && !holding && !teleop,
                    class: "control-segment",
                    aria_label: "Release motor torque",
                    disabled: focused.is_none() || telemetry.is_none(),
                    onclick: move |_| if holding || teleop { confirm_release.set(true) },
                    "Release"
                }
                ModeButton {
                    purpose: ActionPurpose::Teleoperate,
                    checked: teleop,
                    class: "control-segment",
                    disabled: pair.is_none() || !hardware.controls_available(),
                    title: if pair.is_none() { "Set up a leader to drive the follower by hand" } else { "Drive the follower from the leader" },
                    onclick: move |_| {
                        if !teleop && let Some((leader, follower)) = pair.clone() {
                            on_action(RolloutAction::Teleoperate { leader, follower });
                        }
                    },
                    "Teleop"
                }
                ModeButton {
                    purpose: ActionPurpose::Start,
                    checked: false,
                    class: "control-segment",
                    disabled: policy_blocker.is_some(),
                    title: policy_blocker.clone().unwrap_or_else(|| "Run the checkpoint on the follower".to_owned()),
                    onclick: move |_| on_action(RolloutAction::RunPolicy),
                    "Policy"
                }
            }
            PolicySection { policy, blocker: policy_blocker }
            if confirm_release() {
                BlockingConfirmation {
                    id: "release-arm-confirmation",
                    title: focused.as_ref().map(|robot| format!("Release {}?", robot.id())).unwrap_or_else(|| "Release arm?".to_owned()),
                    consequence: "Motor torque will turn off immediately and the arm can fall under its own weight.".to_owned(),
                    copy: ConfirmationCopy::new(
                        "Support the arm before continuing.",
                        "Release arm",
                        "Keep holding",
                    ),
                    response: ConfirmationResponse::new(
                        ActionPurpose::Release,
                        {
                            let id = focused.as_ref().map(|robot| robot.id().to_owned());
                            move |_| {
                                confirm_release.set(false);
                                if let Some(id) = id.clone() { on_action(RolloutAction::Release(id)); }
                            }
                        },
                        move |_| confirm_release.set(false),
                    ),
                    dismissal: ConfirmationDismissal::new("motor-mode-release"),
                }
            }
        }
    }
}

trait RobotRolePresentation {
    fn role_label(&self) -> &'static str;
}

impl RobotRolePresentation for str {
    fn role_label(&self) -> &'static str {
        let kind = self.to_ascii_lowercase();
        if kind.contains("follower") {
            "follower"
        } else if kind.contains("leader") {
            "leader"
        } else {
            "arm"
        }
    }
}
