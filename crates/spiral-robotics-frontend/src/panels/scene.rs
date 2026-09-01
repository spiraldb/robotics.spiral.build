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

use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use dioxus::prelude::*;
use dioxus_elements::geometry::WheelDelta;
use dioxus_elements::input_data::MouseButton;
use spiral_robotics_api::robots::{MotorCondition, RobotSummary};
use spiral_robotics_viewport::{
    ArmPose, Command, Condition, Frame as BenchFrame, FrameError, Interaction, Joint, Target,
    Viewport as Bench,
};
#[cfg(feature = "web")]
use wasm_bindgen::JsCast;
#[cfg(feature = "web")]
use wasm_bindgen::prelude::Closure;
use web_sys::HtmlCanvasElement;

use crate::client::episode_data::{TimelineArmSample, TimelineSample};
use crate::shell::activity::RecordedSource;
use crate::shell::state::{Selection, studio};

/// The canvas the viewport draws into. There is exactly one on the page.
const CANVAS_ID: &str = "robot-viewport";

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
}

impl SceneSource {
    fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }

    fn sample(&self) -> Option<&TimelineSample> {
        match self {
            Self::Live => None,
            Self::Recorded(source) => source.renderable(),
        }
    }

    fn recorded_status(&self) -> Option<(bool, Option<&str>)> {
        match self {
            Self::Live => None,
            Self::Recorded(source) => Some((source.state().is_loading(), source.state().reason())),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ViewportPresentation {
    visible_robot: Option<String>,
    resize_revision: u64,
}

impl ViewportPresentation {
    pub(crate) const fn new(visible_robot: Option<String>, resize_revision: u64) -> Self {
        Self {
            visible_robot,
            resize_revision,
        }
    }
}

#[component]
pub(crate) fn Viewport(
    source: SceneSource,
    presentation: ViewportPresentation,
    on_select: EventHandler<Option<Selection>>,
) -> Element {
    let ViewportPresentation {
        visible_robot,
        resize_revision,
    } = presentation;
    let studio = studio();
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
    // Bumped by the window resize listener: the draw below reads it purely so
    // that a resize redraws even when no telemetry is arriving.
    let resized = use_signal(|| 0_u32);
    #[cfg(feature = "web")]
    let mut resized = resized;

    // Held for the life of the component, and dropped — unsubscribing — with it.
    #[cfg(feature = "web")]
    use_hook(|| {
        Rc::new(WindowListener::new("resize", move || {
            let next = resized.peek().wrapping_add(1);
            resized.set(next);
        }))
    });

    let draw_bench = bench.clone();
    use_effect(use_reactive(
        (&resize_revision, &source, &visible_robot),
        move |(_resize_revision, draw_source, visible_robot)| {
            // A workbench divider changes this canvas without changing the
            // browser window. Its host increments the revision while dragging
            // so WebGL redraws against the canvas's new CSS bounds instead of
            // stretching the previous frame until the next telemetry or camera
            // interaction.
            let _ = resized();
            let _ = view_revision();
            let grid = show_grid();
            let hovered = hover();
            let conditions = if draw_source.is_live() {
                studio.motor_conditions()
            } else {
                BTreeMap::new()
            };
            let frame = match (SceneBuildRequest {
                studio: &studio,
                grid,
                source: &draw_source,
                hovered: hovered.as_ref().map(|it| &it.entity),
                visible_robot: visible_robot.as_deref(),
                conditions: &conditions,
            })
            .build()
            {
                Ok(frame) => frame,
                Err(error) => {
                    trouble.set(Some(error.to_string()));
                    condition_marks.set(Vec::new());
                    return;
                }
            };

            let Some(canvas) = ViewportDom::canvas() else {
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
            bench.draw(&frame, ratio);
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

    let arms = studio
        .snapshot()
        .read()
        .robots()
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
                id: CANVAS_ID,
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
                        hover.set(HoverRequest { bench: &bench, at: position }.pick());
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
                            let robot = studio.framed_robot(visible_robot.as_deref());
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
                    HoverNote { hover: hovered, episode_preview: recorded_active }
                }
                Legend { arms, on_select }
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
                    Action {
                        purpose: ActionPurpose::Toggle,
                        class: "viewport-action",
                        aria_pressed: show_grid(),
                        onclick: move |_| show_grid.toggle(),
                        title: "Show or hide the scale grid",
                        "Grid"
                    }
                    Action {
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
fn HoverNote(hover: Hover, episode_preview: bool) -> Element {
    let studio = studio();
    let (title, detail) = hover.entity.describe(&studio, episode_preview);
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
        studio: &crate::shell::state::Studio,
        episode_preview: bool,
    ) -> (String, Option<String>);
}

impl TargetPresentation for Target {
    fn describe(
        &self,
        studio: &crate::shell::state::Studio,
        episode_preview: bool,
    ) -> (String, Option<String>) {
        match self {
            Target::Robot { robot } => (robot.clone(), None),
            Target::Camera { robot, role } => (format!("{role} camera"), Some(robot.clone())),
            Target::Joint { robot, joint } => {
                let label = Joint::named(joint)
                    .map_or_else(|| joint.clone(), |joint| joint.label().to_owned());
                let telemetry = studio.robot_telemetry(robot);
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

/// Build the bench from what the studio currently knows.
///
/// Selection, hover, and hardware verdicts remain application policy; the
/// viewport only receives their presentation states for semantic targets.
struct SceneBuildRequest<'a> {
    studio: &'a crate::shell::state::Studio,
    grid: bool,
    source: &'a SceneSource,
    hovered: Option<&'a Target>,
    visible_robot: Option<&'a str>,
    conditions: &'a BTreeMap<(String, String), Condition>,
}

impl SceneBuildRequest<'_> {
    fn build(&self) -> Result<BenchFrame, FrameError> {
        let selected = self.studio.selection().read().to_owned();
        // Hovering a joint row in the inspector lights the motor on the bench, the
        // same way hovering the motor lights the row: one object, seen twice.
        let pointed = self.studio.pointed_selection().read().to_owned();
        let mut frame = BenchFrame::new(self.grid);
        self.add_arms(&mut frame)?;

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

    fn add_arms(&self, frame: &mut BenchFrame) -> Result<(), FrameError> {
        let arms = self
            .studio
            .snapshot()
            .read()
            .robots()
            .iter()
            .filter(|arm| self.visible_robot.is_none_or(|visible| arm.id() == visible))
            .cloned()
            .collect::<Vec<_>>();
        for arm in &arms {
            let recorded_angles = self
                .source
                .sample()
                .and_then(|sample| sample.arms().iter().find(|sample| sample.arm() == arm.id()))
                .and_then(RecordedRobotPose::recorded_angles);
            let telemetry = self
                .source
                .is_live()
                .then(|| {
                    self.studio
                        .robot_telemetry(arm.id())
                        .filter(|_| arm.calibrated())
                })
                .flatten();
            let angles = recorded_angles.or_else(|| {
                telemetry.as_ref().map(|robot| {
                    use crate::activities::live::control::MeasuredJointTelemetry;
                    robot.joints().measured_angles()
                })
            });
            frame.add_arm(
                arm.id().to_owned(),
                spiral_robotics_viewport::ArmPresentation::new(
                    ACCENTS[self.studio.position_of(arm.id()) % ACCENTS.len()].0,
                    angles,
                ),
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
impl crate::shell::state::Studio {
    fn motor_conditions(self) -> BTreeMap<(String, String), Condition> {
        let telemetry_signal = self.telemetry();
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
    bench: &'a Rc<RefCell<Option<Bench>>>,
    at: (f64, f64),
}

impl HoverRequest<'_> {
    fn pick(&self) -> Option<Hover> {
        let slot = self.bench.try_borrow().ok()?;
        let bench = slot.as_ref()?;
        let target = bench.pick(self.at)?;
        let rect = ViewportDom::canvas()?.get_bounding_client_rect();
        Some(Hover {
            entity: target,
            at: (self.at.0 - rect.left(), self.at.1 - rect.top()),
        })
    }
}

/// Render the current-calibration degrees projected by the backend.
trait RecordedRobotPose {
    fn recorded_angles(&self) -> Option<ArmPose>;
}

impl RecordedRobotPose for TimelineArmSample {
    fn recorded_angles(&self) -> Option<ArmPose> {
        let degrees = self
            .degrees()
            .iter()
            .filter_map(|(joint, degrees)| Some((joint.clone(), (*degrees)?)))
            .collect::<BTreeMap<_, _>>();
        let gripper = self
            .normalized()
            .get("gripper")
            .copied()
            .flatten()
            .filter(|value| value.is_finite());
        (!degrees.is_empty() || gripper.is_some())
            .then(|| ArmPose::from_degrees(&degrees, gripper.unwrap_or_default()))
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

impl CameraBindingPresentation for spiral_robotics_api::robots::CameraBinding {
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
fn Legend(arms: Vec<RobotSummary>, on_select: EventHandler<Option<Selection>>) -> Element {
    let studio = studio();
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
                    Action {
                        purpose: ActionPurpose::Select,
                        variant: ActionVariant::Row,
                        class: "viewport-legend-row",
                        aria_pressed: studio.selection().read().as_ref().is_some_and(|selection| selection.matches_robot(arm.id())),
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
            Action {
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
                Action {
                    purpose: ActionPurpose::Apply,
                    title: "Front view", aria_label: "Front view",
                    onclick: move |_| on_command.call(Command::Front),
                    "Front"
                }
                Action {
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

impl crate::shell::state::Studio {
    fn framed_robot(&self, visible_robot: Option<&str>) -> Option<String> {
        visible_robot.map(str::to_owned).or_else(|| {
            self.selection()
                .read()
                .as_ref()
                .map(|selection| selection.robot_id().to_owned())
                .or_else(|| self.focused().map(|robot| robot.id().to_owned()))
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

struct ViewportDom;

impl ViewportDom {
    /// Keep receiving moves after the pointer leaves the canvas, so a drag that
    /// runs off the edge of a panel keeps turning the robot.
    #[cfg(feature = "web")]
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

    #[cfg(not(feature = "web"))]
    fn capture_pointer(_event: &Event<PointerData>) {}

    #[cfg(feature = "web")]
    fn canvas() -> Option<HtmlCanvasElement> {
        web_sys::window()?
            .document()?
            .get_element_by_id(CANVAS_ID)?
            .dyn_into()
            .ok()
    }

    #[cfg(not(feature = "web"))]
    fn canvas() -> Option<HtmlCanvasElement> {
        None
    }
}

/// A window event subscription that ends when the viewport does.
#[cfg(feature = "web")]
struct WindowListener {
    event: &'static str,
    callback: Closure<dyn FnMut()>,
}

#[cfg(feature = "web")]
impl WindowListener {
    fn new(event: &'static str, handler: impl FnMut() + 'static) -> Option<Self> {
        let callback = Closure::wrap(Box::new(handler) as Box<dyn FnMut()>);
        web_sys::window()?
            .add_event_listener_with_callback(event, callback.as_ref().unchecked_ref())
            .ok()?;
        Some(Self { event, callback })
    }
}

#[cfg(feature = "web")]
impl Drop for WindowListener {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            let _ = window.remove_event_listener_with_callback(
                self.event,
                self.callback.as_ref().unchecked_ref(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_pose_uses_backend_projected_degrees() {
        let sample = TimelineArmSample::new(
            "follower",
            BTreeMap::from([("shoulder_pan".to_owned(), Some(1.0))]),
            BTreeMap::from([("shoulder_pan".to_owned(), Some(90.0))]),
        );

        let pose = sample.recorded_angles().expect("episode_preview pose");
        assert_eq!(pose.degrees(Joint::ShoulderPan), Some(90.0));
    }

    #[test]
    fn recorded_pose_does_not_guess_without_projected_degrees() {
        let sample = TimelineArmSample::new(
            "follower",
            BTreeMap::from([("shoulder_pan".to_owned(), None)]),
            BTreeMap::from([("shoulder_pan".to_owned(), None)]),
        );
        assert!(sample.recorded_angles().is_none());
    }
}
