//! Dockable scene panel adapting Studio state to the viewport renderer.
//!
//! Everything about *drawing* the scene — the camera, the WebGL, picking, the
//! gesture arithmetic, and turning a robot description into shapes — lives in
//! [`spiral_robotics_viewport`], which cannot see this crate. What is left here is the
//! part that is genuinely the studio's: which arms are on the bench, what their
//! motors are doing, and what the operator has picked.
//!
//! The seam is worth naming. The crate answers *where on the canvas is this
//! thing*; this module answers *what should be said about it*. That is why the
//! hover callout is Dioxus markup positioned from an anchor rather than text
//! drawn into the scene: the renderer never learns what a motor is called, and
//! the studio never learns what a projection matrix is.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_elements::geometry::WheelDelta;
use dioxus_elements::input_data::MouseButton;
use spiral_robotics_common::datasets::DatasetRobot;
use spiral_robotics_common::robots::{MotorCondition, RobotSummary};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};
use spiral_robotics_viewport::{
    Command, Condition, Frame as BenchFrame, FrameError, Interaction, Joint, Target,
    Viewport as Bench,
};
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

use crate::activities::live::recording_review::{
    EpisodePreview, TimelineArmSample, TimelineSample,
};
use crate::panels::recorded_source::{LoadState, RecordedSource};
use crate::robots::{HardwareViewState, RobotSelection};
use crate::robots::{Selection, visualization::ConfiguredModel};
use spiral_robotics_common::robots::visualization::{RobotVisualization, TelemetryCoordinate};

/// The colours arms are told apart by, as WebGL values and as CSS.
///
/// The description paints its own accent, but a scene with two identical arms
/// in it needs them to look different, so anything the file marks `accent` is
/// repainted per robot from this list.
///
/// These are the only saturated colours in the interface, and they earn it by
/// carrying identity: the dot beside an arm's name is the same colour as that
/// arm in the scene. They are stated once here in both forms rather than being
/// written into the stylesheet as well, so the two can never drift.
///
/// Orange comes last on purpose. The gripper jaws are orange in every arm —
/// that is the one part that opens, and it says so — so the usual pair of a
/// leader and a follower gets two colours that cannot be mistaken for it.
const ACCENTS: [([f32; 4], &str); 4] = [
    ([0.29, 0.56, 0.85, 1.0], "#4a90d9"),
    ([0.60, 0.43, 0.84, 1.0], "#9a6dd7"),
    ([0.30, 0.67, 0.60, 1.0], "#4dab9a"),
    ([0.88, 0.53, 0.18, 1.0], "#e0872f"),
];

pub(crate) struct ScenePalette;

impl ScenePalette {
    pub(crate) fn robot_index(robot: &RobotSummary) -> usize {
        match robot.robot_type() {
            Some(kind) if kind.contains("follower") => 0,
            Some(kind) if kind.contains("leader") => 1,
            _ => {
                let hash = robot.id().bytes().fold(0usize, |hash, byte| {
                    hash.wrapping_mul(31).wrapping_add(usize::from(byte))
                });
                2 + hash % 2
            }
        }
    }

    /// The accent for the arm at `index` in the workspace.
    pub(crate) fn accent_css(index: usize) -> &'static str {
        ACCENTS[index % ACCENTS.len()].1
    }
}

/// What the pointer is over, and where to put a note about it.
#[derive(Clone, PartialEq)]
struct Hover {
    entity: Target,
    /// Where the callout points, in client pixels.
    at: (f64, f64),
}

/// The scene's data boundary. Recorded scenes never fall through to live
/// telemetry while their episode is loading or unavailable.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SceneSource {
    Live,
    Recorded(RecordedSource<TimelineSample>),
    Dataset {
        robot: Rc<DatasetRobot>,
        positions: Option<BTreeMap<String, f32>>,
    },
}

impl SceneSource {
    /// Missing or mismatched recorded data stays loading; it never becomes live telemetry.
    pub(crate) fn recorded(episode: &str, source: Option<&RecordedSource<EpisodePreview>>) -> Self {
        let source = source
            .filter(|source| source.episode() == episode)
            .cloned()
            .map(|source| source.map(EpisodePreview::into_sample))
            .unwrap_or_else(|| {
                RecordedSource::new(episode, 0.0, LoadState::Loading { previous: None })
            });
        Self::Recorded(source)
    }

    fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }

    fn sample(&self) -> Option<&TimelineSample> {
        match self {
            Self::Live | Self::Dataset { .. } => None,
            Self::Recorded(source) => source.renderable(),
        }
    }

    fn recorded_status(&self) -> Option<(bool, Option<&str>)> {
        match self {
            Self::Live => None,
            Self::Recorded(source) => Some((source.state().is_loading(), source.state().reason())),
            Self::Dataset { positions, .. } => positions
                .is_none()
                .then_some((false, Some("No joint pose at this time."))),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ViewportPresentation {
    visible_robot: Option<String>,
}

impl ViewportPresentation {
    pub(crate) const fn new(visible_robot: Option<String>) -> Self {
        Self { visible_robot }
    }
}

#[component]
pub(crate) fn Viewport(
    telemetry: Option<HardwareViewState>,
    robot_selection: RobotSelection,
    source: SceneSource,
    presentation: ViewportPresentation,
) -> Element {
    let ViewportPresentation { visible_robot } = presentation;
    let mut selection = robot_selection.selected();
    let on_select = use_callback(move |next| selection.set(next));
    let canvas_dom = use_hook(ViewportDom::new);
    let bench = use_hook(|| Rc::new(RefCell::new(Option::<Bench>::None)));
    let mut view_revision = use_signal(|| 0_u64);
    let mut dragging = use_signal(|| false);
    let mut trouble = use_signal(|| Option::<String>::None);
    let mut hover = use_signal(|| Option::<Hover>::None);
    let mut condition_marks = use_signal(Vec::<Mark>::new);
    // On by default: the bench is a physical workspace and the 20cm rhythm is
    // how an operator reads reach and clearance off the screen. It stays a
    // control, so a scene that only needs the arms can still shed it.
    let mut show_grid = use_signal(|| true);
    let panel = dioxus_workbench::PanelContext::current();
    let activity = crate::shell::activity::WorkspaceActivity::current();

    let dataset_robot = match &source {
        SceneSource::Dataset { robot, .. } => Some(robot.clone()),
        _ => None,
    };
    let models = use_memo(use_reactive(&dataset_robot, move |dataset_robot| {
        if let Some(robot) = dataset_robot {
            return ConfiguredModel::parse(robot.urdf(), robot.meshes()).map(|model| {
                BTreeMap::from([(
                    "recorded".into(),
                    ConfiguredModel {
                        model,
                        configuration: None,
                    },
                )])
            });
        }
        telemetry
            .map(|telemetry| {
                telemetry
                    .snapshot()
                    .read()
                    .robots()
                    .iter()
                    .filter_map(|robot| {
                        let document = robot.document()?;
                        match document.visualization() {
                            Ok(None) => None,
                            Ok(Some(configuration)) => Some(
                                ConfiguredModel::new(configuration)
                                    .map(|model| (robot.id().to_owned(), model)),
                            ),
                            Err(error) => Some(Err(error)),
                        }
                    })
                    .collect::<Result<BTreeMap<_, _>, String>>()
            })
            .unwrap_or_else(|| Ok(BTreeMap::new()))
    }));

    let draw_bench = bench.clone();
    use_effect(use_reactive(
        (&source, &visible_robot),
        move |(draw_source, visible_robot)| {
            let geometry = panel.geometry();
            if !geometry.visible()
                || !telemetry.map_or_else(|| activity.is_active(), HardwareViewState::is_active)
            {
                return;
            }
            let _ = view_revision();
            let grid = show_grid();
            let hovered = hover();
            let conditions = if draw_source.is_live() {
                telemetry
                    .map(|telemetry| telemetry.motor_conditions())
                    .unwrap_or_default()
            } else {
                BTreeMap::new()
            };
            let models = models.read();
            let models = match models.as_ref() {
                Ok(models) => models,
                Err(error) => {
                    trouble.set(Some(error.clone()));
                    return;
                }
            };
            let frame = match (SceneBuildRequest {
                robots: &telemetry.map(SceneRobot::capture).unwrap_or_default(),
                selection: robot_selection,
                grid,
                source: &draw_source,
                hovered: hovered.as_ref().map(|it| &it.entity),
                visible_robot: visible_robot.as_deref(),
                conditions: &conditions,
            })
            .build(models)
            {
                Ok(frame) => frame,
                Err(error) => {
                    trouble.set(Some(error.to_string()));
                    condition_marks.set(Vec::new());
                    return;
                }
            };

            let Some(canvas) = canvas_dom.canvas() else {
                return;
            };
            let mut slot = draw_bench.borrow_mut();
            if slot.is_none() {
                match Bench::new(canvas) {
                    Ok(ready) => *slot = Some(ready),
                    Err(error) => {
                        trouble.set(Some(error.to_string()));
                        return;
                    }
                }
            }
            let Some(bench) = slot.as_mut() else {
                return;
            };
            let ratio = web_sys::window().map_or(1.0, |window| window.device_pixel_ratio());
            if let Err(error) = bench.draw(&frame, ratio) {
                trouble.set(Some(error.to_string()));
                return;
            }
            condition_marks.set(if draw_source.is_live() {
                bench.marks_for(&conditions)
            } else {
                Vec::new()
            });
            if trouble.peek().is_some() {
                trouble.set(None);
            }
        },
    ));

    let arms = telemetry
        .map(|telemetry| telemetry.snapshot().read().robots().to_vec())
        .unwrap_or_default()
        .iter()
        .filter(|robot| {
            visible_robot
                .as_ref()
                .is_none_or(|visible| robot.id() == visible)
        })
        .cloned()
        .collect::<Vec<_>>();
    let failure = trouble();
    // Where each unwell motor sits on the canvas right now. Computed from the
    // scene the crate just drew, so a mark cannot drift away from the part it
    // is about as the camera moves.
    let marks = condition_marks();
    let recorded_active = !source.is_live();
    let recorded_status = source.recorded_status();

    rsx! {
        div { class: "relative h-full w-full overflow-hidden bg-viewport",
            canvas {
                id: canvas_dom.id(),
                class: "h-full w-full select-none",
                class: if dragging() {
                    "cursor-grabbing"
                } else if hover().is_some() {
                    "cursor-pointer"
                } else {
                    "cursor-grab"
                },
                role: "application",
                tabindex: "0",
                "aria-label": "Interactive 3D robot viewer. Click a motor or camera to select it and see its readings in the inspector. Drag to orbit, shift-drag to pan, scroll to zoom, and press F to frame the selection.",
                onpointerdown: {
                    let bench = bench.clone();
                    move |event| {
                        ViewportDom::capture_pointer(&event);
                        // The middle and right buttons pan, matching every other 3D
                        // viewer; shift with the left button does the same for a
                        // one-button mouse.
                        let panning = event.modifiers().shift()
                            || matches!(
                                event.trigger_button(),
                                Some(MouseButton::Secondary | MouseButton::Auxiliary)
                            );
                        if let Ok(mut slot) = bench.try_borrow_mut()
                            && let Some(bench) = slot.as_mut()
                        {
                            bench.begin_drag(event.client_point(), panning);
                            dragging.set(true);
                        }
                    }
                },
                onpointermove: {
                    let bench = bench.clone();
                    move |event| {
                        let position = event.client_point();
                        if dragging() {
                            if let Ok(mut slot) = bench.try_borrow_mut()
                                && slot.as_mut().is_some_and(|bench| bench.drag_to(position))
                            {
                                view_revision.bump();
                            }
                            return;
                        }
                        // Only while nothing is being dragged: an orbit is not
                        // a tour of everything the pointer sweeps past.
                        hover.set(HoverRequest { bench: &bench, at: position, canvas: canvas_dom }.pick());
                    }
                },
                onpointerup: {
                    let bench = bench.clone();
                    move |event| {
                        let position = event.client_point();
                        let (was_click, picked) = bench
                            .try_borrow_mut()
                            .ok()
                            .and_then(|mut slot| {
                                let bench = slot.as_mut()?;
                                let was_click = !bench.drag_moved();
                                let picked = was_click.then(|| bench.pick(position)).flatten();
                                bench.end_drag();
                                Some((was_click, picked))
                            })
                            .unwrap_or_default();
                        if was_click {
                            // Clicking bare bench clears the selection, the way
                            // clicking empty space does everywhere else.
                            on_select.call(picked.as_ref().map(Selection::from));
                        }
                        dragging.set(false);
                    }
                },
                onpointercancel: {
                    let bench = bench.clone();
                    move |_| {
                        if let Ok(mut slot) = bench.try_borrow_mut()
                            && let Some(bench) = slot.as_mut()
                        {
                            bench.end_drag();
                        }
                        dragging.set(false);
                    }
                },
                onpointerleave: {
                    let bench = bench.clone();
                    move |_| {
                        if let Ok(mut slot) = bench.try_borrow_mut()
                            && let Some(bench) = slot.as_mut()
                        {
                            bench.end_drag();
                        }
                        dragging.set(false);
                        hover.set(None);
                    }
                },
                oncontextmenu: move |event| event.prevent_default(),
                onwheel: {
                    let bench = bench.clone();
                    move |event| {
                        let notches = match event.delta() {
                            WheelDelta::Pixels(delta) => delta.y / 120.0,
                            WheelDelta::Lines(delta) => delta.y / 3.0,
                            WheelDelta::Pages(delta) => delta.y,
                        };
                        if let Ok(mut slot) = bench.try_borrow_mut()
                            && let Some(bench) = slot.as_mut()
                        {
                            bench.command(Command::Wheel(notches));
                            view_revision.bump();
                        }
                    }
                },
                onkeydown: {
                    let bench = bench.clone();
                    move |event| {
                        let command = match event.key() {
                        Key::ArrowLeft => Some(Command::OrbitLeft),
                        Key::ArrowRight => Some(Command::OrbitRight),
                        Key::ArrowUp => Some(Command::OrbitUp),
                        Key::ArrowDown => Some(Command::OrbitDown),
                        Key::Character(value) if value == "+" || value == "=" => {
                            Some(Command::ZoomIn)
                        }
                        Key::Character(value) if value == "-" || value == "_" => {
                            Some(Command::ZoomOut)
                        }
                        Key::Character(value) if value == "0" => Some(Command::Home),
                        Key::Character(value) if value.eq_ignore_ascii_case("f") => {
                            let robot = telemetry.and_then(|telemetry| robot_selection.framed_robot(visible_robot.as_deref(), telemetry.snapshot().read().robots()));
                            if let Ok(mut slot) = bench.try_borrow_mut()
                                && let Some(bench) = slot.as_mut()
                                && bench.frame_robot(robot.as_deref())
                            {
                                view_revision.bump();
                            }
                            None
                        }
                        _ => None,
                        };
                        if let Some(command) = command
                            && let Ok(mut slot) = bench.try_borrow_mut()
                            && let Some(bench) = slot.as_mut()
                        {
                            bench.command(command);
                            view_revision.bump();
                        }
                    }
                },
            }

            if let Some(failure) = failure {
                div { class: "absolute inset-0 flex items-center justify-center p-4",
                    p { class: "max-w-sm rounded border border-line bg-surface px-3 py-2 text-center text-[12px] text-ink-soft",
                        "{failure}"
                    }
                }
            } else {
                // A motor reporting trouble marks itself where it is, because
                // "which one" is the question the inspector's arm-level cells
                // cannot answer and the bench can.
                for mark in marks {
                    span {
                        key: "{mark.0}",
                        class: if mark.3 { "bench-mark bench-mark-fault" } else { "bench-mark" },
                        style: "left: {mark.1}px; top: {mark.2}px",
                        "aria-hidden": true,
                    }
                }
                if let Some(hovered) = hover() {
                    HoverNote { telemetry, hover: hovered, episode_preview: recorded_active }
                }
                Legend { selection, arms, on_select }
                if let Some(sample) = source.sample() {
                    div {
                        class: "pointer-events-none absolute left-1/2 top-2 z-20 -translate-x-1/2 rounded-sm border border-line bg-surface px-2 py-1 font-mono text-[10px] tabular-nums text-ink-soft",
                        "History · {sample.seconds().history_time()}"
                    }
                } else if let Some((loading, reason)) = recorded_status {
                    // The chip says which of the two things happened and stops
                    // there. Appending the backend's reason inline grew a 20px
                    // status marker into a ten-line block of vendor text hanging
                    // over the middle of the stage, hiding the very scene it was
                    // reporting on. The reason is one hover away, and it is
                    // already in the status bar and the notification centre.
                    div {
                        class: "scene-source-note",
                        title: reason.unwrap_or_default(),
                        if loading { "Loading recorded scene…" } else { "Recorded scene unavailable" }
                    }
                }
                div { class: "viewport-help",
                    "click select · drag orbit · shift-drag pan · scroll zoom · F frame"
                }
                // One group rather than two absolutely positioned buttons, the
                // second of which was pushed off the first by a hard-coded 64px
                // that only held while "Home" was exactly that wide.
                div { class: "viewport-controls",
                    Button {
                        purpose: ActionPurpose::Toggle,
                        class: "viewport-action",
                        aria_pressed: show_grid(),
                        onclick: move |_| show_grid.toggle(),
                        title: "Show or hide the scale grid",
                        "Grid"
                    }
                    Button {
                        purpose: ActionPurpose::Apply,
                        class: "viewport-action",
                        onclick: {
                            let bench = bench.clone();
                            move |_| {
                                if let Ok(mut slot) = bench.try_borrow_mut()
                                    && let Some(bench) = slot.as_mut()
                                {
                                    bench.command(Command::Home);
                                    view_revision.bump();
                                }
                            }
                        },
                        title: "Restore the default camera (0)",
                        "Home"
                    }
                }
                ViewCube {
                    on_command: {
                        let bench = bench.clone();
                        move |command| {
                            if let Ok(mut slot) = bench.try_borrow_mut()
                                && let Some(bench) = slot.as_mut()
                            {
                                bench.command(command);
                                view_revision.bump();
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The one fact worth putting under the pointer.
///
/// A hover is a glance, so it carries a name and the single reading that would
/// make an operator stop: a temperature when the motor is unwell, and otherwise
/// nothing but the name. Everything else is in the inspector, which is where
/// the operator is going next anyway — and which stays reachable without a
/// pointer at all.
#[component]
fn HoverNote(telemetry: Option<HardwareViewState>, hover: Hover, episode_preview: bool) -> Element {
    let (title, detail) = hover.entity.describe(telemetry, episode_preview);
    rsx! {
        div {
            class: "bench-note",
            style: "left: {hover.at.0}px; top: {hover.at.1}px",
            "aria-hidden": true,
            span { class: "bench-note-title", "{title}" }
            if let Some(detail) = detail {
                span { class: "bench-note-detail", "{detail}" }
            }
        }
    }
}

/// What to call an entity, and the reading worth showing beside its name.
trait TargetPresentation {
    fn describe(
        &self,
        telemetry: Option<HardwareViewState>,
        episode_preview: bool,
    ) -> (String, Option<String>);
}

impl TargetPresentation for Target {
    fn describe(
        &self,
        telemetry: Option<HardwareViewState>,
        episode_preview: bool,
    ) -> (String, Option<String>) {
        match self {
            Target::Robot { robot } => (robot.clone(), None),
            Target::Camera { robot, role } => (format!("{role} camera"), Some(robot.clone())),
            Target::Joint { robot, joint } => {
                let label = Joint::named(joint)
                    .map_or_else(|| joint.clone(), |joint| joint.label().to_owned());
                let telemetry = telemetry.and_then(|telemetry| telemetry.robot_telemetry(robot));
                let detail = (!episode_preview)
                    .then(|| {
                        telemetry.as_ref().and_then(|telemetry| {
                            telemetry
                                .joints()
                                .iter()
                                .find(|candidate| candidate.name() == *joint)
                                .and_then(|joint| joint.temperature_celsius())
                        })
                    })
                    .flatten()
                    .map(|celsius| format!("{celsius:.0} °C"));
                (label, detail)
            }
        }
    }
}

/// Turn a scene entity into the studio's own idea of what is selected.
impl From<&Target> for Selection {
    fn from(target: &Target) -> Self {
        match target {
            Target::Joint { robot, joint } => Selection::Joint {
                robot: robot.clone(),
                joint: joint.clone(),
            },
            Target::Robot { robot } => Selection::Robot(robot.clone()),
            Target::Camera { robot, role } => Selection::Camera {
                robot: robot.clone(),
                role: role.clone(),
            },
        }
    }
}

/// Scene construction receives a sampled model, without project or command access.
struct SceneRobot {
    configuration: RobotSummary,
    telemetry: Option<spiral_robotics_common::robots::RobotTelemetry>,
    accent: usize,
}
impl SceneRobot {
    fn capture(telemetry: HardwareViewState) -> Vec<Self> {
        telemetry
            .snapshot()
            .read()
            .robots()
            .iter()
            .map(|robot| Self {
                configuration: robot.clone(),
                telemetry: telemetry.robot_telemetry(robot.id()),
                accent: ScenePalette::robot_index(robot),
            })
            .collect()
    }
}

/// Build the bench from what the studio currently knows.
///
/// Selection, hover, and hardware verdicts remain application policy; the
/// viewport only receives their presentation states for semantic targets.
struct SceneBuildRequest<'a> {
    robots: &'a [SceneRobot],
    selection: RobotSelection,
    grid: bool,
    source: &'a SceneSource,
    hovered: Option<&'a Target>,
    visible_robot: Option<&'a str>,
    conditions: &'a BTreeMap<(String, String), Condition>,
}

impl SceneBuildRequest<'_> {
    fn build(&self, models: &BTreeMap<String, ConfiguredModel>) -> Result<BenchFrame, FrameError> {
        let selected = self.selection.selected().read().to_owned();
        // Hovering a joint row in the inspector lights the motor on the bench, the
        // same way hovering the motor lights the row: one object, seen twice.
        let pointed = self.selection.pointed().read().to_owned();
        let mut frame = BenchFrame::new(self.grid);
        self.add_arms(&mut frame, models)?;

        if let Some(selected) = selected.as_ref().map(Target::from) {
            frame.set_interaction(selected, Interaction::Selected);
        }
        if let Some(pointed) = pointed.as_ref().map(Target::from) {
            frame.set_interaction(pointed, Interaction::Hovered);
        }
        if let Some(hovered) = self.hovered {
            frame.set_interaction(hovered.clone(), Interaction::Hovered);
        }
        for ((robot, joint), condition) in self.conditions {
            frame.set_condition(
                Target::Joint {
                    robot: robot.clone(),
                    joint: joint.clone(),
                },
                *condition,
            );
        }
        Ok(frame)
    }

    fn add_arms(
        &self,
        frame: &mut BenchFrame,
        models: &BTreeMap<String, ConfiguredModel>,
    ) -> Result<(), FrameError> {
        if let SceneSource::Dataset { positions, .. } = self.source {
            if let Some(positions) = positions {
                frame.add_arm(
                    "recorded",
                    spiral_robotics_viewport::ArmPresentation::new(
                        models["recorded"].model.clone(),
                        positions.clone(),
                    ),
                )?;
            }
            return Ok(());
        }
        for robot in self.robots.iter().filter(|robot| {
            self.visible_robot
                .is_none_or(|visible| robot.configuration.id() == visible)
        }) {
            let arm = &robot.configuration;
            let Some(model) = models.get(arm.id()) else {
                continue;
            };
            let configuration = model
                .configuration
                .as_ref()
                .expect("project robot configuration");
            let recorded = self
                .source
                .sample()
                .and_then(|sample| sample.arms().iter().find(|sample| sample.arm() == arm.id()))
                .and_then(|sample| sample.recorded_positions(configuration));
            let telemetry = self
                .source
                .is_live()
                .then_some(robot.telemetry.as_ref())
                .flatten()
                .filter(|_| arm.calibrated());
            let tracked = recorded.is_some() || telemetry.is_some();
            let positions = recorded.unwrap_or_else(|| {
                model.measured_positions(telemetry.map(|robot| robot.joints()).unwrap_or_default())
            });
            frame.add_arm(
                arm.id(),
                spiral_robotics_viewport::ArmPresentation::new(model.model.clone(), positions)
                    .with_appearance(ACCENTS[robot.accent % ACCENTS.len()].0, tracked),
            )?;
            for camera in arm.cameras() {
                camera.add_to(frame, arm.id())?;
            }
        }
        Ok(())
    }
}

impl From<&Selection> for Target {
    fn from(selection: &Selection) -> Self {
        match selection {
            Selection::Robot(robot) => Self::Robot {
                robot: robot.clone(),
            },
            Selection::Joint { robot, joint } => Self::Joint {
                robot: robot.clone(),
                joint: joint.clone(),
            },
            Selection::Camera { robot, role } => Self::Camera {
                robot: robot.clone(),
                role: role.clone(),
            },
        }
    }
}

/// Every hardware-owned motor verdict in the current frame.
impl HardwareViewState {
    fn motor_conditions(self) -> BTreeMap<(String, String), Condition> {
        let telemetry_signal = self.frame();
        let telemetry = telemetry_signal.read();
        let mut conditions = BTreeMap::new();
        for robot in telemetry.robots() {
            for joint in robot.joints() {
                let condition = match joint.condition() {
                    MotorCondition::Nominal => Condition::Nominal,
                    MotorCondition::Caution => Condition::Caution,
                    MotorCondition::Fault => Condition::Fault,
                };
                if condition != Condition::Nominal {
                    conditions.insert((robot.id().to_owned(), joint.name().to_owned()), condition);
                }
            }
        }
        conditions
    }
}

/// A key, its place on the canvas, and whether it is a fault rather than a
/// caution — one entry per motor currently reporting trouble.
type Mark = (String, f64, f64, bool);

trait BenchMarks {
    fn marks_for(&self, conditions: &BTreeMap<(String, String), Condition>) -> Vec<Mark>;
}

impl BenchMarks for Bench {
    fn marks_for(&self, conditions: &BTreeMap<(String, String), Condition>) -> Vec<Mark> {
        conditions
            .iter()
            .filter_map(|((robot, joint), condition)| {
                let target = Target::Joint {
                    robot: robot.clone(),
                    joint: joint.clone(),
                };
                let (x, y) = self.anchor(&target)?;
                Some((
                    format!("{robot}/{joint}"),
                    x,
                    y,
                    *condition == Condition::Fault,
                ))
            })
            .collect()
    }
}

struct HoverRequest<'a> {
    canvas: ViewportDom,
    bench: &'a Rc<RefCell<Option<Bench>>>,
    at: (f64, f64),
}

impl HoverRequest<'_> {
    fn pick(&self) -> Option<Hover> {
        let slot = self.bench.try_borrow().ok()?;
        let bench = slot.as_ref()?;
        let target = bench.pick(self.at)?;
        let rect = self.canvas.canvas()?.get_bounding_client_rect();
        Some(Hover {
            entity: target,
            at: (self.at.0 - rect.left(), self.at.1 - rect.top()),
        })
    }
}

/// Project recorded telemetry through the same configured coordinates as live data.
trait RecordedRobotPose {
    fn recorded_positions(
        &self,
        configuration: &RobotVisualization,
    ) -> Option<BTreeMap<String, f32>>;
}
impl RecordedRobotPose for TimelineArmSample {
    fn recorded_positions(
        &self,
        configuration: &RobotVisualization,
    ) -> Option<BTreeMap<String, f32>> {
        let mut observed = false;
        let positions = configuration.positions(|name, input| {
            let value = match input {
                TelemetryCoordinate::Degrees => self.degrees(),
                TelemetryCoordinate::Normalized => self.normalized(),
            }
            .get(name)
            .copied()
            .flatten()
            .filter(|value| value.is_finite());
            observed |= value.is_some();
            value
        });
        observed.then_some(positions)
    }
}

trait RecordedTimePresentation {
    fn history_time(self) -> String;
}

impl RecordedTimePresentation for f64 {
    fn history_time(self) -> String {
        let seconds = self.max(0.0);
        let minutes = (seconds / 60.0).floor() as u64;
        format!("{minutes}:{:04.1}", seconds - minutes as f64 * 60.0)
    }
}

/// Where a bound camera sits, for the ones whose file says.
trait CameraBindingPresentation {
    fn add_to(&self, frame: &mut BenchFrame, robot: &str) -> Result<(), FrameError>;
}

impl CameraBindingPresentation for spiral_robotics_common::robots::CameraBinding {
    fn add_to(&self, frame: &mut BenchFrame, robot: &str) -> Result<(), FrameError> {
        let camera_mount = self.mount();
        let Some(mount) = camera_mount.as_ref() else {
            return Ok(());
        };
        frame
            .mount_camera(
                robot,
                spiral_robotics_viewport::CameraMount::new(
                    self.name().to_owned(),
                    spiral_robotics_viewport::CameraAttachment::new(
                        mount.frame(),
                        mount.xyz(),
                        mount.rpy(),
                    ),
                    mount.fov_degrees(),
                ),
            )
            .map(|_| ())
    }
}

/// Which arm is which colour in the scene.
#[component]
fn Legend(
    selection: Signal<Option<Selection>>,
    arms: Vec<RobotSummary>,
    on_select: EventHandler<Option<Selection>>,
) -> Element {
    if arms.is_empty() {
        return rsx! {
            div { class: "pointer-events-none absolute inset-0 flex items-center justify-center",
                p { class: "text-[12px] text-ink-faint", "Add an arm to see it here." }
            }
        };
    }

    rsx! {
        ul { class: "viewport-legend",
            for (index, arm) in arms.into_iter().enumerate() {
                li { key: "{arm.id()}",
                    Button {
                        purpose: ActionPurpose::Select,
                        variant: ButtonVariant::Row,
                        class: "viewport-legend-row",
                        aria_pressed: selection.read().as_ref().is_some_and(|selection| selection.matches_robot(arm.id())),
                        title: "Select {arm.id()}; press F to frame it",
                        onclick: {
                            let id = arm.id().to_owned();
                            move |_| on_select.call(Some(Selection::Robot(id.clone())))
                        },
                        span { class: "h-1.5 w-1.5 shrink-0 rounded-full", style: "background: {ScenePalette::accent_css(index)}" }
                        span { "{arm.id()}" }
                        if !arm.connection().connected() { span { class: "text-ink-faint", "offline" } }
                    }
                }
            }
        }
    }
}

/// A visible camera-state control: its faces both describe and change the view.
#[component]
fn ViewCube(on_command: EventHandler<Command>) -> Element {
    rsx! {
        div { class: "view-cube", role: "group", "aria-label": "Camera orientation",
            Button {
                purpose: ActionPurpose::Apply,
                title: "Top view", aria_label: "Top view",
                onclick: move |_| on_command.call(Command::Top),
                "Top"
            }
            // Spelled out rather than abbreviated: `F` really is a keyboard
            // shortcut here, so a neighbouring cap reading `R` invited an
            // operator to press the key that starts a recording on live
            // hardware.
            div { class: "view-cube-row",
                Button {
                    purpose: ActionPurpose::Apply,
                    title: "Front view", aria_label: "Front view",
                    onclick: move |_| on_command.call(Command::Front),
                    "Front"
                }
                Button {
                    purpose: ActionPurpose::Apply,
                    title: "Right side view", aria_label: "Right side view",
                    onclick: move |_| on_command.call(Command::Right),
                    "Right"
                }
            }
            // The axis key belongs to the cube, not to a seventh box floating
            // above it: both answer "which way is the scene facing".
            div { class: "viewport-axis-key",
                span { class: "axis-x", "X" }
                span { class: "axis-y", "Y" }
                span { class: "axis-z", "Z" }
            }
        }
    }
}

impl RobotSelection {
    fn framed_robot(self, visible_robot: Option<&str>, robots: &[RobotSummary]) -> Option<String> {
        visible_robot.map(str::to_owned).or_else(|| {
            self.selected()
                .read()
                .as_ref()
                .map(|selection| selection.robot_id().to_owned())
                .or_else(|| self.focused(robots).map(|robot| robot.id().to_owned()))
        })
    }
}

trait RevisionSignal {
    fn bump(&mut self);
}

impl RevisionSignal for Signal<u64> {
    fn bump(&mut self) {
        let next = self.peek().wrapping_add(1);
        self.set(next);
    }
}

trait PointerPosition {
    fn client_point(&self) -> (f64, f64);
}

impl PointerPosition for Event<PointerData> {
    fn client_point(&self) -> (f64, f64) {
        let position = self.client_coordinates();
        (position.x, position.y)
    }
}

#[derive(Clone, Copy)]
struct ViewportDom {
    scope: ScopeId,
}

impl ViewportDom {
    fn new() -> Self {
        Self {
            scope: dioxus::dioxus_core::current_scope_id(),
        }
    }

    fn id(self) -> String {
        format!("robot-viewport-{}", self.scope.0)
    }

    /// Keep receiving moves after the pointer leaves the canvas, so a drag that
    /// runs off the edge of a panel keeps turning the robot.
    fn capture_pointer(event: &Event<PointerData>) {
        let Some(raw) = event.downcast::<web_sys::PointerEvent>() else {
            return;
        };
        if let Some(target) = raw
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        {
            let _ = target.set_pointer_capture(raw.pointer_id());
        }
    }

    fn canvas(self) -> Option<HtmlCanvasElement> {
        web_sys::window()?
            .document()?
            .get_element_by_id(&self.id())?
            .dyn_into()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn recorded_pose_uses_backend_projected_degrees() {
        let sample = TimelineArmSample::new(
            "follower",
            BTreeMap::from([("hinge".to_owned(), Some(1.0))]),
            BTreeMap::from([("hinge".to_owned(), Some(90.0))]),
        );

        let configuration = serde_json::from_value(
            serde_json::json!({"urdf":"", "joints":{"hinge":{"input":"degrees", "scale":std::f64::consts::PI / 180.0, "offset":0.0, "rest":0.0}}}),
        )
        .unwrap();
        let pose = sample
            .recorded_positions(&configuration)
            .expect("episode_preview pose");
        assert!((pose["hinge"] - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn recorded_pose_does_not_guess_without_projected_degrees() {
        let sample = TimelineArmSample::new(
            "follower",
            BTreeMap::from([("hinge".to_owned(), None)]),
            BTreeMap::from([("hinge".to_owned(), None)]),
        );
        let configuration = serde_json::from_value(
            serde_json::json!({"urdf":"", "joints":{"hinge":{"input":"degrees", "scale":std::f64::consts::PI / 180.0, "offset":0.0, "rest":0.0}}}),
        )
        .unwrap();
        assert!(sample.recorded_positions(&configuration).is_none());
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_missing_recorded_scene_source_never_falls_back_to_live() {
        assert!(
            matches!(SceneSource::recorded("take-1", None), SceneSource::Recorded(source) if matches!(source.state(), LoadState::Loading { previous: None }))
        );
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod dataset_tests {
    use super::*;
    use spiral_robotics_viewport::Frame;
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn imported_robot_renders_and_moves_with_its_named_joint() {
        let document = web_sys::window().unwrap().document().unwrap();
        let element = document.create_element("canvas").unwrap();
        element
            .set_attribute("style", "width:256px;height:256px")
            .unwrap();
        document.body().unwrap().append_child(&element).unwrap();
        let canvas: web_sys::HtmlCanvasElement = element.clone().dyn_into().unwrap();
        let gl: web_sys::WebGl2RenderingContext = canvas
            .get_context("webgl2")
            .unwrap()
            .unwrap()
            .dyn_into()
            .unwrap();
        let urdf = r#"<robot name="test"><link name="base"/><link name="arm"><visual><origin xyz="0.25 0 0.1"/><geometry><box size="0.5 0.07 0.07"/></geometry><material name="test"><color rgba="0.8 0.1 0.1 1"/></material></visual></link><joint name="recorded_hinge" type="continuous"><parent link="base"/><child link="arm"/><axis xyz="0 0 1"/></joint></robot>"#;
        let robot =
            serde_json::from_value(serde_json::json!({"urdf":urdf,"bindings":[],"meshes":{}}))
                .unwrap();
        let source = SceneSource::Dataset {
            robot: Rc::new(robot),
            positions: None,
        };
        assert!(!source.is_live());
        assert_eq!(
            source.recorded_status(),
            Some((false, Some("No joint pose at this time.")))
        );
        let model = match &source {
            SceneSource::Dataset { robot, .. } => {
                ConfiguredModel::parse(robot.urdf(), robot.meshes()).unwrap()
            }
            _ => unreachable!(),
        };
        let mut viewport = Bench::new(canvas).unwrap();
        let mut pictures = Vec::new();
        for (angle, home) in [(0.0, false), (1.0, false), (0.0, true)] {
            if home {
                viewport.command(Command::ZoomIn);
                viewport.command(Command::Home);
            }
            let mut frame = Frame::new(true);
            frame
                .add_arm(
                    "fixture",
                    spiral_robotics_viewport::ArmPresentation::new(
                        model.clone(),
                        BTreeMap::from([("recorded_hinge".into(), angle)]),
                    ),
                )
                .unwrap();
            assert!(viewport.draw(&frame, 1.0).unwrap());
            let mut pixels = vec![0; 256 * 256 * 4];
            gl.read_pixels_with_opt_u8_array(
                0,
                0,
                256,
                256,
                web_sys::WebGl2RenderingContext::RGBA,
                web_sys::WebGl2RenderingContext::UNSIGNED_BYTE,
                Some(&mut pixels),
            )
            .unwrap();
            assert_eq!(gl.get_error(), web_sys::WebGl2RenderingContext::NO_ERROR);
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[0] > 120 && pixel[1] < 100)
            );
            pictures.push(pixels);
        }
        assert_ne!(pictures[0], pictures[1]);
        assert_eq!(pictures[0], pictures[2]);
        element.remove();
    }
}
