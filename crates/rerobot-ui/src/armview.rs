//! A live drawing of one SO-ARM101.
//!
//! Two orthogonal views tell the whole story of a six-joint arm: a side view
//! carries the three joints that share the vertical working plane, and a small
//! top view carries the base rotation. Everything is derived from measured
//! joint angles, so the picture is a readout, not an animation.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use rerobot_core::api::RobotTelemetry;
use rerobot_core::arm::{ArmPose, JointAngles, Point, SO101_GEOMETRY};

/// Draw the arm described by `telemetry`.
///
/// An uncalibrated or disconnected arm still draws, at its rest pose and
/// visibly dimmed, because an empty panel says less than a greyed-out one.
#[component]
pub fn ArmView(telemetry: Option<RobotTelemetry>, calibrated: bool) -> Element {
    let joints = telemetry.as_ref().map(|robot| &robot.joints);
    let degrees = joints
        .map(|joints| {
            joints
                .iter()
                .filter_map(|joint| Some((joint.name.clone(), joint.degrees?)))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let gripper = joints
        .and_then(|joints| {
            joints
                .iter()
                .find(|joint| joint.name == "gripper")
                .and_then(|joint| joint.normalized)
        })
        .unwrap_or(0.0);

    let live = calibrated && !degrees.is_empty();
    let angles = JointAngles::from_named_degrees(&degrees).with_gripper_normalized(gripper);
    let pose = ArmPose::solve_so101(&angles);

    rsx! {
        div { class: "flex flex-col gap-2",
            div {
                class: if live { "relative" } else { "relative opacity-40" },
                SideView { pose }
                div { class: "absolute top-2 right-2 flex flex-col items-end gap-1",
                    TopView { pan_deg: pose.pan_deg, reach_mm: pose.reach_mm() }
                }
                if !live {
                    div { class: "absolute inset-0 flex items-center justify-center",
                        span { class: "rounded-md bg-slate-900/80 px-3 py-1.5 text-xs font-medium text-white",
                            {if calibrated { "Not connected" } else { "Calibrate to see live joint angles" }}
                        }
                    }
                }
            }
            dl { class: "grid grid-cols-3 gap-2 text-center",
                Readout { label: "Reach", value: format!("{:.0} mm", pose.reach_mm()) }
                Readout { label: "Height", value: format!("{:.0} mm", pose.tool.y) }
                Readout { label: "Gripper", value: format!("{:.0}%", pose.gripper * 100.0) }
            }
        }
    }
}

#[component]
fn Readout(label: &'static str, value: String) -> Element {
    rsx! {
        div { class: "rounded-lg bg-slate-100 px-2 py-1.5 dark:bg-slate-800",
            dt { class: "text-[10px] font-medium tracking-wide text-slate-500 uppercase dark:text-slate-400",
                "{label}"
            }
            dd { class: "font-mono text-sm text-slate-900 dark:text-slate-100", "{value}" }
        }
    }
}

/// The arm's vertical working plane, seen from the side.
#[component]
fn SideView(pose: ArmPose) -> Element {
    // Millimetres are used directly as user units; the group flips the y axis
    // so the model's "up" is up on screen. The arm is a tall subject in a wide
    // panel, so the drawing keeps its own aspect ratio and is centred rather
    // than stretched to the full width.
    // The element's aspect ratio matches the view box exactly, so the drawing
    // fills its frame instead of being letterboxed inside it.
    rsx! {
        svg {
            class: "mx-auto aspect-[74/57] w-full max-w-md rounded-lg bg-slate-100 dark:bg-slate-800",
            view_box: "-370 -420 740 570",
            role: "img",
            "aria-label": "Side view of the arm",
            g { transform: "scale(1,-1)",
                // Mounting surface.
                line {
                    class: "stroke-slate-300 dark:stroke-slate-700",
                    x1: "-260", y1: "0", x2: "260", y2: "0",
                    stroke_width: "6", stroke_linecap: "round",
                }
                rect {
                    class: "fill-slate-300 dark:fill-slate-700",
                    x: "-50", y: "0", width: "100",
                    height: "{SO101_GEOMETRY.base_height_mm}",
                    rx: "8",
                }
                Link { from: pose.shoulder, to: pose.elbow, width: 22 }
                Link { from: pose.elbow, to: pose.wrist, width: 18 }
                Link { from: pose.wrist, to: pose.tool, width: 12 }
                Jaws { pose }
                Pivot { at: pose.shoulder, radius: 13 }
                Pivot { at: pose.elbow, radius: 11 }
                Pivot { at: pose.wrist, radius: 9 }
            }
        }
    }
}

#[component]
fn Link(from: Point, to: Point, width: i32) -> Element {
    rsx! {
        line {
            class: "stroke-sky-600 dark:stroke-sky-500",
            x1: "{from.x}", y1: "{from.y}", x2: "{to.x}", y2: "{to.y}",
            stroke_width: "{width}", stroke_linecap: "round",
        }
    }
}

#[component]
fn Pivot(at: Point, radius: i32) -> Element {
    rsx! {
        circle {
            class: "fill-white stroke-sky-700 dark:fill-slate-900 dark:stroke-sky-400",
            cx: "{at.x}", cy: "{at.y}", r: "{radius}", stroke_width: "5",
        }
    }
}

/// Two jaws opening symmetrically across the tool axis.
#[component]
fn Jaws(pose: ArmPose) -> Element {
    let half_span = SO101_GEOMETRY.gripper_span_mm * pose.gripper / 2.0;
    // The jaws slide perpendicular to the direction the tool points.
    let radians = pose.tool_deg.to_radians();
    let (offset_x, offset_y) = (radians.cos() * half_span, -radians.sin() * half_span);
    let (tip_x, tip_y) = (radians.sin() * 26.0, radians.cos() * 26.0);

    rsx! {
        for sign in [1.0_f64, -1.0] {
            line {
                key: "{sign}",
                class: "stroke-amber-500",
                x1: "{pose.tool.x + offset_x * sign}",
                y1: "{pose.tool.y + offset_y * sign}",
                x2: "{pose.tool.x + offset_x * sign + tip_x}",
                y2: "{pose.tool.y + offset_y * sign + tip_y}",
                stroke_width: "8",
                stroke_linecap: "round",
            }
        }
    }
}

/// The base rotation, seen from above.
#[component]
fn TopView(pan_deg: f64, reach_mm: f64) -> Element {
    let radius = 40.0;
    // Zero degrees points at the top of the dial, matching the side view's
    // "front of the robot is to the right" convention rotated into plan view.
    let radians = pan_deg.to_radians();
    let extent = (reach_mm.abs() / 330.0).clamp(0.25, 1.0) * radius;
    let (x, y) = (radians.sin() * extent, -radians.cos() * extent);

    rsx! {
        div { class: "flex flex-col items-center gap-0.5 rounded-lg bg-white/70 px-2 py-1.5 dark:bg-slate-900/70",
            svg {
                class: "h-20 w-20",
                view_box: "-50 -50 100 100",
                role: "img",
                "aria-label": "Base rotation seen from above",
                circle {
                    class: "fill-slate-100 stroke-slate-300 dark:fill-slate-800 dark:stroke-slate-600",
                    cx: "0", cy: "0", r: "{radius}", stroke_width: "2",
                }
                line {
                    class: "stroke-slate-300 dark:stroke-slate-600",
                    x1: "0", y1: "-{radius}", x2: "0", y2: "{radius}", stroke_width: "1",
                }
                line {
                    class: "stroke-sky-600 dark:stroke-sky-400",
                    x1: "0", y1: "0", x2: "{x}", y2: "{y}",
                    stroke_width: "6", stroke_linecap: "round",
                }
                circle { class: "fill-slate-500", cx: "0", cy: "0", r: "4" }
            }
            span { class: "font-mono text-[10px] text-slate-500 dark:text-slate-400",
                "{pan_deg:.0}°"
            }
        }
    }
}
