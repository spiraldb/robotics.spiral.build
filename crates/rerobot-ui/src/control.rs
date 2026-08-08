//! Driving the arms: picking a mode, watching the pose, moving joints.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{GoalRequest, JointTelemetry, ModeSelection, RobotSummary, RobotTelemetry};
use rerobot_core::arm;

use crate::api;
use crate::armview::ArmView;
use crate::setup::set_torque;
use crate::state::{Studio, studio};

#[component]
pub fn ControlTab() -> Element {
    let studio = studio();
    let robots = studio.snapshot.read().robots.clone();
    let connected = studio.connected_robots();

    rsx! {
        div { class: "flex flex-col gap-6",
            ModePanel { connected: connected.clone() }

            if robots.is_empty() {
                p { class: "card p-8 text-center text-sm text-slate-500 dark:text-slate-400",
                    "Add a robot on the Setup tab to control it here."
                }
            }
            div { class: "grid gap-6 xl:grid-cols-2",
                for robot in robots {
                    RobotPanel { key: "{robot.id}", robot }
                }
            }
        }
    }
}

#[component]
fn ModePanel(connected: Vec<RobotSummary>) -> Element {
    let studio = studio();
    let mode = studio.mode();
    let calibrated = connected
        .iter()
        .filter(|robot| robot.calibrated)
        .cloned()
        .collect::<Vec<_>>();

    let select = move |selection: ModeSelection| {
        studio.run("", async move {
            api::send(Method::POST, "/api/mode", Some(&selection)).await
        });
    };

    // Teleoperation needs a distinct pair, so the pickers default to the first
    // two calibrated arms and never offer the same arm twice.
    //
    // Only the operator's explicit choice is stored; the effective pair is
    // derived here on every render and falls back to the defaults whenever a
    // choice stops naming an available arm. Deriving rather than keeping two
    // signals in step with an effect matters: an effect that both reads and
    // writes a signal retriggers itself, and `Signal::set` marks a signal dirty
    // even when the value is unchanged, so the "keep them in step" version spun
    // forever whenever fewer than two arms were available.
    let mut chosen_leader = use_signal(|| Option::<String>::None);
    let mut chosen_follower = use_signal(|| Option::<String>::None);
    let names = calibrated
        .iter()
        .map(|robot| robot.id.clone())
        .collect::<Vec<_>>();
    let leader = chosen_leader()
        .filter(|name| names.contains(name))
        .or_else(|| names.first().cloned())
        .unwrap_or_default();
    let follower = chosen_follower()
        .filter(|name| names.contains(name) && *name != leader)
        .or_else(|| names.iter().find(|name| **name != leader).cloned())
        .unwrap_or_default();
    let can_teleoperate = !leader.is_empty() && !follower.is_empty();

    rsx! {
        section { class: "card p-4",
            h2 { class: "mb-3 text-base font-semibold", "Mode" }
            div { class: "flex flex-wrap items-end gap-3",
                button {
                    r#type: "button",
                    class: if matches!(mode, ModeSelection::Idle) { "btn btn-primary" } else { "btn btn-ghost" },
                    onclick: move |_| select(ModeSelection::Idle),
                    "Idle"
                }

                div { class: "flex items-end gap-2",
                    div {
                        label { class: "label", "Manual control" }
                        select {
                            class: "field min-w-40",
                            onchange: move |event| {
                                let value = event.value();
                                if !value.is_empty() {
                                    select(ModeSelection::Manual { robot: value });
                                }
                            },
                            option {
                                value: "",
                                selected: !matches!(mode, ModeSelection::Manual { .. }),
                                "Choose an arm…"
                            }
                            for robot in calibrated.clone() {
                                option {
                                    key: "{robot.id}",
                                    value: "{robot.id}",
                                    selected: matches!(&mode, ModeSelection::Manual { robot: chosen } if *chosen == robot.id),
                                    "{robot.id}"
                                }
                            }
                        }
                    }
                }

                div { class: "flex items-end gap-2",
                    div {
                        label { class: "label", "Teleoperation" }
                        div { class: "flex items-center gap-1.5",
                            select {
                                class: "field min-w-32",
                                disabled: !can_teleoperate,
                                onchange: move |event| chosen_leader.set(Some(event.value())),
                                for robot in calibrated.clone() {
                                    option {
                                        key: "{robot.id}",
                                        value: "{robot.id}",
                                        selected: leader == robot.id,
                                        "{robot.id}"
                                    }
                                }
                            }
                            span { class: "text-slate-400", "→" }
                            select {
                                class: "field min-w-32",
                                disabled: !can_teleoperate,
                                onchange: move |event| chosen_follower.set(Some(event.value())),
                                for robot in calibrated.clone().into_iter().filter(|robot| robot.id != leader) {
                                    option {
                                        key: "{robot.id}",
                                        value: "{robot.id}",
                                        selected: follower == robot.id,
                                        "{robot.id}"
                                    }
                                }
                            }
                        }
                    }
                    button {
                        r#type: "button",
                        class: if matches!(mode, ModeSelection::Teleoperate { .. }) { "btn btn-primary" } else { "btn btn-ghost" },
                        disabled: !can_teleoperate,
                        onclick: {
                            let (leader, follower) = (leader.clone(), follower.clone());
                            move |_| select(ModeSelection::Teleoperate {
                                leader: leader.clone(),
                                follower: follower.clone(),
                            })
                        },
                        "Start"
                    }
                }
            }

            p { class: "mt-3 text-sm text-slate-500 dark:text-slate-400",
                match &mode {
                    ModeSelection::Idle =>
                        "Motors are read but never driven. Pick a mode to move an arm.".to_owned(),
                    ModeSelection::Manual { robot } =>
                        format!("Sliders on {robot} command its joints. Enable torque to make it move."),
                    ModeSelection::Teleoperate { leader, follower } =>
                        format!("{follower} follows {leader}. Enable torque on {follower}; leave it off on {leader} so it can be moved by hand."),
                }
            }
            if !can_teleoperate {
                p { class: "mt-1 text-xs text-slate-400",
                    "Teleoperation needs two connected, calibrated arms."
                }
            }
        }
    }
}

