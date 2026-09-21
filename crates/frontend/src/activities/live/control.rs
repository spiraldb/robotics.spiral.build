//! Joint controls for the arm selected on the Live screen.

use crate::robots::RobotClient;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::robots::visualization::ConfiguredModel;
use dioxus::prelude::*;
use spiral_robotics_common::robots::{
    JointTelemetry, MotorCondition, MotorLimits, RobotSummary, RobotTelemetry,
};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonAuxiliaryHandlers, ButtonVariant};
use spiral_robotics_viewport as arm;

use crate::robots::Selection;
use crate::robots::{HardwareViewState, RobotSelection};
use crate::shell::state::StudioProjectState;

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
pub(super) fn JointPanel(
    robot_selection: RobotSelection,
    project: StudioProjectState,
    hardware: HardwareViewState,
    on_select: EventHandler<Option<Selection>>,
    on_point: EventHandler<Option<Selection>>,
) -> Element {
    let Some(robot) = robot_selection.focused(hardware.snapshot().read().robots()) else {
        return rsx! {
            p { class: "px-2 py-1 text-[11px] text-ink-faint", "Add an arm to see its joints." }
        };
    };

    let telemetry = hardware.robot_telemetry(robot.id());

    rsx! {
        div { class: "live-joint-panel flex flex-col",
            if let Some(error) = telemetry.as_ref().and_then(|robot| robot.error().to_owned()) {
                p { class: "mx-2 mt-1 rounded bg-danger-soft px-1.5 py-1 text-[11px] text-danger",
                    "{error}"
                }
            }

            ToolReadout {
                telemetry: telemetry.clone(),
                robot: robot.clone(),
            }

            ConditionReadout {
                robot: robot.clone(),
                telemetry: telemetry.clone(),
            }

            JointControls {
                project, hardware,
                robot: robot.clone(),
                behavior: JointInteraction::new(robot_selection.selected(), on_select, on_point),
            }

            MotorDetail {
                selection: robot_selection.selected(),
                robot: robot.clone(),
                telemetry: telemetry.clone(),
            }

        }
    }
}

/// Where the arm is holding its gripper, solved from the robot description.
#[component]
fn ToolReadout(telemetry: Option<RobotTelemetry>, robot: RobotSummary) -> Element {
    let configured = use_memo(use_reactive(&robot, |robot| {
        robot
            .document()?
            .visualization()
            .ok()?
            .and_then(|configuration| ConfiguredModel::new(configuration).ok())
    }));
    let pose = telemetry
        .as_ref()
        .filter(|_| robot.calibrated())
        .and_then(|telemetry| {
            let configured = configured.read();
            let configured = configured.as_ref()?;
            let positions = configured.measured_positions(telemetry.joints());
            let tip = configured
                .configuration
                .as_ref()?
                .tool_link()
                .and_then(|link| configured.model.tool_position(link, &positions));
            Some((tip, configured.model.joint_fraction("gripper", &positions)))
        });
    let tip = pose.and_then(|(tip, _)| tip);
    let tracking_error = telemetry.as_ref().and_then(|robot| {
        robot
            .joints()
            .iter()
            .filter_map(|joint| Some((joint.degrees()? - joint.target_degrees()?).abs()))
            .filter(|error| error.is_finite())
            .reduce(f64::max)
    });
    let lagging = telemetry
        .as_ref()
        .is_some_and(RobotTelemetry::tracking_lagging);

    rsx! {
        dl { class: "live-summary-readouts grid gap-1.5 px-3 pb-1 pt-2",
            Readout {
                label: "Reach",
                value: tip.map(|[x, y, _]| format!("{:.0}", x.hypot(y))),
                details: ReadoutDetails::new("mm"),
            }
            Readout {
                label: "Height",
                value: tip.map(|[_, _, z]| format!("{z:.0}")),
                details: ReadoutDetails::new("mm"),
            }
            // A bare percentage does not say which end is which, and reading it
            // backwards is exactly the mistake this readout used to invite — so
            // "open" moved into the label, where it names the direction without
            // wrapping the value onto a second line and knocking the four cells
            // out of alignment with each other.
            Readout {
                label: "Gripper open",
                value: pose.and_then(|(_, opening)| opening).map(|opening| format!("{:.0}", opening * 100.0)),
                details: ReadoutDetails::new("%"),
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
                details: ReadoutDetails::new("°")
                    .with_hint("The largest gap between where a joint is and where it was told to be")
                    .with_caution(lagging),
            }
        }
    }
}

/// The limits the named motor reported for itself when it connected.
trait RobotMotorLimits {
    fn motor_limits(&self, joint: &str) -> MotorLimits;
}

