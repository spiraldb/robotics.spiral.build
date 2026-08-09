//! The joints of whichever arm is selected: what they read, and how to move them.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{GoalRequest, JointTelemetry, ModeSelection, RobotSummary, RobotTelemetry};
use rerobot_core::arm::{self, JointAngles};

use crate::api;
use crate::cameras::RobotCameras;
use crate::setup::set_torque;
use crate::state::{Selection, studio};
use crate::viewport::{accent_css, tool_position};

#[derive(Default)]
struct PendingGoals(BTreeMap<String, f64>);

impl PendingGoals {
    fn replace(&mut self, joint: String, value: f64) {
        self.0.insert(joint, value);
    }

    fn take(&mut self) -> BTreeMap<String, f64> {
        std::mem::take(&mut self.0)
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

#[component]
pub fn JointPanel() -> Element {
    let studio = studio();
    let Some(robot) = studio.focused() else {
        return rsx! {
            p { class: "px-2 py-1 text-[11px] text-ink-faint", "Add an arm to see its joints." }
        };
    };

    let telemetry = studio.robot_telemetry(&robot.id);
    let torque = telemetry.as_ref().is_some_and(|robot| robot.torque);
    // The arm has to stay free to move by hand during a sweep, so the server
    // refuses to energize it; do not offer a button that cannot work.
    let calibrating = telemetry
        .as_ref()
        .is_some_and(|robot| robot.calibration.is_some());
    let drivable = studio.is_manual_target(&robot.id) && torque && robot.calibrated;
    let role = studio.teleoperation_role(&robot.id);
    let accent = accent_css(studio.position_of(&robot.id));
    // Releasing is guarded the way deleting a take already is, because it is the
    // one control on this panel with a physical consequence.
    let mut confirming_release = use_signal(|| false);
    let id = robot.id.clone();
    let hold_id = robot.id.clone();
    let manual_id = robot.id.clone();
    let manual = studio.is_manual_target(&robot.id);
    // Taking manual control of one arm cannot happen behind a running pair or a
    // replay, and an uncalibrated arm cannot be commanded at all — the server
    // refuses normalized goals for one, so offering the toggle would only
    // produce a refusal.
    let manual_blocked = if manual {
        None
    } else if telemetry.is_none() {
        Some("Connect this arm first")
    } else if !robot.calibrated {
        Some("Calibrate this arm before driving its joints")
    } else if role.is_some() {
        Some("Stop teleoperation first")
    } else if matches!(studio.mode(), ModeSelection::Replay { .. }) {
        Some("Stop the replay first")
    } else if !studio.controls_available() {
        Some("Controls are paused while telemetry reconnects")
    } else {
        None
    };

    rsx! {
        div { class: "flex flex-col",
            header { class: "flex flex-wrap items-center gap-1 border-b border-line px-2 py-1",
                span {
                    class: "h-1.5 w-1.5 shrink-0 rounded-full",
                    style: "background: {accent}",
                }
                h2 { class: "text-[12px] font-semibold tracking-[-0.01em]", "{robot.id}" }
                if telemetry.is_none() {
                    span { class: "chip", "offline" }
                }
                // What this arm is doing in the running pair, not what it is
                // called: on a bench whose arms are named `leader` and
                // `follower`, a chip repeating the name beside the name is a
                // word printed twice.
                if let Some(role) = role {
                    span {
                        class: "chip",
                        title: if role == "leader" { "Hand-guided; the follower copies it" } else { "Energized and copying the leader" },
                        {if role == "leader" { "leading" } else { "following" }}
                    }
                }
                // Torque is the one state where a wrong assumption moves an
                // arm, so it is the one state on this panel worth colouring.
                if torque {
                    span { class: "chip chip-danger", "torque on" }
                }
                // Releasing drops a powered arm under its own weight, so the
                // consequence is printed on the page at the moment it applies
                // rather than left in a tooltip the operator — who has a hand on
                // the arm and none on the mouse — will never hover.
                if torque && confirming_release() {
                    span { class: "release-confirm ml-auto",
                        span { class: "release-warning",
                            {match role {
                                Some("follower") => "Support the arm. This also stops it following the leader.",
                                _ => "Support the arm — it goes limp when released.",
                            }}
                        }
                        button {
                            r#type: "button",
                            class: "btn btn-danger",
                            onclick: move |_| {
                                confirming_release.set(false);
                                set_torque(studio, &id, false);
                            },
                            "Release"
                        }
                        button {
                            r#type: "button",
                            class: "btn",
                            onclick: move |_| confirming_release.set(false),
                            "Keep holding"
                        }
                    }
                } else {
                    // Which arm the sliders drive is a property of this panel,
                    // not of the header: it names one arm, and this is the one
                    // arm's panel. It used to be a picker in the mode bar, so
                    // moving a joint meant setting a target in one place and
                    // energizing it in another.
                    button {
                        r#type: "button",
                        class: "btn btn-outline ml-auto",
                        "aria-pressed": manual,
                        disabled: manual_blocked.is_some(),
                        title: match (manual_blocked, manual) {
                            (Some(reason), _) => reason,
                            (None, true) => "Stop driving this arm from the sliders",
                            (None, false) => "Drive this arm's joints from the sliders below",
                        },
                        onclick: move |_| crate::modes::set_mode(
                            studio,
                            if manual { ModeSelection::Idle } else { ModeSelection::Manual { robot: manual_id.clone() } },
                        ),
                        "Manual"
                    }
                    button {
                        r#type: "button",
                        class: if torque { "btn btn-danger" } else { "btn btn-outline" },
                        disabled: telemetry.is_none() || (calibrating && !torque),
                        title: match (calibrating && !torque, torque) {
                            (true, _) => "Finish or cancel calibration first",
                            (_, true) => "Let this arm go limp",
                            _ => "Energize the motors so the arm holds its position",
                        },
                        onclick: move |_| {
                            if torque {
                                confirming_release.set(true);
                            } else {
                                // A stale confirmation from a previous hold
                                // must not greet the next one.
                                confirming_release.set(false);
                                set_torque(studio, &hold_id, true);
                            }
                        },
                        {if torque { "Release" } else { "Hold" }}
                    }
                }
            }

            if let Some(error) = telemetry.as_ref().and_then(|robot| robot.error.clone()) {
                p { class: "mx-2 mt-1 rounded bg-danger-soft px-1.5 py-1 text-[11px] text-danger",
                    "{error}"
                }
            }

            ToolReadout {
                telemetry: telemetry.clone(),
                calibrated: robot.calibrated,
            }

            JointControls {
                robot: robot.clone(),
                telemetry: telemetry.clone(),
                drivable,
            }

            RobotCameras { robot }

        }
    }
}

/// Where the arm is holding its gripper, solved from the robot description.
#[component]
fn ToolReadout(telemetry: Option<RobotTelemetry>, calibrated: bool) -> Element {
    let description = crate::state::description();
    let Ok(urdf) = description.as_ref() else {
        return rsx! {};
    };
    let angles = telemetry
        .as_ref()
        .filter(|_| calibrated)
        .map(|robot| measured_angles(&robot.joints));
    let tip = angles
        .as_ref()
        .and_then(|angles| tool_position(urdf, angles));
    let tracking_error = telemetry.as_ref().and_then(|robot| {
        robot
            .joints
            .iter()
            .filter_map(|joint| Some((joint.degrees? - joint.target_degrees?).abs()))
            .filter(|error| error.is_finite())
            .reduce(f64::max)
    });
    let lagging = tracking_error
        .is_some_and(|error| error >= rerobot_core::api::CAUTION_FOLLOW_ERROR_DEGREES);

    rsx! {
        dl { class: "grid grid-cols-4 gap-1 px-2 py-1",
            Readout {
                label: "Reach",
                value: tip.map(|tip| format!("{:.0}", tip.truncate().length())),
                unit: "mm",
            }
            Readout {
                label: "Height",
                value: tip.map(|tip| format!("{:.0}", tip.z)),
                unit: "mm",
            }
            // A bare percentage does not say which end is which, and reading it
            // backwards is exactly the mistake this readout used to invite — so
            // "open" moved into the label, where it names the direction without
            // wrapping the value onto a second line and knocking the four cells
            // out of alignment with each other.
            Readout {
                label: "Gripper open",
                value: angles.map(|angles| format!("{:.0}", angles.gripper * 100.0)),
                unit: "%",
            }
            // One name for one number, in every mode. This is the same quantity
            // the session timeline records for a whole take; calling it
            // `Target Δ` here and `Follow Δ` there would make two readings of
            // the same measurement look like two measurements.
            //
            // Going out of range tints the cell and nothing else. It used to
            // also unfold a two-line definition between the readouts and the
            // sliders, which meant the whole joint stack jumped down a row and
            // back every time a joint caught up with its target.
            Readout {
                label: "Follow Δ",
                // One decimal, matching the session timeline: the same number
                // rounded two different ways reads as two measurements.
                value: tracking_error.map(|error| format!("{error:.1}")),
                unit: "°",
                caution: lagging,
                hint: "The largest gap between where a joint is and where it was told to be",
            }
        }
    }
}

/// One measured quantity: its name, its number, and the unit the number is in.
///
/// The unit is a separate faint glyph rather than part of the string, so four
/// of these side by side line their digits up instead of each starting wherever
/// its own label happened to end. The digits also hold a fixed width: these
/// numbers change twenty times a second, and a unit that slides left every time
/// a hundreds digit drops is the whole strip shimmering.
#[component]
fn Readout(
    label: String,
    value: Option<String>,
    unit: &'static str,
    #[props(default)] caution: bool,
    #[props(default)] hint: &'static str,
) -> Element {
    let known = value.is_some();
    rsx! {
        div {
            class: if caution { "readout readout-caution" } else { "readout" },
            title: "{hint}",
            dt { class: "readout-label", "{label}" }
            dd { class: "readout-value",
                span { class: "readout-number", {value.unwrap_or_else(|| "—".to_owned())} }
                if known { span { class: "readout-unit", "{unit}" } }
            }
        }
    }
}

/// Collect an arm's calibrated joint angles out of its telemetry.
pub fn measured_angles(joints: &[JointTelemetry]) -> JointAngles {
    let degrees = joints
        .iter()
        .filter_map(|joint| Some((joint.name.clone(), joint.degrees?)))
        .collect::<BTreeMap<_, _>>();
    let gripper = joints
        .iter()
        .find(|joint| joint.name == "gripper")
        .and_then(|joint| joint.normalized)
        .unwrap_or(0.0);
    JointAngles::from_named_degrees(&degrees).with_gripper_normalized(gripper)
}

#[component]
fn JointControls(
    robot: RobotSummary,
    telemetry: Option<RobotTelemetry>,
    drivable: bool,
) -> Element {
    let mut studio = studio();
    // While a slider is being dragged the arm is still catching up, so the
    // commanded value is shown until the measurement agrees with it. That echo
    // is only meaningful for as long as this arm is actually being driven, so
    // it is dropped the moment it is not — otherwise the next drive session
    // would open showing targets from the previous one.
    let mut commanded = use_signal(BTreeMap::<String, f64>::new);
    // One sender owns this robot's goal endpoint. Slider input only replaces
    // the latest value in this map; the sender drains every changed joint into
    // one ordered request at most once per render frame. Requests can never
    // overtake one another, and the final value is always sent after an
    // in-flight request completes.
    let pending = use_hook(|| Rc::new(RefCell::new(PendingGoals::default())));
    let command_robot = robot.id.clone();
    use_future({
        let pending = pending.clone();
        move || {
            let pending = pending.clone();
            let command_robot = command_robot.clone();
            async move {
                loop {
                    gloo_timers::future::TimeoutFuture::new(32).await;
                    let may_drive = studio.controls_available()
                        && studio.is_manual_target(&command_robot)
                        && studio
                            .robot_telemetry(&command_robot)
                            .is_some_and(|robot| robot.torque);
                    if !may_drive {
                        pending.borrow_mut().clear();
                        continue;
                    }
                    let normalized = pending.borrow_mut().take();
                    if normalized.is_empty() {
                        continue;
                    }
                    let request = GoalRequest { normalized };
                    if let Err(error) = api::send(
                        Method::POST,
                        &format!("/api/robots/{command_robot}/goal"),
                        Some(&request),
                    )
                    .await
                    {
                        studio.report(error);
                    }
                }
            }
        }
    });
    let watched = robot.id.clone();
    let clear_pending = pending.clone();
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
        if !driving {
            clear_pending.borrow_mut().clear();
        }
    });

