//! Live operation workbench: capture, control, and the selected arm's state.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{ModeSelection, TorqueRequest, default_teleoperation_pair};

use crate::api;
use crate::control::JointPanel;
use crate::modes::set_mode;
use crate::state::{Selection, studio};

/// Semantic changes requested by the Live workbench. The shell applies these
/// to the activity projection; this component does not reach into selection
/// state behind the visualization's back.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LiveIntent {
    Select(Option<Selection>),
    Point(Option<Selection>),
}

/// The complete left-hand workbench for operating a connected pair.
#[component]
pub fn LiveWorkbench(on_intent: EventHandler<LiveIntent>) -> Element {
    let studio = studio();
    let robots = studio.snapshot.read().robots.clone();

    rsx! {
        div { class: "studio-workbench-scroll live-workbench",
            ControlSection {}

            section { class: "workbench-section live-properties",
                header { class: "workbench-section-heading",
                    h3 { "Motors" }
                }
                RobotRoster {
                    robots,
                    on_select: move |selection| on_intent.call(LiveIntent::Select(selection)),
                }
                JointPanel {
                    on_select: move |selection| on_intent.call(LiveIntent::Select(selection)),
                    on_point: move |selection| on_intent.call(LiveIntent::Point(selection)),
                }
            }
        }
    }
}

#[component]
fn RobotRoster(
    robots: Vec<rerobot_core::api::RobotSummary>,
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
                    let accent = crate::viewport::accent_css(studio.position_of(&robot.id));
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
fn ControlSection() -> Element {
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
                        move |_| if let Some(id) = id.as_deref() { hold_robot(studio, id); }
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
                            set_mode(studio, ModeSelection::Teleoperate { leader, follower });
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
                                if let Some(id) = id.as_deref() { release_robot(studio, id); }
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

/// Hold is the operator-facing form of Manual: select the focused arm as the
/// command target, then energize it. Keeping the requests in one ordered task
/// prevents the removed arm-local Manual control from remaining a hidden
/// prerequisite for writable sliders.
fn hold_robot(studio: crate::state::Studio, id: &str) {
    let id = id.to_owned();
    studio.run("", async move {
        let mode = ModeSelection::Manual { robot: id.clone() };
        api::send(Method::POST, "/api/mode", Some(&mode)).await?;
        api::send(
            Method::POST,
            &format!("/api/robots/{id}/torque"),
            Some(&TorqueRequest { enabled: true }),
        )
        .await
    });
}

/// Release always attempts the physical release even if leaving the current
/// mode fails. Torque-off is the safety consequence; an Idle transition is the
/// semantic cleanup and may report its own failure afterward.
fn release_robot(studio: crate::state::Studio, id: &str) {
    let id = id.to_owned();
    studio.run_safety("Motors released.", async move {
        let idle = api::send(Method::POST, "/api/mode", Some(&ModeSelection::Idle)).await;
        api::send(
            Method::POST,
            &format!("/api/robots/{id}/torque"),
            Some(&TorqueRequest { enabled: false }),
        )
        .await?;
        idle
    });
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