#[component]
fn RobotPanel(robot: RobotSummary) -> Element {
    let studio = studio();
    let telemetry = studio.robot_telemetry(&robot.id);
    let torque = telemetry.as_ref().is_some_and(|robot| robot.torque);
    // The arm has to stay free to move by hand during a sweep, so the server
    // refuses to energize it; do not offer a button that cannot work.
    let calibrating = telemetry
        .as_ref()
        .is_some_and(|robot| robot.calibration.is_some());
    let drivable = studio.is_manual_target(&robot.id) && torque && robot.calibrated;
    let id = robot.id.clone();

    rsx! {
        section { class: "card flex flex-col gap-4 p-4",
            header { class: "flex flex-wrap items-center gap-2",
                h3 { class: "text-base font-semibold", "{robot.id}" }
                if telemetry.is_some() {
                    span { class: "chip bg-emerald-100 text-emerald-800 dark:bg-emerald-500/15 dark:text-emerald-300",
                        "Connected"
                    }
                } else {
                    span { class: "chip bg-slate-100 text-slate-500 dark:bg-slate-800 dark:text-slate-400",
                        "Offline"
                    }
                }
                if torque {
                    span { class: "chip bg-rose-100 text-rose-800 dark:bg-rose-500/15 dark:text-rose-300",
                        "Torque on"
                    }
                }
                if calibrating {
                    span { class: "chip bg-sky-100 text-sky-800 dark:bg-sky-500/15 dark:text-sky-300",
                        "Calibrating"
                    }
                }
                button {
                    r#type: "button",
                    class: if torque { "btn btn-danger ml-auto" } else { "btn btn-ghost ml-auto" },
                    disabled: telemetry.is_none() || (calibrating && !torque),
                    title: if calibrating && !torque { "Finish or cancel calibration first" } else { "" },
                    onclick: move |_| set_torque(studio, &id, !torque),
                    {if torque { "Release torque" } else { "Hold position" }}
                }
            }

            if let Some(error) = telemetry.as_ref().and_then(|robot| robot.error.clone()) {
                p { class: "rounded-md bg-rose-50 px-3 py-2 text-sm text-rose-800 dark:bg-rose-500/10 dark:text-rose-300",
                    "{error}"
                }
            }

            ArmView { telemetry: telemetry.clone(), calibrated: robot.calibrated }

            JointControls {
                robot: robot.clone(),
                telemetry: telemetry.clone(),
                drivable,
            }
        }
    }
}

