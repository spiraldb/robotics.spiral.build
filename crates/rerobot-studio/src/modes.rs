//! Choosing what the studio is doing with the arms.
//!
//! The mode is the single most consequential thing on screen — it decides
//! whether motors move at all — so it lives in the header, visible from every
//! panel rather than behind a tab.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{ModeSelection, ReplayProgress};

use crate::api;
use crate::state::{Studio, studio};

pub fn set_mode(studio: Studio, selection: ModeSelection) {
    let request = selection.clone();
    let future = async move { api::send(Method::POST, "/api/mode", Some(&request)).await };
    if matches!(selection, ModeSelection::Idle) {
        studio.run_safety(
            "Motion commands stopped. Motors may still be holding position.",
            future,
        );
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
    // A replay is a running mode with an energized arm tracking a trajectory.
    // Nudging a picker should not end it, for exactly the reason teleoperation
    // locks these too.
    let pair_locked = teleoperating || matches!(mode, ModeSelection::Replay { .. });
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
    let replay = studio.replay();
    let replaying = matches!(mode, ModeSelection::Replay { .. });
    // Stopping commands does not release motors, so the narration needs to know
    // whether anything is still energized before it calls the studio quiet.
    let holding = studio
        .telemetry
        .read()
        .robots
        .iter()
        .filter(|robot| robot.torque)
        .count();

    rsx! {
        // Teleoperation is the only mode the header configures, because it is
        // the only one that is about a *pair* of arms and so belongs to no
        // single panel. Manual control is one arm's business and now lives on
        // that arm's joint panel, beside the sliders it enables; a standing
        // "Commands stopped" button went with it, since every mode that can
        // actually be running carries its own stop — teleoperation here, replay
        // on the session rail.
        div { class: "mode-bar", role: "group", "aria-label": "Operating mode",
            div {
                class: if teleoperating { "mode-group mode-group-active" } else if has_fault { "mode-group mode-group-fault" } else { "mode-group" },
                title: match (teleoperating, replaying, can_teleoperate) {
                    (true, _, _) => "Stop teleoperation to change the pair",
                    (_, true, _) => "Stop the replay first",
                    (_, _, false) => "Teleoperation needs two connected, calibrated arms",
                    _ => "Mirror a hand-guided arm onto a driven one",
                },
                span { class: "mode-group-name", "Teleop" }
                select {
                    class: "field-inline",
                    "aria-label": "Leader arm",
                    disabled: !can_teleoperate || pair_locked || controls_paused,
                    onchange: move |event| chosen_leader.set(Some(event.value())),
                    for arm in arms.clone() {
                        option { key: "{arm}", value: "{arm}", selected: leader == arm, "{arm}" }
                    }
                }
                span { class: "mode-group-name", "→" }
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
                span {
                    class: "mode-description mode-description-fault",
                    role: "status",
                    "Faulted — {error}"
                }
            } else if let Some(narration) = narrate(&mode, status.active, holding, &replay) {
                // The narration takes a line of its own rather than competing
                // with the controls for whatever space is left over: it is the
                // only place a replay reports its progress, and the only place
                // a studio that has stopped commanding still-energized arms
                // says so.
                span { class: "mode-description", role: "status", "{narration}" }
            }
        }
    }
}

/// What the studio is doing, when that is worth a sentence.
///
/// An idle studio with every motor released is already described by the mode
/// controls and the status chips, so it says nothing: a permanent line
/// restating the resting state is what teaches an operator to stop reading it.
/// Everything else is news — including a studio that has stopped commanding
/// arms which are still holding position, which is the one case where the
/// resting state is not the safe one.
fn narrate(
    mode: &ModeSelection,
    active: bool,
    holding: usize,
    replay: &Option<ReplayProgress>,
) -> Option<String> {
    match mode {
        ModeSelection::Idle if holding == 0 => None,
        ModeSelection::Idle => Some(format!(
            "No motion commands, but {} still holding position. Stopping commands is not a safety-rated stop.",
            match holding {
                1 => "one arm is".to_owned(),
                many => format!("{many} arms are"),
            },
        )),
        ModeSelection::Manual { robot } => Some(if active {
            format!("Manual target: {robot}. Enable Hold to energize it.")
        } else {
            format!("Manual target {robot} is configured but inactive.")
        }),
        ModeSelection::Teleoperate { leader, follower } => Some(if active {
            format!("{leader} is released; {follower} is energized and copying it.")
        } else {
            format!("{leader} → {follower} is configured but not active.")
        }),
        // Replay is started from the session timeline, not from here, but it is
        // a mode like any other and the header is where the studio says what it
        // is doing.
        ModeSelection::Replay { robot, episode } => Some(match replay {
            Some(progress) if progress.frame == 0 => {
                format!("{robot} is moving to the start of {episode}.")
            }
            Some(progress) => format!(
                "{robot} is replaying {episode} — frame {} of {}.",
                progress.frame, progress.frames,
            ),
            None if active => format!("{robot} is replaying {episode}."),
            None => format!("Replay of {episode} on {robot} has stopped."),
        }),
    }
}
