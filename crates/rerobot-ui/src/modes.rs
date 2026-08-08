//! Choosing what the studio is doing with the arms.
//!
//! The mode is the single most consequential thing on screen — it decides
//! whether motors move at all — so it lives in the header, visible from every
//! panel rather than behind a tab.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::ModeSelection;

use crate::api;
use crate::state::{Studio, studio};

fn set_mode(studio: Studio, selection: ModeSelection) {
    let request = selection.clone();
    let future = async move { api::send(Method::POST, "/api/mode", Some(&request)).await };
    if matches!(selection, ModeSelection::Idle) {
        studio.run_safety("Motion commands stopped. Motors may still be holding position.", future);
    } else {
        studio.run("", future);
    }
}

#[component]
pub fn ModeBar() -> Element {
    let studio = studio();
    let mode = studio.mode();
    let status = studio.snapshot.read().mode.clone();
    // Only a calibrated arm can be commanded, so only calibrated arms are
    // offered: an option that would be refused is worse than no option.
    let ready = studio
        .connected_robots()
        .into_iter()
        .filter(|robot| robot.calibrated)
        .collect::<Vec<_>>();
    let arms = ready
        .iter()
        .map(|robot| robot.id.clone())
        .collect::<Vec<_>>();

    // Teleoperation needs a distinct pair, and each arm's file already says
    // which end of it that arm was built to be, so the pickers open on the
    // obvious answer: the leader leads.
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
    let built_for = |role: &str, exclude: &str| {
        ready
            .iter()
            .find(|robot| {
                robot.id != exclude
                    && robot
                        .robot_type
                        .as_deref()
                        .is_some_and(|kind| kind.contains(role))
            })
            .map(|robot| robot.id.clone())
    };
    // A running mode outranks the pickers. Otherwise changing one mid-run
    // would leave the header describing a pair that is not the pair the server
    // is actually mirroring, which is worse than not being able to change it.
    let configured_pair = match &mode {
        ModeSelection::Teleoperate { leader, follower } => Some((leader.clone(), follower.clone())),
        _ => None,
    };
    let teleoperating = status.active && configured_pair.is_some();
    let pair_locked = teleoperating;
    let (leader, follower) = configured_pair.unwrap_or_else(|| {
        let leader = chosen_leader()
            .filter(|name| arms.contains(name))
            .or_else(|| built_for("leader", ""))
            .or_else(|| arms.first().cloned())
            .unwrap_or_default();
        let follower = chosen_follower()
            .filter(|name| arms.contains(name) && *name != leader)
            .or_else(|| built_for("follower", &leader))
            .or_else(|| arms.iter().find(|name| **name != leader).cloned())
            .unwrap_or_default();
        (leader, follower)
    });
    let can_teleoperate = !leader.is_empty() && !follower.is_empty();
    let controls_paused = !studio.controls_available();
    let has_fault = status.error.is_some();

    rsx! {
        div { class: "mode-bar", role: "group", "aria-label": "Operating mode",
            button {
                r#type: "button",
                class: if matches!(mode, ModeSelection::Idle) { "btn mode-button-active" } else { "btn btn-danger" },
                "aria-pressed": matches!(mode, ModeSelection::Idle),
                title: "Stop sending motion commands. This is not a safety-rated emergency stop; motors may keep holding position.",
                onclick: move |_| set_mode(studio, ModeSelection::Idle),
                {if matches!(mode, ModeSelection::Idle) { "Commands stopped" } else { "Stop motion" }}
            }

            div {
                class: if matches!(mode, ModeSelection::Manual { .. }) { "mode-group mode-group-active" } else { "mode-group" },
                // Teleoperation is a running process with an arm energized at
                // the end of it. Nudging a dropdown should not end it.
                title: if pair_locked { "Stop teleoperation first" } else if controls_paused { "Controls are paused while reconnecting" } else { "Drive one arm's joints from the sliders" },
                span { class: "text-[11px] text-ink-faint", "Manual" }
                select {
                    class: "field-inline",
                    "aria-label": "Arm to control by hand",
                    disabled: pair_locked || controls_paused,
                    onchange: move |event| {
                        let value = event.value();
                        set_mode(studio, if value.is_empty() {
                            ModeSelection::Idle
                        } else {
                            ModeSelection::Manual { robot: value }
                        });
                    },
                    option {
                        value: "",
                        selected: !matches!(mode, ModeSelection::Manual { .. }),
                        "off"
                    }
                    for arm in arms.clone() {
                        option {
                            key: "{arm}",
                            value: "{arm}",
                            selected: matches!(&mode, ModeSelection::Manual { robot } if *robot == arm),
                            "{arm}"
                        }
                    }
                }
            }

            div {
                class: if teleoperating { "mode-group mode-group-active" } else if has_fault { "mode-group mode-group-fault" } else { "mode-group" },
                title: match (teleoperating, can_teleoperate) {
                    (true, _) => "Stop teleoperation to change the pair",
                    (_, false) => "Teleoperation needs two connected, calibrated arms",
                    _ => "Mirror a hand-guided arm onto a driven one",
                },
                span { class: "text-[11px] text-ink-faint", "Teleop" }
                select {
                    class: "field-inline",
                    "aria-label": "Leader arm",
                    disabled: !can_teleoperate || pair_locked || controls_paused,
                    onchange: move |event| chosen_leader.set(Some(event.value())),
                    for arm in arms.clone() {
                        option { key: "{arm}", value: "{arm}", selected: leader == arm, "{arm}" }
                    }
                }
                span { class: "text-ink-faint", "→" }
                select {
                    class: "field-inline",
                    "aria-label": "Follower arm",
                    disabled: !can_teleoperate || pair_locked || controls_paused,
                    onchange: move |event| chosen_follower.set(Some(event.value())),
                    for arm in arms.clone().into_iter().filter(|arm| *arm != leader) {
                        option { key: "{arm}", value: "{arm}", selected: follower == arm, "{arm}" }
                    }
                }
                button {
                    r#type: "button",
                    class: if teleoperating { "btn btn-danger" } else { "btn btn-primary" },
                    disabled: (!can_teleoperate && !teleoperating) || (controls_paused && !teleoperating),
                    // Starting energizes an arm, so the button says so before
                    // it is pressed rather than after.
                    title: match teleoperating {
                        true => "Stop mirroring. The follower keeps holding its position.",
                        false => "Releases the leader so it can be moved by hand, and holds the follower",
                    },
                    onclick: {
                        let (leader, follower) = (leader.clone(), follower.clone());
                        move |_| {
                            if teleoperating {
                                set_mode(studio, ModeSelection::Idle);
                            } else {
                                set_mode(studio, ModeSelection::Teleoperate {
                                    leader: leader.clone(),
                                    follower: follower.clone(),
                                })
                            }
                        }
                    },
                    {if teleoperating { "Stop teleoperation" } else if has_fault { "Retry teleoperation" } else { "Start teleoperation" }}
                }
                if !teleoperating {
                    span { class: "teleop-consequence", "releases leader · energizes follower" }
                }
            }

            if let Some(error) = &status.error {
                span { class: "mode-description mode-description-fault", "Faulted — {error}" }
            } else {
                span { class: "mode-description",
                    match &mode {
                        ModeSelection::Idle => "No motion commands. Motor holding state is shown at right.".to_owned(),
                        ModeSelection::Manual { robot } =>
                            if status.active { format!("Manual target: {robot}. Enable Hold to energize it.") } else { format!("Manual target {robot} is configured but inactive.") },
                        ModeSelection::Teleoperate { leader, follower } =>
                            if status.active { format!("{leader} is released; {follower} is energized and copying it.") } else { format!("{leader} → {follower} is configured but not active.") },
                    }
                }
            }
        }
    }
}