impl RobotMotorLimits for RobotSummary {
    fn motor_limits(&self, joint: &str) -> MotorLimits {
        self.motors()
            .iter()
            .find(|motor| motor.name() == joint)
            .map(|motor| motor.limits())
            .unwrap_or_default()
    }
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
    let joints = telemetry
        .map(|robot| robot.joints().to_owned())
        .unwrap_or_default();

    // The hottest motor decides both thermal cells, so they always describe the
    // same motor as each other.
    let hottest = joints
        .iter()
        .filter(|joint| joint.temperature_celsius().is_some())
        .max_by(|left, right| {
            left.temperature_celsius()
                .unwrap_or_default()
                .total_cmp(&right.temperature_celsius().unwrap_or_default())
        });
    let hot = hottest.is_some_and(|joint| joint.condition() != MotorCondition::Nominal);

    // Every motor shares one supply, so the interesting number is the worst of
    // them: the motor seeing the deepest sag is the one about to complain.
    let supply = joints
        .iter()
        .filter_map(|joint| joint.voltage_volts())
        .reduce(f64::min);
    let sagging = joints.iter().any(|joint| {
        joint.voltage_volts().is_some()
            && joint.voltage_volts() == supply
            && joint.condition() != MotorCondition::Nominal
    });

    let hardest = joints
        .iter()
        .filter(|joint| joint.load().is_some())
        .max_by(|left, right| {
            left.load()
                .unwrap_or_default()
                .abs()
                .total_cmp(&right.load().unwrap_or_default().abs())
        });
    let strained = hardest.is_some_and(|joint| joint.condition() != MotorCondition::Nominal);

