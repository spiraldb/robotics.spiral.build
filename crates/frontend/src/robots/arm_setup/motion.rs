//! How far the control loop moves one arm toward its goal in each 60 Hz
//! cycle, in encoder counts.

use dioxus::prelude::*;
use spiral_robotics_common::robots::RobotSummary;
use spiral_robotics_common::robots::config::{MotionLimits, RobotConfigSave};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Field, FieldLayout, TextInput, TextInputKind,
};

use crate::robots::{HardwareCommands, HardwareViewState};
use crate::shell::state::StudioProjectState;

/// The arm's two motion limits, saved together. The open motor bus holds
/// them, so like the port they change only while the arm is disconnected.
#[component]
pub(super) fn MotionSection(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    robot: RobotSummary,
) -> Element {
    let saved = robot.config().motion();
    let mut manual = use_signal(|| saved.manual_step_counts());
    let mut following = use_signal(|| saved.following_step_counts());
    // The limits move with a save here or in another browser; so do the fields.
    use_effect(use_reactive((&saved,), move |(saved,)| {
        manual.set(saved.manual_step_counts());
        following.set(saved.following_step_counts());
    }));
    let mut save_motion = use_action(move |request: RobotConfigSave| {
        HardwareCommands::new(project, telemetry).save_robot(request)
    });
    let locked = robot.connection().connected();
    let changed =
        (manual(), following()) != (saved.manual_step_counts(), saved.following_step_counts());
    let disabled = locked || save_motion.pending();

    rsx! {
        section { class: "device-section-content",
            div { class: "device-section-head",
                h3 { class: "label mb-0 flex-1", "Motion" }
                if changed {
                    Button {
                        purpose: ActionPurpose::Apply,
                        variant: ButtonVariant::Link,
                        disabled,
                        aria_busy: save_motion.pending(),
                        title: if locked { "Disconnect this arm before changing its motion" } else { "" },
                        onclick: move |_| match MotionLimits::new(manual(), following()) {
                            Ok(motion) => {
                                save_motion.call(RobotConfigSave::new(
                                    robot.config().clone().with_motion(motion),
                                    Some(robot.revision()),
                                ));
                            }
                            Err(error) => project.report(error),
                        },
                        if save_motion.pending() { "Saving…" } else { "Save" }
                    }
                }
            }
            div { class: "flex flex-col gap-1",
                MotionStepField {
                    label: "Manual step",
                    title: "Most encoder counts per control cycle toward a goal set by hand",
                    counts: manual,
                    disabled,
                }
                MotionStepField {
                    label: "Following step",
                    title: "Most encoder counts per control cycle while following a leader or a policy",
                    counts: following,
                    disabled,
                }
            }
        }
    }
}

/// One limit: its name hard left, its count hard right.
#[component]
fn MotionStepField(
    label: &'static str,
    title: &'static str,
    counts: Signal<u32>,
    disabled: bool,
) -> Element {
    let mut counts = counts;
    rsx! {
        Field { label, layout: FieldLayout::Inline, class: "device-motion-field", title,
            TextInput {
                kind: TextInputKind::Number,
                min: "1",
                step: "1",
                value: "{counts}",
                disabled,
                oninput: move |value: String| {
                    if let Ok(value) = value.trim().parse() {
                        counts.set(value);
                    }
                },
            }
        }
    }
}
