use crate::robots::RobotClient;
use dioxus::prelude::*;
use spiral_robotics_common::robots::{CalibrationJointProgress, RobotSummary};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};
use spiral_robotics_viewport as arm;

use crate::robots::{HardwareCommands, HardwareViewState};
use crate::shell::state::StudioProjectState;

#[component]
pub(super) fn CalibrationSection(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    robot: RobotSummary,
) -> Element {
    let progress = telemetry
        .robot_telemetry(robot.id())
        .and_then(|robot| robot.calibration().cloned());
    let connected = robot.connection().connected();
    let mut command = use_signal(|| None::<&'static str>);
    let id = robot.id().to_owned();
    let recalibrate_id = id.clone();
    let calibration_title = if !connected {
        "Connect the arm first"
    } else if robot.config().calibrated() {
        "Every joint already has a recorded range. Recalibrate after rebuilding the arm or moving a horn."
    } else {
        ""
    };
    let Some(progress) = progress else {
        return rsx! {
            section {
                div { class: "device-section-head",
                    h3 { class: "label mb-0 flex-1", "Calibration" }
                    Button {
                        purpose: ActionPurpose::Start,
                        variant: ButtonVariant::Outline,
                        disabled: !connected || command().is_some(),
                        aria_busy: command() == Some("start"),
                        title: calibration_title,
                        onclick: move |_| {
                            if command.peek().is_some() {
                                return;
                            }
                            let id = recalibrate_id.clone();
                            command.set(Some("start"));
                            HardwareCommands::new(project, telemetry).run("", async move {
                                let result = RobotClient::current().start_calibration(id)
                                    .await
                                    .map_err(|error| error.to_string());
                                command.set(None);
                                result
                            });
                        },
                        {if command() == Some("start") { "Starting…" } else if robot.config().calibrated() { "Recalibrate" } else { "Calibrate" }}
                    }
                }
                // Instructions only where there is still work to do. On a
                // calibrated arm the paragraph said nothing the button did not,
                // and it said it once per arm, forever.
                if !robot.config().calibrated() {
                    p { class: "device-section-note",
                        "Record each joint's travel so the studio can command it safely."
                    }
                }
            }
        };
    };

    rsx! {
        section {
            div { class: "flex items-center gap-1",
                h3 { class: "label mb-0 flex-1", "Calibrating" }
                Button {
                    purpose: ActionPurpose::Apply,
                    variant: ButtonVariant::Solid,
                    disabled: !progress.ready() || command().is_some(),
                    aria_busy: command() == Some("finish"),
                    title: if progress.ready() { "" } else { "Every joint still needs to move" },
                    onclick: {
                        let id = id.clone();
                        move |_| {
                            if command.peek().is_some() {
                                return;
                            }
                            let id = id.clone();
                            command.set(Some("finish"));
                            HardwareCommands::new(project, telemetry).run("Calibration saved.", async move {
                                let result = RobotClient::current().finish_calibration(id)
                                    .await
                                    .map_err(|error| error.to_string())
                                    .map(|_| ());
                                command.set(None);
                                result
                            });
                        }
                    },
                    if command() == Some("finish") { "Saving…" } else { "Save" }
                }
                Button {
                    purpose: ActionPurpose::Cancel,
                    disabled: command().is_some(),
                    aria_busy: command() == Some("cancel"),
                    onclick: {
                        let id = id.clone();
                        move |_| {
                            if command.peek().is_some() {
                                return;
                            }
                            let id = id.clone();
                            command.set(Some("cancel"));
                            HardwareCommands::new(project, telemetry).run("", async move {
                                let result = RobotClient::current().cancel_calibration(id)
                                    .await
                                    .map_err(|error| error.to_string());
                                command.set(None);
                                result
                            });
                        }
                    },
                    if command() == Some("cancel") { "Cancelling…" } else { "Cancel" }
                }
            }
            p { class: "device-section-note device-section-note-live",
                "Torque is off. Move every joint to both ends, then save."
            }
            ul { class: "flex flex-col gap-0.5",
                for joint in progress.joints().to_owned() {
                    li { key: "{joint.name()}", class: "flex min-h-5 items-center gap-1.5",
                        span { class: "w-16 shrink-0 truncate text-[11px]",
                            {arm::Joint::named(joint.name()).map_or_else(|| joint.name().to_owned(), |known| known.label().to_owned())}
                        }
                        SweepBar { joint: joint.clone() }
                        span {
                            class: "w-3 shrink-0 text-center font-mono text-[10px]",
                            style: if joint.ready() { "color: var(--tone-good)" } else { "color: var(--tone-line)" },
                            "✓"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SweepBar(joint: CalibrationJointProgress) -> Element {
    let full = joint.encoder_counts_per_turn().max(1) as f64;
    let percent = |value: i64| (value as f64 / full * 100.0).clamp(0.0, 100.0);
    let left = percent(joint.min());
    let width = (percent(joint.max()) - left).max(0.5);

    rsx! {
        div { class: "relative h-1 flex-1 rounded-full bg-line",
            div { class: "absolute inset-y-0 rounded-full bg-ink-faint", style: "left: {left}%; width: {width}%" }
            div { class: "absolute top-1/2 h-2.5 w-0.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-ink", style: "left: {percent(joint.position())}%" }
        }
    }
}
