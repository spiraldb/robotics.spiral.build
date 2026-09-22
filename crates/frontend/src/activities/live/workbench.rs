//! Live screen workbench: capture, control, and selected-arm state.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use spiral_robotics_common::robots::ModeSelection;
use spiral_robotics_ui::{
    ActionPurpose, BlockingConfirmation, Button, ButtonVariant, ConfirmationCopy,
    ConfirmationDismissal, ConfirmationResponse, Field, FieldLayout, ModeButton, ModeGroup,
    TextInput,
};

use crate::activities::live::actions::LiveAction;
use crate::activities::live::control::JointPanel;
use crate::components::dataset_picker::DatasetCatalog;
use crate::robots::Selection;
use crate::robots::{HardwareViewState, RobotSelection};
use crate::shell::state::StudioProjectState;
use spiral_robotics_common::recording::RecordingProgress;

/// The complete left-hand workbench for operating a connected pair.
#[component]
pub(super) fn LiveWorkbench(
    project: StudioProjectState,
    hardware: HardwareViewState,
    selection: RobotSelection,
    capture: CaptureDestination,
    on_action: EventHandler<LiveAction>,
) -> Element {
    let robots = hardware.snapshot().read().robots().to_owned();

    rsx! {
        div { class: "studio-workbench-scroll live-workbench",
            ControlSection { project, hardware, selection, on_action }
            CaptureSection { capture, recording: hardware.recording() }

            section { class: "workbench-section live-properties",
                header { class: "workbench-section-heading",
                    h3 { "Motors" }
                }
                RobotRoster {
                    selection,
                    robots,
                    on_select: move |selection| on_action.call(LiveAction::Select(selection)),
                }
                JointPanel {
                    project, hardware, robot_selection: selection,
                    on_select: move |selection| on_action.call(LiveAction::Select(selection)),
                    on_point: move |selection| on_action.call(LiveAction::Point(selection)),
                }
            }
        }
    }
}

/// Where new episodes are saved.
///
/// Not a choice from the catalog: recording into a dataset that does not exist
/// yet is how most datasets start, so the operator types a name and the
/// catalog only offers what is already there as suggestions.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct CaptureDestination {
    chosen: Signal<String>,
    catalog: DatasetCatalog,
}

impl CaptureDestination {
    pub(super) fn use_destination() -> Self {
        Self {
            chosen: use_signal(|| "default".to_owned()),
            catalog: DatasetCatalog::use_catalog(),
        }
    }

    /// The dataset name new episodes are recorded into.
    pub(super) const fn chosen(self) -> Signal<String> {
        self.chosen
    }

    /// The catalog's dataset names, plus whatever is currently typed.
    fn suggestions(self) -> BTreeSet<String> {
        self.catalog
            .datasets()
            .into_iter()
            .filter_map(|dataset| {
                dataset
                    .selection()
                    .label()
                    .strip_prefix("dataset_")
                    .map(str::to_owned)
            })
            .chain(std::iter::once(self.chosen.read().to_owned()))
            .collect()
    }
}

#[component]
fn CaptureSection(capture: CaptureDestination, recording: Option<RecordingProgress>) -> Element {
    let mut selected_dataset = capture.chosen;
    let dataset_ids = capture.suggestions();
    let catalog = capture.catalog;

    rsx! {
        section { class: "workbench-section capture-destination",
            header { class: "workbench-section-heading",
                h3 { "Capture destination" }
                if let Some(recording) = recording.as_ref() {
                    span { "Recording {recording.episode()}" }
                }
            }
            Field { label: "Dataset", layout: FieldLayout::Inline, class: "capture-dataset-field",
                TextInput {
                    list: "live-dataset-options",
                    value: selected_dataset.read().to_owned(),
                    disabled: recording.is_some(),
                    maxlength: "96",
                    pattern: "[A-Za-z0-9_.-]+",
                    placeholder: "pick-place-v1",
                    oninput: move |value| selected_dataset.set(value),
                }
            }
            datalist { id: "live-dataset-options",
                for id in dataset_ids { option { value: "{id}" } }
            }
            if let Some(problem) = catalog.problem() {
                div { class: "min-w-0 rounded bg-danger-soft px-2 py-1.5 text-[11px] text-danger", role: "status",
                    strong { class: "block font-medium", "Could not load datasets" }
                    div { class: "break-words", "{problem}" }
                    div { class: "mt-1", "{problem.recovery_guidance()}" }
                    Button {
                        purpose: ActionPurpose::Retry,
                        variant: ButtonVariant::Outline,
                        class: "mt-1",
                        aria_label: "Retry loading datasets",
                        onclick: move |_| catalog.reread(),
                        "Retry"
                    }
                }
            } else if catalog.reading() {
                div { class: "text-[11px] text-ink-faint", role: "status", "Loading datasets…" }
            }
            p {
                if recording.is_some() {
                    "Dataset assignment is locked until this episode finishes."
                } else {
                    "New episodes are saved to this dataset."
                }
            }
        }
    }
}

