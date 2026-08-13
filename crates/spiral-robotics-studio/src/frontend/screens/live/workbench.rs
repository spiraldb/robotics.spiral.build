//! Live screen workbench: capture, control, and selected-arm state.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use spiral_robotics_core::api::{ModeSelection, default_teleoperation_pair};

use crate::frontend::app::state::{Selection, studio};
use crate::frontend::screens::live::actions::LiveAction;
use crate::frontend::screens::live::control::JointPanel;

/// The complete left-hand workbench for operating a connected pair.
#[component]
pub fn LiveWorkbench(on_action: EventHandler<LiveAction>) -> Element {
    let studio = studio();
    let robots = studio.snapshot.read().robots.clone();

    rsx! {
        div { class: "studio-workbench-scroll live-workbench",
            ControlSection { on_action }
            CaptureSection {}

            section { class: "workbench-section live-properties",
                header { class: "workbench-section-heading",
                    h3 { "Motors" }
                }
                RobotRoster {
                    robots,
                    on_select: move |selection| on_action.call(LiveAction::Select(selection)),
                }
                JointPanel {
                    on_select: move |selection| on_action.call(LiveAction::Select(selection)),
                    on_point: move |selection| on_action.call(LiveAction::Point(selection)),
                }
            }
        }
    }
}

#[component]
fn CaptureSection() -> Element {
    let studio = studio();
    let mut selected_dataset = studio.selected_dataset;
    let recording = studio.recording();
    let dataset_ids = studio
        .episodes()
        .into_iter()
        .map(|episode| episode.dataset_id)
        .chain(std::iter::once(selected_dataset.read().clone()))
        .collect::<BTreeSet<_>>();

    rsx! {
        section { class: "workbench-section capture-destination",
            header { class: "workbench-section-heading",
                h3 { "Capture destination" }
                if let Some(recording) = recording.as_ref() {
                    span { "Recording {recording.episode}" }
                }
            }
            label { class: "capture-dataset-field",
                span { "Dataset" }
                input {
                    class: "field",
                    list: "live-dataset-options",
                    value: selected_dataset.read().clone(),
                    disabled: recording.is_some(),
                    maxlength: "96",
                    pattern: "[A-Za-z0-9_.-]+",
                    placeholder: "pick-place-v1",
                    oninput: move |event| selected_dataset.set(event.value()),
                }
            }
            datalist { id: "live-dataset-options",
                for id in dataset_ids { option { value: "{id}" } }
            }
            p {
                if recording.is_some() {
                    "Dataset assignment is locked until this episode finishes."
                } else {
                    "New episodes are saved to this dataset."
                }
            }
        }
    }
}

#[component]
fn RobotRoster(
    robots: Vec<spiral_robotics_core::api::RobotSummary>,
    on_select: EventHandler<Option<Selection>>,
) -> Element {
    let studio = studio();
    let focused = studio.focused().map(|robot| robot.id);
    let empty = robots.is_empty();

    rsx! {
        section { class: "live-robot-roster", role: "tablist", "aria-label": "Robot arms",
            for robot in robots {
                {
                    let id = robot.id.clone();
                    let selected = focused.as_deref() == Some(robot.id.as_str());
                    let ready = robot.connection.connected && robot.calibrated && robot.problem.is_none();
                    let role = robot.robot_type.as_deref().map(role_label).unwrap_or("arm");
                    let show_role = !robot.id.eq_ignore_ascii_case(role);
                    let state = if robot.problem.is_some() {
                        "Needs attention"
                    } else if !robot.connection.connected {
                        "Disconnected"
                    } else if !robot.calibrated {
                        "Calibration needed"
                    } else {
                        "Connected and calibrated"
                    };
                    let accessible_label = if show_role {
                        format!("{}, {role}, {state}", robot.id)
                    } else {
                        format!("{}, {state}", robot.id)
                    };
                    let accent =
                        crate::frontend::panels::scene::accent_css(
                            studio.position_of(&robot.id),
                        );
                    rsx! {
                        button {
                            key: "{robot.id}",
                            r#type: "button",
                            class: if selected { "live-robot-row live-robot-row-selected" } else { "live-robot-row" },
                            role: "tab",
                            "aria-pressed": selected,
                            "aria-selected": selected,
                            "aria-label": "{accessible_label}",
                            title: "{state}",
                            onclick: move |_| on_select.call(Some(Selection::Robot(id.clone()))),
                            span {
                                class: "live-robot-accent",
                                style: "background: {accent}",
                            }
                            span { class: "live-robot-copy",
                                strong { "{robot.id}" }
                                if show_role { small { "{role}" } }
                            }
                            span {
                                class: if ready { "live-robot-state live-robot-state-ready" } else { "live-robot-state" },
                                "aria-hidden": "true",
                            }
                        }
                    }
                }
            }
            if empty {
                p { class: "workbench-empty", "Add an arm in Settings to begin." }
            }
        }
    }
}

#[component]
fn ControlSection(on_action: EventHandler<LiveAction>) -> Element {
    let studio = studio();
    let focused = studio.focused();
    let telemetry = focused
        .as_ref()
        .and_then(|robot| studio.robot_telemetry(&robot.id));
    let holding = telemetry.as_ref().is_some_and(|robot| robot.torque);
    let teleop = matches!(studio.mode(), ModeSelection::Teleoperate { .. }) && studio.mode_active();
    let pair = default_teleoperation_pair(&studio.snapshot.read().robots);
    let mut confirm_release = use_signal(|| false);

    rsx! {
        section { class: "workbench-section control-section",
            header { class: "workbench-section-heading",
                h3 { "Control" }
                if teleop { span { "Leader → follower" } }
            }
            div { class: "control-segmented", role: "group", "aria-label": "Motor control mode",
                button {
                    r#type: "button",
                    class: if holding && !teleop { "control-segment control-segment-active control-segment-energized" } else { "control-segment" },
                    disabled: focused.is_none() || telemetry.is_none(),
                    onclick: {
                        let id = focused.as_ref().map(|robot| robot.id.clone());
                        move |_| if let Some(id) = id.clone() { on_action.call(LiveAction::Hold(id)); }
                    },
                    "Hold"
                }
                button {
                    r#type: "button",
                    class: if !holding && !teleop { "control-segment control-segment-active" } else { "control-segment" },
                    disabled: focused.is_none() || telemetry.is_none(),
                    onclick: move |_| confirm_release.set(true),
                    "Release"
                }
                button {
                    r#type: "button",
                    class: if teleop { "control-segment control-segment-active control-segment-energized" } else { "control-segment" },
                    disabled: pair.is_none() || !studio.controls_available(),
                    onclick: move |_| {
                        if let Some((leader, follower)) = pair.clone() {
                            on_action.call(LiveAction::Teleoperate { leader, follower });
                        }
                    },
                    "Teleop"
                }
            }
            if confirm_release() {
                div { class: "inline-safety-confirm", role: "alert",
                    span { "Support the arm—it goes limp when released." }
                    button {
                        r#type: "button", class: "studio-button studio-button-danger",
                        onclick: {
                            let id = focused.as_ref().map(|robot| robot.id.clone());
                            move |_| {
                                confirm_release.set(false);
                                if let Some(id) = id.clone() { on_action.call(LiveAction::Release(id)); }
                            }
                        },
                        "Release"
                    }
                    button { r#type: "button", class: "studio-button", onclick: move |_| confirm_release.set(false), "Cancel" }
                }
            }
        }
    }
}

fn role_label(kind: &str) -> &'static str {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("follower") {
        "follower"
    } else if kind.contains("leader") {
        "leader"
    } else {
        "arm"
    }
}
