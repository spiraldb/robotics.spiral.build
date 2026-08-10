//! Live operation workbench: capture, control, and the selected arm's state.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{ModeSelection, SetRecordingPausedRequest, default_teleoperation_pair};

use crate::api;
use crate::control::JointPanel;
use crate::modes::set_mode;
use crate::setup::set_torque;
use crate::state::{Selection, studio};

/// The complete left-hand workbench for operating a connected pair.
#[component]
pub fn LiveWorkbench() -> Element {
    let studio = studio();
    let robots = studio.snapshot.read().robots.clone();
    let recording = studio.recording();

    rsx! {
        div { class: "studio-workbench-scroll live-workbench",
            RecordingSection { recording }
            ControlSection {}

            section { class: "workbench-section live-properties",
                header { class: "workbench-section-heading",
                    h3 { "Motors & cameras" }
                }
                RobotRoster { robots }
                JointPanel {}
            }
        }
    }
}

#[component]
fn RobotRoster(robots: Vec<rerobot_core::api::RobotSummary>) -> Element {
    let mut studio = studio();
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
                            onclick: move |_| studio.selected.set(Some(Selection::Robot(id.clone()))),
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
fn RecordingSection(recording: Option<rerobot_core::api::RecordingProgress>) -> Element {
    let studio = studio();
    let can_record = studio.controls_available()
        && default_teleoperation_pair(&studio.snapshot.read().robots).is_some()
        && !matches!(studio.mode(), ModeSelection::Replay { .. });
    let paused = recording.as_ref().is_some_and(|progress| progress.paused);

    rsx! {
        section { class: "workbench-section recording-section",
            header { class: "workbench-section-heading",
                h3 { "Recording" }
                if paused {
                    span { class: "capture-state", "Paused · same episode" }
                } else if recording.is_some() {
                    span { class: "capture-state capture-state-live",
                        span { class: "capture-dot" }
                        "Teleop recording"
                    }
                }
            }

            if let Some(progress) = recording {
                div { class: "capture-readout",
                    div {
                        strong { "{progress.episode}" }
                        span {
                            if progress.paused_seconds > 0.0 {
                                "{progress.frames} samples · {format_elapsed(progress.paused_seconds)} omitted"
                            } else {
                                "{progress.frames} samples"
                            }
                        }
                    }
                    time {
                        role: "timer",
                        title: "Active captured time; deliberate pauses are excluded",
                        "{format_elapsed(progress.elapsed_seconds)} active"
                    }
                }
                div { class: "capture-actions capture-actions-active",
                    button {
                        r#type: "button", class: "studio-button",
                        disabled: progress.paused && !studio.controls_available(),
                        onclick: move |_| {
                            set_recording_paused(studio, !progress.paused);
                        },
                        if progress.paused { "Resume" } else { "Pause" }
                    }
                    button {
                        r#type: "button", class: "studio-button",
                        onclick: move |_| crate::timeline::stop_recording(studio, true),
                        "Stop"
                    }
                    button {
                        r#type: "button", class: "studio-button studio-button-danger-quiet",
                        onclick: move |_| crate::timeline::stop_recording(studio, false),
                        "Delete"
                    }
                }
            } else {
                button {
                    r#type: "button",
                    class: "record-start-button",
                    disabled: !can_record,
                    title: if can_record { "Start leader-to-follower teleoperation and record" } else { "Connect and calibrate a compatible leader and follower first" },
                    onclick: move |_| crate::timeline::start_recording(studio),
                    span { class: "capture-dot" }
                    "Record episode"
                }
            }
        }
    }
}

fn set_recording_paused(studio: crate::state::Studio, paused: bool) {
    let request = SetRecordingPausedRequest { paused };
    let future =
        async move { api::send(Method::POST, "/api/episodes/record/pause", Some(&request)).await };
    if paused {
        // Pausing moves the system toward less capture, so it remains usable
        // when browser telemetry is stale but the HTTP command path is alive.
        studio.run_safety("Recording paused. The episode remains open.", future);
    } else {
        studio.run("Recording resumed in the same episode.", future);
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
                    class: if holding && !teleop { "control-segment control-segment-active" } else { "control-segment" },
                    disabled: focused.is_none() || telemetry.is_none(),
                    onclick: {
                        let id = focused.as_ref().map(|robot| robot.id.clone());
                        move |_| if let Some(id) = id.as_deref() { set_torque(studio, id, true); }
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
                    class: if teleop { "control-segment control-segment-active" } else { "control-segment" },
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
                                if let Some(id) = id.as_deref() { set_torque(studio, id, false); }
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

fn format_elapsed(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor() as u64;
    let remainder = seconds - minutes as f64 * 60.0;
    format!("{minutes:02}:{remainder:04.1}")
}