#[component]
fn RobotRoster(
    selection: RobotSelection,
    robots: Vec<spiral_robotics_common::robots::RobotSummary>,
    on_select: EventHandler<Option<Selection>>,
) -> Element {
    let focused = selection
        .focused(&robots)
        .map(|robot| robot.id().to_owned());
    let empty = robots.is_empty();

    rsx! {
        section { class: "live-robot-roster", role: "tablist", "aria-label": "Robot arms",
            for robot in robots {
                {
                    let id = robot.id().to_owned();
                    let selected = focused.as_deref() == Some(robot.id());
                    let ready = robot.connection().connected() && robot.calibrated() && robot.problem().is_none();
                    let role = robot.robot_type().map(str::role_label).unwrap_or("arm");
                    let show_role = !robot.id().eq_ignore_ascii_case(role);
                    let state = if robot.problem().is_some() {
                        "Needs attention"
                    } else if !robot.connection().connected() {
                        "Disconnected"
                    } else if !robot.calibrated() {
                        "Calibration needed"
                    } else {
                        "Connected and calibrated"
                    };
                    let accessible_label = if show_role {
                        format!("{}, {role}, {state}", robot.id())
                    } else {
                        format!("{}, {state}", robot.id())
                    };
                    let accent =
                        crate::panels::scene::ScenePalette::accent_css(
                            crate::panels::scene::ScenePalette::robot_index(&robot),
                        );
                    rsx! {
                        Button {
                            key: "{robot.id()}",
                            purpose: ActionPurpose::Select,
                            variant: ButtonVariant::Row,
                            class: "live-robot-row",
                            role: "tab",
                            aria_pressed: selected,
                            aria_selected: selected,
                            aria_label: "{accessible_label}",
                            title: "{state}",
                            onclick: move |_| on_select.call(Some(Selection::Robot(id.clone()))),
                            span {
                                class: "live-robot-accent",
                                style: "background: {accent}",
                            }
                            span { class: "live-robot-copy",
                                strong { "{robot.id()}" }
                                if show_role { small { "{role}" } }
                            }
                            span {
                                class: if ready { "live-robot-state live-robot-state-ready" } else { "live-robot-state" },
                                "aria-hidden": "true",
                            }
                        }
                    }
                }
            }
            if empty {
                p { class: "workbench-empty", "Add an arm in Settings to begin." }
            }
        }
    }
}

#[component]
fn ControlSection(
    project: StudioProjectState,
    hardware: HardwareViewState,
    selection: RobotSelection,
    on_action: EventHandler<LiveAction>,
) -> Element {
    let focused = selection.focused(hardware.snapshot().read().robots());
    let telemetry = focused
        .as_ref()
        .and_then(|robot| hardware.robot_telemetry(robot.id()));
    let holding = telemetry.as_ref().is_some_and(|robot| robot.torque());
    let teleop =
        matches!(hardware.mode(), ModeSelection::Teleoperate { .. }) && hardware.mode_active();
    let pair = hardware.snapshot().read().default_teleoperation_pair();
    let mut confirm_release = use_signal(|| false);

    rsx! {
        section { class: "workbench-section control-section",
            header { class: "workbench-section-heading",
                h3 { "Control" }
                if teleop { span { "Leader → follower" } }
            }
            ModeGroup { class: "control-segmented", label: "Motor control mode",
                ModeButton {
                    purpose: ActionPurpose::Hold,
                    checked: holding && !teleop,
                    class: "control-segment",
                    disabled: focused.is_none() || telemetry.is_none(),
                    onclick: {
                        let id = focused.as_ref().map(|robot| robot.id().to_owned());
                        move |_| if (!holding || teleop)
                            && let Some(id) = id.clone()
                        {
                            on_action.call(LiveAction::Hold(id));
                        }
                    },
                    "Hold"
                }
                ModeButton {
                    id: "motor-mode-release",
                    purpose: ActionPurpose::Release,
                    checked: !holding && !teleop,
                    class: "control-segment",
                    aria_label: "Release motor torque",
                    disabled: focused.is_none() || telemetry.is_none(),
                    onclick: move |_| if holding || teleop { confirm_release.set(true) },
                    "Release"
                }
                ModeButton {
                    purpose: ActionPurpose::Teleoperate,
                    checked: teleop,
                    class: "control-segment",
                    disabled: pair.is_none() || !hardware.controls_available(),
                    onclick: move |_| {
                        if !teleop && let Some((leader, follower)) = pair.clone() {
                            on_action.call(LiveAction::Teleoperate { leader, follower });
                        }
                    },
                    "Teleop"
                }
            }
            if confirm_release() {
                BlockingConfirmation {
                    id: "release-arm-confirmation",
                    title: focused.as_ref().map(|robot| format!("Release {}?", robot.id())).unwrap_or_else(|| "Release arm?".to_owned()),
                    consequence: "Motor torque will turn off immediately and the arm can fall under its own weight.".to_owned(),
                    copy: ConfirmationCopy::new(
                        "Support the arm before continuing.",
                        "Release arm",
                        "Keep holding",
                    ),
                    response: ConfirmationResponse::new(
                        ActionPurpose::Release,
                        {
                            let id = focused.as_ref().map(|robot| robot.id().to_owned());
                            move |_| {
                                confirm_release.set(false);
                                if let Some(id) = id.clone() { on_action.call(LiveAction::Release(id)); }
                            }
                        },
                        move |_| confirm_release.set(false),
                    ),
                    dismissal: ConfirmationDismissal::new("motor-mode-release"),
                }
            }
        }
    }
}

trait RobotRolePresentation {
    fn role_label(&self) -> &'static str;
}

impl RobotRolePresentation for str {
    fn role_label(&self) -> &'static str {
        let kind = self.to_ascii_lowercase();
        if kind.contains("follower") {
            "follower"
        } else if kind.contains("leader") {
            "leader"
        } else {
            "arm"
        }
    }
}
