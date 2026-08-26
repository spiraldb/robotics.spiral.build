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
use spiral_robotics_viewport::glam::Vec3;
use spiral_robotics_viewport::{
    ArmPlacement, BENCH_SPACING, BenchPosition, Camera, CameraAttachment, CameraMount, Condition,
    Drag, DragUpdate, Gesture, Interaction, PickRequest, RobotEntity, RobotScene, SceneAppearance,
    SceneFrame, Viewport as Bench,
};
use spiral_robotics_viewport::{JointAngles, JointDefinition};
#[cfg(feature = "web")]
use wasm_bindgen::JsCast;
#[cfg(feature = "web")]
use wasm_bindgen::prelude::Closure;
use web_sys::HtmlCanvasElement;

use crate::client::http::{TimelineArmSample, TimelineSample};
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

trait BenchCanvas {
    /// The canvas height in CSS pixels, which is what turns a drag into metres.
    fn canvas_height(&self) -> f32;
}

impl BenchCanvas for Rc<RefCell<Option<Bench>>> {
    fn canvas_height(&self) -> f32 {
        self.try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref().map(Bench::height))
            .unwrap_or(1.0)
    }
}

/// What the pointer is over, and where to put a note about it.
#[derive(Clone, PartialEq)]
struct Hover {
    entity: RobotEntity,
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
    let urdf = crate::shell::state::description();
    let bench = use_hook(|| Rc::new(RefCell::new(Option::<Bench>::None)));
    let mut camera = use_signal(Camera::default);
    let mut drag = use_signal(|| Option::<Drag>::None);
    let mut trouble = use_signal(|| Option::<String>::None);
    let mut hover = use_signal(|| Option::<Hover>::None);
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
    let draw_urdf = urdf.clone();
    use_effect(use_reactive(
        (&resize_revision, &source, &visible_robot),
        move |(_resize_revision, draw_source, visible_robot)| {
            // A workbench divider changes this canvas without changing the
            // browser window. Its host increments the revision while dragging
            // so WebGL redraws against the canvas's new CSS bounds instead of
            // stretching the previous frame until the next telemetry or camera
            // interaction.
            let _ = resized();
            let camera = camera();
            let grid = show_grid();
            let hovered = hover();
            let Ok(urdf) = draw_urdf.as_ref() else {
                return;
            };
            let scene = SceneBuildRequest {
                urdf,
                studio: &studio,
                grid,
                source: &draw_source,
                hovered: hovered.as_ref().map(|it| &it.entity),
                visible_robot: visible_robot.as_deref(),
            }
            .build();

            let Some(canvas) = ViewportDom::canvas() else {
                return;
            };
            let mut slot = draw_bench.borrow_mut();
            if slot.is_none() {
                match Bench::new(canvas) {
                    Ok(ready) => *slot = Some(ready),
                    Err(error) => {
                        trouble.set(Some(error));
                        return;
                    }
                }
            }
            let Some(bench) = slot.as_mut() else {
                return;
            };
            let ratio = web_sys::window().map_or(1.0, |window| window.device_pixel_ratio());
            bench.draw(SceneFrame::new(&camera, scene.scene(), ratio));
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
    let failure = match urdf.as_ref() {
        Err(error) => Some(format!("The robot description could not be read: {error}")),
        Ok(_) => trouble(),
    };
    // Where each unwell motor sits on the canvas right now. Computed from the
    // scene the crate just drew, so a mark cannot drift away from the part it
    // is about as the camera moves.
    let marks = if source.is_live() {
        ConditionMarksRequest {
            urdf: &urdf,
            studio: &studio,
            bench: &bench,
            camera: camera(),
            grid: show_grid(),
            visible_robot: visible_robot.as_deref(),
        }
        .marks()
    } else {
        Vec::new()
    };
    let historical_active = !source.is_live();
    let recorded_status = source.recorded_status();

    rsx! {
        div { class: "relative h-full w-full overflow-hidden bg-viewport",
            canvas {
                id: CANVAS_ID,
                class: "h-full w-full select-none",
                class: if drag().is_some() {
                    "cursor-grabbing"
                } else if hover().is_some() {
                    "cursor-pointer"
                } else {
                    "cursor-grab"
                },
                role: "application",
                tabindex: "0",
                "aria-label": "Interactive 3D robot viewer. Click a motor or camera to select it and see its readings in the inspector. Drag to orbit, shift-drag to pan, scroll to zoom, and press F to frame the selection.",
                onpointerdown: move |event| {
                    ViewportDom::capture_pointer(&event);
                    // The middle and right buttons pan, matching every other 3D
                    // viewer; shift with the left button does the same for a
                    // one-button mouse.
                    let panning = event.modifiers().shift()
                        || matches!(
                            event.trigger_button(),
                            Some(MouseButton::Secondary | MouseButton::Auxiliary)
                        );
                    drag.set(Some(Drag::new(event.client_point(), panning)));
                },
                onpointermove: {
                    let bench = bench.clone();
                    let urdf = urdf.clone();
                    let source = source.clone();
                    let visible_robot = visible_robot.clone();
                    move |event| {
                        let position = event.client_point();
                        if let Some(mut active) = drag() {
                            active.advance(
                                &mut camera.write(),
                                DragUpdate::new(position, bench.canvas_height()),
                            );
                            drag.set(Some(active));
                            return;
                        }
                        // Only while nothing is being dragged: an orbit is not
                        // a tour of everything the pointer sweeps past.
                        hover.set(
                            HoverRequest {
                                urdf: &urdf,
                                studio: &studio,
                                bench: &bench,
                                camera: camera(),
                                grid: show_grid(),
                                pick: PickContext {
                                source: &source,
                                visible_robot: visible_robot.as_deref(),
                                at: position,
                            },
                            }
                            .pick(),
                        );
                    }
                },
                onpointerup: {
                    let bench = bench.clone();
                    let urdf = urdf.clone();
                    let source = source.clone();
                    let visible_robot = visible_robot.clone();
                    move |event| {
                        if drag().is_some_and(|active| !active.moved()) {
                            let position = event.client_point();
                            let picked = HoverRequest {
                                urdf: &urdf,
                                studio: &studio,
                                bench: &bench,
                                camera: camera(),
                                grid: show_grid(),
                                pick: PickContext {
                                    source: &source,
                                    visible_robot: visible_robot.as_deref(),
                                    at: position,
                                },
                            }
                            .pick();
                            // Clicking bare bench clears the selection, the way
                            // clicking empty space does everywhere else.
                            on_select.call(picked.map(|hit| Selection::from(&hit.entity)));
                        }
                        drag.set(None);
                    }
                },
                onpointercancel: move |_| {
                    drag.set(None);
                },
                onpointerleave: move |_| {
                    drag.set(None);
                    hover.set(None);
                },
                oncontextmenu: move |event| event.prevent_default(),
                onwheel: move |event| {
                    let notches = match event.delta() {
                        WheelDelta::Pixels(delta) => delta.y / 120.0,
                        WheelDelta::Lines(delta) => delta.y / 3.0,
                        WheelDelta::Pages(delta) => delta.y,
                    };
                    Gesture::Wheel(notches).apply(&mut camera.write());
                },
                onkeydown: move |event| {
                    let gesture = match event.key() {
                        Key::ArrowLeft => Some(Gesture::OrbitLeft),
                        Key::ArrowRight => Some(Gesture::OrbitRight),
                        Key::ArrowUp => Some(Gesture::OrbitUp),
                        Key::ArrowDown => Some(Gesture::OrbitDown),
                        Key::Character(value) if value == "+" || value == "=" => {
                            Some(Gesture::ZoomIn)
                        }
                        Key::Character(value) if value == "-" || value == "_" => {
                            Some(Gesture::ZoomOut)
                        }
                        Key::Character(value) if value == "0" => {
                            camera.set(Camera::default());
                            None
                        }
                        Key::Character(value) if value.eq_ignore_ascii_case("f") => {
                            studio.frame_selection(&mut camera, visible_robot.as_deref());
                            None
                        }
                        _ => None,
                    };
                    if let Some(gesture) = gesture {
                        gesture.apply(&mut camera.write());
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
                    HoverNote { hover: hovered, historical: historical_active }
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
                        onclick: move |_| camera.set(Camera::default()),
                        title: "Restore the default camera (0)",
                        "Home"
                    }
                }
                ViewCube { camera }
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
fn HoverNote(hover: Hover, historical: bool) -> Element {
    let studio = studio();
    let (title, detail) = hover.entity.describe(&studio, historical);
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
trait RobotEntityPresentation {
    fn describe(
        &self,
        studio: &crate::shell::state::Studio,
        historical: bool,
    ) -> (String, Option<String>);
    fn matches_selection(&self, selection: Option<&Selection>) -> bool;
}

impl RobotEntityPresentation for RobotEntity {
    fn describe(
        &self,
        studio: &crate::shell::state::Studio,
        historical: bool,
    ) -> (String, Option<String>) {
        match self {
            RobotEntity::Structure { robot } => (robot.clone(), None),
            RobotEntity::Camera { robot, role } => (format!("{role} camera"), Some(robot.clone())),
            RobotEntity::Joint { robot, joint } => {
                let label = JointDefinition::so101(joint)
                    .map_or_else(|| joint.clone(), |joint| joint.label().to_owned());
                let telemetry = studio.robot_telemetry(robot);
                let detail = (!historical)
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

    fn matches_selection(&self, selection: Option<&Selection>) -> bool {
        match selection {
            Some(Selection::Robot(robot)) => self.robot() == robot,
            Some(Selection::Joint { robot, joint }) => matches!(
                self,
                Self::Joint { robot: on, joint: named } if on == robot && named == joint
            ),
            Some(Selection::Camera { robot, role }) => matches!(
                self,
                Self::Camera { robot: on, role: named } if on == robot && named == role
            ),
            None => false,
        }
    }
}

/// Turn a scene entity into the studio's own idea of what is selected.
impl From<&RobotEntity> for Selection {
    fn from(entity: &RobotEntity) -> Self {
        match entity {
            RobotEntity::Joint { robot, joint } => Selection::Joint {
                robot: robot.clone(),
                joint: joint.clone(),
            },
            RobotEntity::Structure { robot } => Selection::Robot(robot.clone()),
            RobotEntity::Camera { robot, role } => Selection::Camera {
                robot: robot.clone(),
                role: role.clone(),
            },
        }
    }
}

/// Whether `entity` is one of the things `selection` refers to.
///
/// Selecting a whole arm selects all of it. The legend row names the robot, not
/// the plinth it stands on, so lighting only the base would answer a different
/// question than the one the operator asked.
/// Build the bench from what the studio currently knows.
///
/// The two closures are where the studio's state meets the scene: one says what
/// the operator is doing with a thing, the other what its hardware is saying
/// about itself. Neither is stored in the scene, because both change far more
/// often than the geometry does.
struct SceneBuildRequest<'a> {
    urdf: &'a spiral_robotics_viewport::Urdf,
    studio: &'a crate::shell::state::Studio,
    grid: bool,
    source: &'a SceneSource,
    hovered: Option<&'a RobotEntity>,
    visible_robot: Option<&'a str>,
}

impl SceneBuildRequest<'_> {
    fn build(&self) -> RobotScene {
        let selected = self.studio.selection().read().to_owned();
        // Hovering a joint row in the inspector lights the motor on the bench, the
        // same way hovering the motor lights the row: one object, seen twice.
        let pointed = self.studio.pointed_selection().read().to_owned();
        let hovered = self.hovered.cloned();
        let conditions = if self.source.is_live() {
            self.studio.motor_conditions()
        } else {
            Default::default()
        };

        RobotScene::compose(
            self.urdf,
            &self.studio.placements(self.source, self.visible_robot),
            SceneAppearance::new(
                self.grid,
                |entity: &RobotEntity| {
                    if entity.matches_selection(selected.as_ref()) {
                        Interaction::Selected
                    } else if hovered.as_ref() == Some(entity)
                        || entity.matches_selection(pointed.as_ref())
                    {
                        Interaction::Hovered
                    } else {
                        Interaction::Resting
                    }
                },
                |entity: &RobotEntity| match entity {
                    RobotEntity::Joint { robot, joint } => conditions
                        .get(&(robot.clone(), joint.clone()))
                        .copied()
                        .unwrap_or_default(),
                    _ => Condition::Nominal,
                },
            ),
        )
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

struct ConditionMarksRequest<'a> {
    urdf: &'a crate::shell::state::Description,
    studio: &'a crate::shell::state::Studio,
    bench: &'a Rc<RefCell<Option<Bench>>>,
    camera: Camera,
    grid: bool,
    visible_robot: Option<&'a str>,
}

impl ConditionMarksRequest<'_> {
    fn marks(&self) -> Vec<Mark> {
        let Ok(urdf) = self.urdf.as_ref().as_ref() else {
            return Vec::new();
        };
        let Ok(slot) = self.bench.try_borrow() else {
            return Vec::new();
        };
        let Some(bench) = slot.as_ref() else {
            return Vec::new();
        };
        let scene = SceneBuildRequest {
            urdf,
            studio: self.studio,
            grid: self.grid,
            source: &SceneSource::Live,
            hovered: None,
            visible_robot: self.visible_robot,
        }
        .build();
        let rect = ViewportDom::canvas().map(|canvas| canvas.get_bounding_client_rect());
        let (left, top) = rect.map_or((0.0, 0.0), |rect| (rect.left(), rect.top()));

        scene
            .scene()
            .entities()
            .filter(|entity| entity.condition() != Condition::Nominal)
            .map(|entity| entity.key())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter_map(|key| {
                let entity = scene.entity(key)?;
                let RobotEntity::Joint { robot, joint } = entity else {
                    return None;
                };
                let at = bench.anchor(&self.camera, scene.scene().anchor_point(key)?)?;
                let fault = scene
                    .scene()
                    .entity(key)
                    .is_some_and(|entity| entity.condition() == Condition::Fault);
                Some((format!("{robot}/{joint}"), at.0 - left, at.1 - top, fault))
            })
            .collect()
    }
}

/// What the pointer is over, and where the note about it belongs.
struct PickContext<'a> {
    source: &'a SceneSource,
    visible_robot: Option<&'a str>,
    at: (f64, f64),
}

struct HoverRequest<'a> {
    urdf: &'a crate::shell::state::Description,
    studio: &'a crate::shell::state::Studio,
    bench: &'a Rc<RefCell<Option<Bench>>>,
    camera: Camera,
    grid: bool,
    pick: PickContext<'a>,
}

impl HoverRequest<'_> {
    fn pick(&self) -> Option<Hover> {
        let urdf = self.urdf.as_ref().as_ref().ok()?;
        let slot = self.bench.try_borrow().ok()?;
        let bench = slot.as_ref()?;
        let scene = SceneBuildRequest {
            urdf,
            studio: self.studio,
            grid: self.grid,
            source: self.pick.source,
            hovered: None,
            visible_robot: self.pick.visible_robot,
        }
        .build();
        let key = bench.pick(PickRequest::new(&self.camera, scene.scene(), self.pick.at))?;
        let rect = ViewportDom::canvas()?.get_bounding_client_rect();
        Some(Hover {
            entity: scene.entity(key)?.clone(),
            at: (self.pick.at.0 - rect.left(), self.pick.at.1 - rect.top()),
        })
    }
}

/// Every arm on the bench, posed from what it is currently reporting.
impl crate::shell::state::Studio {
    fn placements(self, source: &SceneSource, visible_robot: Option<&str>) -> Vec<ArmPlacement> {
        let arms = self
            .snapshot()
            .read()
            .robots()
            .iter()
            .filter(|arm| visible_robot.is_none_or(|visible| arm.id() == visible))
            .cloned()
            .collect::<Vec<_>>();
        let count = arms.len();
        arms.iter()
            .enumerate()
            .map(|(index, arm)| {
                // An arm that is offline or uncalibrated has no angles worth
                // drawing, so it stands at its rest pose and is faded rather than
                // left out — an empty bench says less than a greyed-out one.
                let historical_angles = source
                    .sample()
                    .and_then(|sample| sample.arms().iter().find(|sample| sample.arm() == arm.id()))
                    .and_then(HistoricalRobotPose::historical_angles);
                let telemetry = source
                    .is_live()
                    .then(|| self.robot_telemetry(arm.id()).filter(|_| arm.calibrated()))
                    .flatten();
                let angles = historical_angles.or_else(|| {
                    telemetry.as_ref().map(|robot| {
                        use crate::activities::live::control::MeasuredJointTelemetry;
                        robot.joints().measured_angles()
                    })
                });
                let placement = ArmPlacement::new(
                    arm.id().to_owned(),
                    ACCENTS[self.position_of(arm.id()) % ACCENTS.len()].0,
                    BenchPosition::new(index, count),
                )
                .with_cameras(
                    arm.cameras()
                        .iter()
                        .filter_map(CameraBindingPresentation::camera_mount)
                        .collect(),
                );
                match angles {
                    Some(angles) => placement.with_live_pose(angles),
                    None => placement,
                }
            })
            .collect()
    }
}

/// Render the current-calibration degrees projected by the backend.
trait HistoricalRobotPose {
    fn historical_angles(&self) -> Option<JointAngles>;
}

impl HistoricalRobotPose for TimelineArmSample {
    fn historical_angles(&self) -> Option<JointAngles> {
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
        (!degrees.is_empty() || gripper.is_some()).then(|| {
            JointAngles::from(&degrees).with_gripper_normalized(gripper.unwrap_or_default())
        })
    }
}

trait HistoricalTimePresentation {
    fn history_time(self) -> String;
}

impl HistoricalTimePresentation for f64 {
    fn history_time(self) -> String {
        let seconds = self.max(0.0);
        let minutes = (seconds / 60.0).floor() as u64;
        format!("{minutes}:{:04.1}", seconds - minutes as f64 * 60.0)
    }
}

/// Where a bound camera sits, for the ones whose file says.
trait CameraBindingPresentation {
    fn camera_mount(&self) -> Option<CameraMount>;
}

impl CameraBindingPresentation for spiral_robotics_api::robots::CameraBinding {
    fn camera_mount(&self) -> Option<CameraMount> {
        let camera_mount = self.mount();
        let mount = camera_mount.as_ref()?;
        Some(CameraMount::new(
            self.name().to_owned(),
            CameraAttachment::new(
                mount.frame().map(str::to_owned),
                mount.xyz().map(|value| value as f32),
                mount.rpy().map(|value| value as f32),
            ),
            mount.fov_degrees() as f32,
        ))
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
fn ViewCube(mut camera: Signal<Camera>) -> Element {
    rsx! {
        div { class: "view-cube", role: "group", "aria-label": "Camera orientation",
            Action {
                purpose: ActionPurpose::Apply,
                title: "Top view", aria_label: "Top view",
                onclick: move |_| camera.set(camera().oriented(-0.65, 1.42)),
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
                    onclick: move |_| camera.set(camera().oriented(-std::f32::consts::FRAC_PI_2, 0.08)),
                    "Front"
                }
                Action {
                    purpose: ActionPurpose::Apply,
                    title: "Right side view", aria_label: "Right side view",
                    onclick: move |_| camera.set(camera().oriented(0.0, 0.08)),
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
    fn frame_selection(&self, camera: &mut Signal<Camera>, visible_robot: Option<&str>) {
        let robot = visible_robot.map(str::to_owned).or_else(|| {
            self.selection()
                .read()
                .as_ref()
                .map(|selection| selection.robot_id().to_owned())
                .or_else(|| self.focused().map(|robot| robot.id().to_owned()))
        });
        let Some(robot) = robot else {
            camera.set(Camera::default());
            return;
        };
        let count = if visible_robot.is_some() {
            1
        } else {
            self.snapshot().read().robots().len()
        };
        let index = if visible_robot.is_some() {
            0
        } else {
            self.position_of(&robot)
        };
        let base = Vec3::new(
            0.0,
            (index as f32 - count.saturating_sub(1) as f32 / 2.0) * BENCH_SPACING,
            0.0,
        );
        camera.set(camera().focused_on(base + Vec3::new(0.03, 0.0, 0.17), 0.68));
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
    fn historical_pose_uses_backend_projected_degrees() {
        let sample = TimelineArmSample::new(
            "follower",
            BTreeMap::from([("shoulder_pan".to_owned(), Some(1.0))]),
            BTreeMap::from([("shoulder_pan".to_owned(), Some(90.0))]),
        );

        let pose = sample.historical_angles().expect("historical pose");
        assert_eq!(pose.pan_degrees(), 90.0);
    }

    #[test]
    fn historical_pose_does_not_guess_without_projected_degrees() {
        let sample = TimelineArmSample::new(
            "follower",
            BTreeMap::from([("shoulder_pan".to_owned(), None)]),
            BTreeMap::from([("shoulder_pan".to_owned(), None)]),
        );
        assert!(sample.historical_angles().is_none());
    }
}