    let Some(telemetry) = telemetry else {
        return rsx! {
            p { class: "px-2 py-1 text-[11px] text-ink-faint", "Connect this arm to see its joints." }
        };
    };

    let id = robot.id.clone();

    rsx! {
        div { class: "flex flex-col px-2 pb-1.5",
            // The one blocker this panel cannot answer for itself: an
            // uncalibrated arm refuses every command, and the control that fixes
            // it lives in the robot explorer. "Press Manual", "press Hold" and
            // "stop the teleoperation you just started" were sentences pointing
            // at buttons forty pixels above them — and each one appeared and
            // vanished under the sliders as the mode changed.
            if !robot.calibrated {
                p { class: "joint-blocked",
                    "Calibrate this arm in the robot explorer to command its joints."
                }
            }
            for joint in telemetry.joints.clone() {
                JointRow {
                    key: "{joint.name}",
                    joint: joint.clone(),
                    drivable,
                    selected: studio
                        .selected
                        .read()
                        .as_ref()
                        .is_some_and(|selection| selection.matches_joint(&robot.id, &joint.name)),
                    value: drivable
                        .then(|| commanded.read().get(&joint.name).copied())
                        .flatten(),
                    on_change: {
                        let name = joint.name.clone();
                        let pending = pending.clone();
                        move |value| command_joint(commanded, pending.clone(), name.clone(), value)
                    },
                    on_select: {
                        let (robot_id, joint_name) = (id.clone(), joint.name.clone());
                        move |_| studio.selected.set(Some(Selection::Joint {
                            robot: robot_id.clone(),
                            joint: joint_name.clone(),
                        }))
                    }
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
    mut commanded: Signal<BTreeMap<String, f64>>,
    pending: Rc<RefCell<PendingGoals>>,
    joint: String,
    value: f64,
) {
    commanded.write().insert(joint.clone(), value);
    pending.borrow_mut().replace(joint, value);
}

#[cfg(test)]
mod tests {
    use super::PendingGoals;

    #[test]
    fn pending_goals_keep_only_the_latest_value_per_joint() {
        let mut pending = PendingGoals::default();
        pending.replace("elbow".into(), -0.4);
        pending.replace("wrist".into(), 0.2);
        pending.replace("elbow".into(), 0.7);

        let batch = pending.take();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch["elbow"], 0.7);
        assert_eq!(batch["wrist"], 0.2);
        assert!(pending.take().is_empty());
    }
}

#[component]
fn JointRow(
    joint: JointTelemetry,
    drivable: bool,
    selected: bool,
    value: Option<f64>,
    on_change: EventHandler<f64>,
    on_select: EventHandler<()>,
) -> Element {
    let label = arm::so101_joint(&joint.name).map_or(joint.name.clone(), |j| j.label.to_owned());
    let measured = joint.normalized.unwrap_or(0.0);
    let shown = value.unwrap_or(measured);

    rsx! {
        div { class: if selected { "joint-row joint-row-selected" } else { "joint-row" },
            button {
                r#type: "button",
                class: "joint-label",
                title: "Select {label}",
                onclick: move |_| on_select.call(()),
                "{label}"
            }
            input {
                r#type: "range",
                class: "joint-slider",
                "aria-label": "{label} position",
                min: "-1",
                max: "1",
                step: "0.005",
                disabled: !drivable || joint.range.is_none(),
                value: "{shown}",
                title: match joint.target_degrees {
                    Some(target) => format!("Measured joint angle; current target {target:+.0}°"),
                    None => "Measured joint angle".to_owned(),
                },
                oninput: move |event| {
                    if let Ok(value) = event.value().parse::<f64>() {
                        on_change.call(value);
                    }
                },
            }
            output { class: "joint-value",
                match joint.degrees {
                    Some(degrees) => format!("{degrees:+.0}°"),
                    None => format!("{}", joint.raw),
                }
            }
        }
    }
}
