use crate::activities::settings::robots::RobotClient;
use crate::components::interaction::{
    Action, ActionPurpose, ActionVariant, BlockingConfirmation, ConfirmationCopy,
    ConfirmationDismissal, ConfirmationResponse,
};
use dioxus::prelude::*;
use spiral_robotics_common::robots::{CalibrationJointProgress, CalibrationProgress, RobotSummary};
use spiral_robotics_viewport as arm;

use crate::shell::notifications::MessageSource;
use crate::shell::robot_workspace::RobotWorkspace;
use crate::tasks::SingleFlight;

#[component]
pub(super) fn CalibrationSection(
    studio: RobotWorkspace,
    robot: RobotSummary,
    progress: Option<CalibrationProgress>,
) -> Element {
    let connected = robot.connection().connected();
    let mut confirming_reapply = use_signal(|| false);
    let reapplying = use_signal(|| false);
    let mut command = use_signal(|| None::<&'static str>);
    let id = robot.id().to_owned();
    let recalibrate_id = id.clone();
    let reapply_title =
        "Recalculate recorded joint state from raw encoder positions using this calibration";
    let calibration_title = if !connected {
        "Connect the arm first"
    } else if robot.calibrated() {
        "Every joint already has a recorded range. Recalibrate after rebuilding the arm or moving a horn."
    } else {
        ""
    };
    let reapply_confirm_label = if reapplying() {
        "Applying…".to_owned()
    } else {
        "Apply to recorded takes".to_owned()
    };

    let Some(progress) = progress else {
        return rsx! {
            section {
                div { class: "device-section-head",
                    h3 { class: "label mb-0 flex-1", "Calibration" }
                    if robot.calibrated() {
                        Action {
                            id: "reapply-calibration-{robot.id()}",
                            purpose: ActionPurpose::Apply,
                            disabled: reapplying() || command().is_some(),
                            title: reapply_title,
                            onclick: move |_| confirming_reapply.set(true),
                            "Reapply calibration"
                        }
                    }
                    Action {
                        purpose: ActionPurpose::Start,
                        variant: ActionVariant::Outline,
                        disabled: !connected || reapplying() || command().is_some(),
                        aria_busy: command() == Some("start"),
                        title: calibration_title,
                        onclick: move |_| {
                            if command.peek().is_some() {
                                return;
                            }
                            let id = recalibrate_id.clone();
                            command.set(Some("start"));
                            studio.run("", async move {
                                let result = RobotClient::current().start_calibration(id)
                                    .await
                                    .map_err(|error| error.to_string());
                                command.set(None);
                                result
                            });
                        },
                        {if command() == Some("start") { "Starting…" } else if robot.calibrated() { "Recalibrate" } else { "Calibrate" }}
                    }
                }
                if confirming_reapply() {
                    BlockingConfirmation {
                        id: "reapply-calibration-confirmation-{robot.id()}",
                        title: format!("Apply {} calibration to recorded takes?", robot.id()),
                        consequence: "Recorded actions and raw encoder positions are preserved. Measured joint state will be replaced.".to_owned(),
                        copy: ConfirmationCopy::new(
                            "Recalculate measured joint state in every recorded take containing this arm using this calibration.",
                            reapply_confirm_label,
                            "Keep recorded data",
                        ),
                        response: ConfirmationResponse::new(
                            ActionPurpose::Apply,
                            {
                                let id = id.clone();
                                move |_| {
                                if reapplying() {
                                    return;
                                }
                                let id = id.clone();
                                studio.run_utility("", async move {
                                    let Some(result) = SingleFlight::run(
                                        reapplying,
                                        async move {
                                            RobotClient::current().reapply_calibration(id)
                                                .await
                                                .map_err(|error| error.to_string())
                                        },
                                    )
                                    .await
                                    else {
                                        return Ok(());
                                    };
                                    if result.is_ok() {
                                        confirming_reapply.set(false);
                                    }
                                    match result {
                                        Ok(result) => {
                                            studio.report_success(result.reapply_success());
                                            if !result.warnings().is_empty() {
                                                studio.report_warning_from(
                                                    MessageSource::Studio,
                                                    result.warnings().join(" "),
                                                );
                                            }
                                            Ok(())
                                        }
                                        Err(error) => Err(error),
                                    }
                                });
                                }
                            },
                            move |_| confirming_reapply.set(false),
                        ),
                        dismissal: ConfirmationDismissal::new(format!(
                            "reapply-calibration-{}",
                            robot.id(),
                        )).with_pending(reapplying()),
                    }
                }
                // Instructions only where there is still work to do. On a
                // calibrated arm the paragraph said nothing the button did not,
                // and it said it once per arm, forever.
                if !robot.calibrated() {
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
                Action {
                    purpose: ActionPurpose::Apply,
                    variant: ActionVariant::Solid,
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
                            studio.run("Calibration saved.", async move {
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
                Action {
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
                            studio.run("", async move {
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

trait CalibrationReapplyPresentation {
    fn reapply_success(&self) -> String;
}

impl CalibrationReapplyPresentation for spiral_robotics_common::robots::CalibrationApplication {
    fn reapply_success(&self) -> String {
        let episodes = self.episodes_updated();
        let frames = self.frames_updated();
        let unchanged = self.episodes_unchanged();
        let skipped = self.episodes_skipped();
        let noun = if episodes == 1 { "take" } else { "takes" };
        let mut message = format!(
            "Reapplied calibration to {episodes} {noun} ({} frames)",
            frames
        );
        if unchanged > 0 {
            message.push_str(&format!("; {unchanged} already matched"));
        }
        if skipped > 0 {
            message.push_str(&format!("; {skipped} skipped"));
        }
        message.push('.');
        message
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