#[component]
fn JointControls(
    robot: RobotSummary,
    telemetry: Option<RobotTelemetry>,
    drivable: bool,
) -> Element {
    let studio = studio();
    // While a slider is being dragged the arm is still catching up, so the
    // commanded value is shown until the measurement agrees with it. That echo
    // is only meaningful for as long as this arm is actually being driven, so
    // it is dropped the moment it is not — otherwise the next drive session
    // would open showing targets from the previous one.
    let mut commanded = use_signal(BTreeMap::<String, f64>::new);
    let watched = robot.id.clone();
    use_effect(move || {
        let driving = studio
            .telemetry
            .read()
            .robots
            .iter()
            .any(|robot| robot.id == watched && robot.torque)
            && studio.is_manual_target(&watched);
        // `peek` deliberately: an effect that subscribes to the signal it
        // writes retriggers itself, and this one only needs to observe
        // telemetry.
        if !driving && !commanded.peek().is_empty() {
            commanded.write().clear();
        }
    });

    let Some(telemetry) = telemetry else {
        return rsx! {
            p { class: "text-sm text-slate-500 dark:text-slate-400",
                "Connect this robot to see its joints."
            }
        };
    };

    let id = robot.id.clone();

    rsx! {
        div { class: "flex flex-col gap-2",
            if !drivable {
                p { class: "text-xs text-slate-400",
                    if !robot.calibrated {
                        "Calibrate this arm to command its joints."
                    } else if !studio.is_manual_target(&robot.id) {
                        "Switch to manual control for this arm to move its joints."
                    } else {
                        "Enable torque to move the joints."
                    }
                }
            }
            for joint in telemetry.joints.clone() {
                JointRow {
                    key: "{joint.name}",
                    joint: joint.clone(),
                    drivable,
                    value: drivable
                        .then(|| commanded.read().get(&joint.name).copied())
                        .flatten(),
                    on_change: {
                        let (id, name) = (id.clone(), joint.name.clone());
                        move |value| command_joint(studio, commanded, id.clone(), name.clone(), value)
                    },
                }
            }
        }
    }
}

/// Command one joint and remember what was asked for.
///
/// The commanded value is kept locally so the slider stays where the operator
/// put it while the arm ramps toward it.
fn command_joint(
    studio: Studio,
    mut commanded: Signal<BTreeMap<String, f64>>,
    id: String,
    joint: String,
    value: f64,
) {
    commanded.write().insert(joint.clone(), value);
    let request = GoalRequest {
        normalized: BTreeMap::from([(joint, value)]),
    };
    studio.run("", async move {
        api::send(
            Method::POST,
            &format!("/api/robots/{id}/goal"),
            Some(&request),
        )
        .await
    });
}

#[component]
fn JointRow(
    joint: JointTelemetry,
    drivable: bool,
    value: Option<f64>,
    on_change: EventHandler<f64>,
) -> Element {
    let label = arm::so101_joint(&joint.name).map_or(joint.name.clone(), |j| j.label.to_owned());
    let measured = joint.normalized.unwrap_or(0.0);
    let shown = value.unwrap_or(measured);

    rsx! {
        div { class: "flex items-center gap-3",
            span { class: "w-28 shrink-0 truncate text-sm", "{label}" }
            input {
                r#type: "range",
                class: "joint-slider",
                "aria-label": "{label} position",
                min: "-1",
                max: "1",
                step: "0.005",
                disabled: !drivable || joint.range.is_none(),
                value: "{shown}",
                oninput: move |event| {
                    if let Ok(value) = event.value().parse::<f64>() {
                        on_change.call(value);
                    }
                },
            }
            span { class: "w-16 shrink-0 text-right font-mono text-xs text-slate-500 dark:text-slate-400",
                match joint.degrees {
                    Some(degrees) => format!("{degrees:+.0}°"),
                    None => format!("{}", joint.raw),
                }
            }
        }
    }
}
