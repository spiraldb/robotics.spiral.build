//! The joints of whichever arm is selected: what they read, and how to move them.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{
    GoalRequest, JointTelemetry, MotorCondition, MotorLimits, RobotSummary, RobotTelemetry,
};
use rerobot_core::arm::{self, JointAngles};

use crate::api;
use crate::state::{Selection, studio};
use crate::viewport::tool_position;

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
pub fn JointPanel(
    on_select: EventHandler<Option<Selection>>,
    on_point: EventHandler<Option<Selection>>,
) -> Element {
    let studio = studio();
    let Some(robot) = studio.focused() else {
        return rsx! {
            p { class: "px-2 py-1 text-[11px] text-ink-faint", "Add an arm to see its joints." }
        };
    };

    let telemetry = studio.robot_telemetry(&robot.id);
    let torque = telemetry.as_ref().is_some_and(|robot| robot.torque);
    let drivable = studio.is_manual_target(&robot.id) && torque && robot.calibrated;

    rsx! {
        div { class: "flex flex-col",
            if let Some(error) = telemetry.as_ref().and_then(|robot| robot.error.clone()) {
                p { class: "mx-2 mt-1 rounded bg-danger-soft px-1.5 py-1 text-[11px] text-danger",
                    "{error}"
                }
            }

            ToolReadout {
                telemetry: telemetry.clone(),
                calibrated: robot.calibrated,
            }

            ConditionReadout {
                robot: robot.clone(),
                telemetry: telemetry.clone(),
            }

            JointControls {
                robot: robot.clone(),
                telemetry: telemetry.clone(),
                drivable,
                on_select,
                on_point,
            }

            MotorDetail {
                robot: robot.clone(),
                telemetry: telemetry.clone(),
            }

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

/// The limits the named motor reported for itself when it connected.
fn motor_limits(robot: &RobotSummary, joint: &str) -> MotorLimits {
    robot
        .motors
        .iter()
        .find(|motor| motor.name == joint)
        .map(|motor| motor.limits)
        .unwrap_or_default()
}

/// How the whole arm is doing against the limits its own motors reported.
///
/// Four cells, two of them thermal, because heat is the one reading that does
/// not recover the moment the load comes off. `Hottest` alone cannot be acted
/// on — fifty-eight degrees is either fine or four minutes from a shutdown
/// depending on which way it is going — so the cell beside it carries the
/// direction. Neither cell names which motor is hot: that does not fit in a
/// readout, and the joint row a few pixels below tints itself instead.
#[component]
fn ConditionReadout(robot: RobotSummary, telemetry: Option<RobotTelemetry>) -> Element {
    let joints = telemetry.map(|robot| robot.joints).unwrap_or_default();

    // The hottest motor decides both thermal cells, so they always describe the
    // same motor as each other.
    let hottest = joints
        .iter()
        .filter(|joint| joint.temperature_celsius.is_some())
        .max_by(|left, right| {
            left.temperature_celsius
                .unwrap_or_default()
                .total_cmp(&right.temperature_celsius.unwrap_or_default())
        });
    let hot = hottest.is_some_and(|joint| {
        joint.condition(&motor_limits(&robot, &joint.name)) != MotorCondition::Nominal
    });

    // Every motor shares one supply, so the interesting number is the worst of
    // them: the motor seeing the deepest sag is the one about to complain.
    let supply = joints
        .iter()
        .filter_map(|joint| joint.voltage_volts)
        .reduce(f64::min);
    let sagging = joints.iter().any(|joint| {
        joint.voltage_volts.is_some()
            && joint.voltage_volts == supply
            && joint.condition(&motor_limits(&robot, &joint.name)) != MotorCondition::Nominal
    });

    let hardest = joints
        .iter()
        .filter(|joint| joint.load.is_some())
        .max_by(|left, right| {
            left.load
                .unwrap_or_default()
                .abs()
                .total_cmp(&right.load.unwrap_or_default().abs())
        });
    let strained = hardest.is_some_and(|joint| {
        joint.condition(&motor_limits(&robot, &joint.name)) != MotorCondition::Nominal
    });

    rsx! {
        dl { class: "grid grid-cols-4 gap-1 px-2 pb-1",
            Readout {
                label: "Hottest",
                value: hottest
                    .and_then(|joint| joint.temperature_celsius)
                    .map(|celsius| format!("{celsius:.0}")),
                unit: "°C",
                caution: hot,
                hint: "The warmest motor on this arm",
            }
            // No caution tint: a rise has no limit to be near, and this studio
            // does not invent thresholds the hardware did not state.
            Readout {
                label: "Rise",
                value: hottest
                    .and_then(|joint| joint.temperature_rise_per_minute)
                    .map(|rate| format!("{rate:+.0}")),
                unit: "°C/m",
                hint: "How fast the warmest motor is heating, measured over the last minute",
            }
            Readout {
                label: "Supply",
                value: supply.map(|volts| format!("{volts:.1}")),
                unit: "V",
                caution: sagging,
                hint: "The lowest supply voltage any motor on this bus reports",
            }
            Readout {
                label: "Load",
                value: hardest
                    .and_then(|joint| joint.load)
                    .map(|load| format!("{:.0}", load.abs() * 100.0)),
                unit: "%",
                caution: strained,
                hint: "The hardest any motor is working, as a share of its rated torque",
            }
        }
    }
}

/// Everything one motor reports, for the motor the operator picked.
///
/// This appears on selection rather than living on the panel permanently: six
/// more numbers for every joint would be a diagnostic console, and the arm-level
/// row above already answers whether anything is wrong. Selection is a
/// deliberate act rather than a live value, so opening this does not put the
/// layout at the mercy of the telemetry stream.
#[component]
fn MotorDetail(robot: RobotSummary, telemetry: Option<RobotTelemetry>) -> Element {
    let studio = studio();
    let selected = studio.selected.read().clone();
    let Some(name) = selected.and_then(|selection| match selection {
        Selection::Joint {
            robot: selected,
            joint,
        } if selected == robot.id => Some(joint),
        _ => None,
    }) else {
        return rsx! {};
    };
    let Some(joint) = telemetry.and_then(|telemetry| {
        telemetry
            .joints
            .into_iter()
            .find(|joint| joint.name == name)
    }) else {
        return rsx! {};
    };

    let label = arm::so101_joint(&joint.name).map_or(joint.name.clone(), |j| j.label.to_owned());
    let model = robot
        .motors
        .iter()
        .find(|motor| motor.name == joint.name)
        .and_then(|motor| motor.model.clone());
    let limits = motor_limits(&robot, &joint.name);
    let condition = joint.condition(&limits);

    rsx! {
        section { class: "motor-detail",
            header { class: "motor-detail-head",
                h3 { class: "motor-detail-name", "{label}" }
                // One line whatever happens: identity when the motor is well,
                // its own complaint when it is not. A line that appears only on
                // a fault would push the readouts below it down a row at the
                // exact moment they matter most.
                if joint.faults.is_empty() {
                    span { class: "motor-detail-id",
                        "id {joint.id}"
                        if let Some(model) = model {
                            " · {model}"
                        }
                    }
                } else {
                    span { class: "motor-detail-fault",
                        {joint.faults.iter().map(|fault| fault.label()).collect::<Vec<_>>().join(" · ")}
                    }
                }
            }
            dl { class: "grid grid-cols-3 gap-1",
                Readout {
                    label: "Temp",
                    value: joint.temperature_celsius.map(|celsius| format!("{celsius:.0}")),
                    unit: "°C",
                    caution: condition != MotorCondition::Nominal,
                    hint: match limits.max_temperature_celsius {
                        Some(limit) => format!("This motor faults at {limit:.0} °C"),
                        None => "This motor did not report a temperature limit".to_owned(),
                    },
                }
                Readout {
                    label: "Rise",
                    value: joint.temperature_rise_per_minute.map(|rate| format!("{rate:+.0}")),
                    unit: "°C/m",
                    hint: "Measured over the last minute",
                }
                Readout {
                    label: "Supply",
                    value: joint.voltage_volts.map(|volts| format!("{volts:.1}")),
                    unit: "V",
                    hint: "Supply voltage at this motor",
                }
                Readout {
                    label: "Load",
                    value: joint.load.map(|load| format!("{:.0}", load.abs() * 100.0)),
                    unit: "%",
                    hint: "Share of this motor's rated torque",
                }
                Readout {
                    label: "Current",
                    value: joint.current_amps.map(|amps| format!("{amps:.2}")),
                    unit: "A",
                    hint: "Peak draw since the last reading",
                }
                Readout {
                    label: "Speed",
                    value: joint
                        .speed_degrees_per_second
                        .map(|speed| format!("{speed:+.0}")),
                    unit: "°/s",
                    hint: "Shaft speed the motor is reporting",
                }
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
    #[props(default)] hint: String,
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
    on_select: EventHandler<Option<Selection>>,
    on_point: EventHandler<Option<Selection>>,
) -> Element {
    let studio = studio();
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
        div {
            class: if drivable { "joint-list joint-list-drivable" } else { "joint-list" },
            // The one blocker this panel cannot answer for itself: an
            // uncalibrated arm refuses every command, and the control that fixes
            // it lives in the robot explorer. Mode and torque transitions belong
            // to the workbench Control section rather than this detail panel.
            if !robot.calibrated {
                p { class: "joint-blocked joint-list-wide",
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
                    condition: joint.condition(&motor_limits(&robot, &joint.name)),
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
                        move |_| on_select.call(Some(Selection::Joint {
                            robot: robot_id.clone(),
                            joint: joint_name.clone(),
                        }))
                    },
                    on_point: {
                        let (robot_id, joint_name) = (id.clone(), joint.name.clone());
                        move |pointing: bool| {
                            on_point.call(pointing.then(|| Selection::Joint {
                                robot: robot_id.clone(),
                                joint: joint_name.clone(),
                            }))
                        }
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
    condition: MotorCondition,
    value: Option<f64>,
    on_change: EventHandler<f64>,
    on_select: EventHandler<()>,
    on_point: EventHandler<bool>,
) -> Element {
    let label = arm::so101_joint(&joint.name).map_or(joint.name.clone(), |j| j.label.to_owned());
    let measured = joint.normalized.unwrap_or(0.0);
    let shown = value.unwrap_or(measured);
    let interactive = drivable && joint.range.is_some();
    let position_percent = ((measured + 1.0) * 50.0).clamp(0.0, 100.0);
    let displayed = match joint.degrees {
        Some(degrees) => format!("{degrees:+.0}°"),
        None => format!("{}", joint.raw),
    };
    let position_title = match joint.target_degrees {
        Some(target) => format!("Measured joint angle; current target {target:+.0}°"),
        None => "Measured joint angle".to_owned(),
    };
    // Which motor is hot is a question the readouts above cannot answer in a
    // cell, so the row answers it. The tint is the whole treatment: nothing is
    // added to the row, so a joint crossing its limit does not move the stack.
    let mut classes = String::from("joint-row");
    if selected {
        classes.push_str(" joint-row-selected");
    }
    match condition {
        MotorCondition::Nominal => {}
        MotorCondition::Caution => classes.push_str(" joint-row-caution"),
        MotorCondition::Fault => classes.push_str(" joint-row-fault"),
    }

    rsx! {
        div {
            class: "{classes}",
            // Pointing at the row lights the motor on the bench, the same way
            // pointing at the motor lights the row. Neither disturbs what is
            // selected: one object, seen twice, and looking at it is not
            // choosing it.
            onpointerenter: move |_| on_point.call(true),
            onpointerleave: move |_| on_point.call(false),
            button {
                r#type: "button",
                class: "joint-label",
                title: "Select {label}",
                onfocus: move |_| on_point.call(true),
                onblur: move |_| on_point.call(false),
                onclick: move |_| on_select.call(()),
                "{label}"
            }
            if interactive {
                input {
                    r#type: "range",
                    class: "joint-slider",
                    "aria-label": "Set {label} position",
                    min: "-1",
                    max: "1",
                    step: "0.005",
                    value: "{shown}",
                    title: "{position_title}",
                    oninput: move |event| {
                        if let Ok(value) = event.value().parse::<f64>() {
                            on_change.call(value);
                        }
                    },
                }
            } else if joint.normalized.is_some() {
                span {
                    class: "joint-position-meter",
                    role: "meter",
                    "aria-label": "{label} measured position",
                    "aria-valuemin": "-1",
                    "aria-valuemax": "1",
                    "aria-valuenow": "{measured}",
                    "aria-valuetext": "{displayed}",
                    title: "{position_title}",
                    span {
                        class: "joint-position-marker",
                        style: "left: {position_percent:.2}%",
                    }
                }
            } else {
                span {
                    class: "joint-position-meter joint-position-unknown",
                    role: "img",
                    "aria-label": "{label} measured position unavailable",
                }
            }
            output { class: "joint-value", "{displayed}" }
        }
    }
}