    rsx! {
        dl { class: "live-summary-readouts grid gap-1.5 px-3 pb-2 pt-0.5",
            Readout {
                label: "Hottest",
                value: hottest
                    .and_then(|joint| joint.temperature_celsius())
                    .map(|celsius| format!("{celsius:.0}")),
                details: ReadoutDetails::new("°C")
                    .with_hint("The warmest motor on this arm")
                    .with_caution(hot),
            }
            // No caution tint: a rise has no limit to be near, and this studio
            // does not invent thresholds the hardware did not state.
            Readout {
                label: "Rise",
                value: hottest
                    .and_then(|joint| joint.temperature_rise_per_minute())
                    .map(|rate| format!("{rate:+.0}")),
                details: ReadoutDetails::new("°C/m")
                    .with_hint("How fast the warmest motor is heating, measured over the last minute"),
            }
            Readout {
                label: "Supply",
                value: supply.map(|volts| format!("{volts:.1}")),
                details: ReadoutDetails::new("V")
                    .with_hint("The lowest supply voltage any motor on this bus reports")
                    .with_caution(sagging),
            }
            Readout {
                label: "Load",
                value: hardest
                    .and_then(|joint| joint.load())
                    .map(|load| format!("{:.0}", load.abs() * 100.0)),
                details: ReadoutDetails::new("%")
                    .with_hint("The hardest any motor is working, as a share of its rated torque")
                    .with_caution(strained),
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
fn MotorDetail(
    selection: Signal<Option<Selection>>,
    robot: RobotSummary,
    telemetry: Option<RobotTelemetry>,
) -> Element {
    let selected = selection.read().to_owned();
    let Some(name) = selected.and_then(|selection| match selection {
        Selection::Joint {
            robot: selected,
            joint,
        } if selected == robot.id() => Some(joint),
        _ => None,
    }) else {
        return rsx! {};
    };
    let Some(joint) = telemetry.and_then(|telemetry| {
        telemetry
            .joints()
            .iter()
            .find(|joint| joint.name() == name)
            .cloned()
    }) else {
        return rsx! {};
    };

    let label =
        arm::Joint::named(joint.name()).map_or(joint.name().to_owned(), |j| j.label().to_owned());
    let model = robot
        .motors()
        .iter()
        .find(|motor| motor.name() == joint.name())
        .and_then(|motor| motor.model().to_owned());
    let limits = robot.motor_limits(joint.name());
    let condition = joint.condition();

    rsx! {
        section { class: "motor-detail",
            header { class: "motor-detail-head",
                h3 { class: "motor-detail-name", "{label}" }
                // One line whatever happens: identity when the motor is well,
                // its own complaint when it is not. A line that appears only on
                // a fault would push the readouts below it down a row at the
                // exact moment they matter most.
                if joint.faults().is_empty() {
                    span { class: "motor-detail-id",
                        "id {joint.id()}"
                        if let Some(model) = model {
                            " · {model}"
                        }
                    }
                } else {
                    span { class: "motor-detail-fault",
                        {joint.faults().iter().map(|fault| fault.label()).collect::<Vec<_>>().join(" · ")}
                    }
                }
            }
            dl { class: "grid grid-cols-3 gap-1.5",
                Readout {
                    label: "Temp",
                    value: joint.temperature_celsius().map(|celsius| format!("{celsius:.0}")),
                    details: ReadoutDetails::new("°C")
                        .with_hint(match limits.max_temperature_celsius() {
                            Some(limit) => format!("This motor faults at {limit:.0} °C"),
                            None => "This motor did not report a temperature limit".to_owned(),
                        })
                        .with_caution(condition != MotorCondition::Nominal),
                }
                Readout {
                    label: "Rise",
                    value: joint.temperature_rise_per_minute().map(|rate| format!("{rate:+.0}")),
                    details: ReadoutDetails::new("°C/m")
                        .with_hint("Measured over the last minute"),
                }
                Readout {
                    label: "Supply",
                    value: joint.voltage_volts().map(|volts| format!("{volts:.1}")),
                    details: ReadoutDetails::new("V").with_hint("Supply voltage at this motor"),
                }
                Readout {
                    label: "Load",
                    value: joint.load().map(|load| format!("{:.0}", load.abs() * 100.0)),
                    details: ReadoutDetails::new("%")
                        .with_hint("Share of this motor's rated torque"),
                }
                Readout {
                    label: "Current",
                    value: joint.current_amps().map(|amps| format!("{amps:.2}")),
                    details: ReadoutDetails::new("A")
                        .with_hint("Peak draw since the last reading"),
                }
                Readout {
                    label: "Speed",
                    value: joint
                        .speed_degrees_per_second()
                        .map(|speed| format!("{speed:+.0}")),
                    details: ReadoutDetails::new("°/s")
                        .with_hint("Shaft speed the motor is reporting"),
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
#[derive(Clone, PartialEq)]
struct ReadoutDetails {
    unit: &'static str,
    caution: bool,
    hint: String,
}

impl ReadoutDetails {
    const fn new(unit: &'static str) -> Self {
        Self {
            unit,
            caution: false,
            hint: String::new(),
        }
    }

    fn with_caution(mut self, caution: bool) -> Self {
        self.caution = caution;
        self
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
}

#[component]
fn Readout(label: String, value: Option<String>, details: ReadoutDetails) -> Element {
    let ReadoutDetails {
        unit,
        caution,
        hint,
    } = details;
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

#[component]
fn JointControls(
    project: StudioProjectState,
    hardware: HardwareViewState,
    robot: RobotSummary,
    behavior: JointInteraction,
) -> Element {
    let telemetry = hardware.robot_telemetry(robot.id());
    let JointInteraction {
        selection,
        on_select,
        on_point,
    } = behavior;
    let torque = telemetry.as_ref().is_some_and(|robot| robot.torque());
    let drivable = hardware.is_manual_target(robot.id()) && torque && robot.calibrated();
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
    let command_robot = robot.id().to_owned();
    let _ = hardware.is_active();
    let mut sender = use_future({
        let pending = pending.clone();
        move || {
            let pending = pending.clone();
            let command_robot = command_robot.clone();
            async move {
                loop {
                    gloo_timers::future::TimeoutFuture::new(32).await;
                    let may_drive = hardware.is_active()
                        && hardware.controls_available()
                        && hardware.is_manual_target(&command_robot)
                        && hardware
                            .robot_telemetry(&command_robot)
                            .is_some_and(|robot| robot.torque());
                    if !may_drive {
                        pending.borrow_mut().clear();
                        continue;
                    }
                    let normalized = pending.borrow_mut().take();
                    if normalized.is_empty() {
                        continue;
                    }
                    if let Err(error) = RobotClient::current()
                        .set_robot_goal(command_robot.clone(), normalized)
                        .await
                        .map_err(|error| error.to_string())
                    {
                        project.report(error);
                    }
                }
            }
        }
    });
    let inactive_pending = pending.clone();
    use_hook_did_run(move |did_run| {
        if did_run && hardware.is_active() {
            sender.resume();
        } else {
            inactive_pending.borrow_mut().clear();
            sender.pause();
        }
    });
    let watched = robot.id().to_owned();
    let clear_pending = pending.clone();
    use_effect(move || {
        let driving = hardware.is_active()
            && hardware
                .frame()
                .read()
                .robots()
                .iter()
                .any(|robot| robot.id() == watched && robot.torque())
            && hardware.is_manual_target(&watched);
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

    let id = robot.id().to_owned();

    rsx! {
        div {
            class: if drivable { "joint-list joint-list-drivable" } else { "joint-list" },
            // The one blocker this panel cannot answer for itself: an
            // uncalibrated arm refuses every command, and the control that fixes
            // it lives in the robot explorer. Mode and torque transitions belong
            // to the workbench Control section rather than this detail panel.
            if !robot.calibrated() {
                p { class: "joint-blocked joint-list-wide",
                    "Calibrate this arm in the robot explorer to command its joints."
                }
            }
            for joint in telemetry.joints().to_owned() {
                JointRow {
                    key: "{joint.name()}",
                    joint: joint.clone(),
                    state: JointRowState {
                        drivable,
                        selected: selection
                            .read()
                            .as_ref()
                            .is_some_and(|selection| selection.matches_joint(robot.id(), joint.name())),
                        condition: joint.condition(),
                        value: drivable
                            .then(|| commanded.read().get(joint.name()).copied())
                            .flatten(),
                    },
                    actions: JointRowActions {
                        on_change: EventHandler::new({
                            let name = joint.name().to_owned();
                            let commander = JointCommander::new(commanded, pending.clone());
                            move |value| commander.clone().set(name.clone(), value)
                        }),
                        on_select: EventHandler::new({
                            let (robot_id, joint_name) = (id.clone(), joint.name().to_owned());
                            move |_| on_select.call(Some(Selection::Joint {
                                robot: robot_id.clone(),
                                joint: joint_name.clone(),
                            }))
                        }),
                        on_point: EventHandler::new({
                            let (robot_id, joint_name) = (id.clone(), joint.name().to_owned());
                            move |pointing: bool| {
                                on_point.call(pointing.then(|| Selection::Joint {
                                    robot: robot_id.clone(),
                                    joint: joint_name.clone(),
                                }))
                            }
                        }),
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct JointInteraction {
    selection: Signal<Option<Selection>>,
    on_select: EventHandler<Option<Selection>>,
    on_point: EventHandler<Option<Selection>>,
}

impl JointInteraction {
    const fn new(
        selection: Signal<Option<Selection>>,
        on_select: EventHandler<Option<Selection>>,
        on_point: EventHandler<Option<Selection>>,
    ) -> Self {
        Self {
            selection,
            on_select,
            on_point,
        }
    }
}

/// Command one joint and remember what was asked for.
///
/// The commanded value is kept locally so the slider stays where the operator
/// put it while the arm ramps toward it.
#[derive(Clone)]
struct JointCommander {
    commanded: Signal<BTreeMap<String, f64>>,
    pending: Rc<RefCell<PendingGoals>>,
}

impl JointCommander {
    fn new(commanded: Signal<BTreeMap<String, f64>>, pending: Rc<RefCell<PendingGoals>>) -> Self {
        Self { commanded, pending }
    }

    fn set(mut self, joint: String, value: f64) {
        self.commanded.write().insert(joint.clone(), value);
        self.pending.borrow_mut().replace(joint, value);
    }
}

#[cfg(test)]
mod tests {
    use super::PendingGoals;

    #[wasm_bindgen_test::wasm_bindgen_test]
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

#[derive(Clone, Copy, PartialEq)]
struct JointRowState {
    drivable: bool,
    selected: bool,
    condition: MotorCondition,
    value: Option<f64>,
}

#[derive(Clone, PartialEq)]
struct JointRowActions {
    on_change: EventHandler<f64>,
    on_select: EventHandler<()>,
    on_point: EventHandler<bool>,
}

#[component]
fn JointRow(joint: JointTelemetry, state: JointRowState, actions: JointRowActions) -> Element {
    let label =
        arm::Joint::named(joint.name()).map_or(joint.name().to_owned(), |j| j.label().to_owned());
    let measured = joint.normalized().unwrap_or(0.0);
    let shown = state.value.unwrap_or(measured);
    let interactive = state.drivable && joint.range().is_some();
    let position_percent = ((measured + 1.0) * 50.0).clamp(0.0, 100.0);
    let displayed = match joint.degrees() {
        Some(degrees) => format!("{degrees:+.0}°"),
        None => format!("{}", joint.raw()),
    };
    let position_title = match joint.target_degrees() {
        Some(target) => format!("Measured joint angle; current target {target:+.0}°"),
        None => "Measured joint angle".to_owned(),
    };
    // Which motor is hot is a question the readouts above cannot answer in a
    // cell, so the row answers it. The tint is the whole treatment: nothing is
    // added to the row, so a joint crossing its limit does not move the stack.
    let mut classes = String::from("joint-row");
    if state.selected {
        classes.push_str(" joint-row-selected");
    }
    match state.condition {
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
            onpointerenter: move |_| actions.on_point.call(true),
            onpointerleave: move |_| actions.on_point.call(false),
            Button {
                purpose: ActionPurpose::Select,
                variant: ButtonVariant::Row,
                class: "joint-label",
                title: "Select {label}",
                auxiliary_handlers: ButtonAuxiliaryHandlers::default().with_focus(
                    move |_| actions.on_point.call(true),
                    move |_| actions.on_point.call(false),
                ),
                onclick: move |_| actions.on_select.call(()),
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
                            actions.on_change.call(value);
                        }
                    },
                }
            } else if joint.normalized().is_some() {
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
