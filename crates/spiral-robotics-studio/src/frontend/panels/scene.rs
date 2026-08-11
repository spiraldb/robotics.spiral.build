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
use spiral_robotics_core::api::{MotorCondition, RobotSummary};
use spiral_robotics_core::arm::{JointAngles, counts_to_degrees};
use spiral_robotics_core::calibration::{Direction, denormalize_centered_raw};
use spiral_robotics_core::glam::Vec3;
use spiral_robotics_viewport::robot::BENCH_SPACING;
use spiral_robotics_viewport::{
    ArmPlacement, Camera, CameraMount, Condition, Drag, Gesture, Interaction, Pinch, RobotEntity,
    RobotScene, Viewport as Bench, robot,
};
#[cfg(feature = "web")]
use wasm_bindgen::JsCast;
#[cfg(feature = "web")]
use wasm_bindgen::prelude::Closure;
use web_sys::HtmlCanvasElement;

use crate::frontend::api::http::{TimelineArmSample, TimelineSample};
use crate::frontend::app::activity::RecordedSource;
use crate::frontend::app::state::{Selection, studio};

pub use spiral_robotics_viewport::tool_position;

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

/// The accent for the arm at `index` in the workspace.
pub fn accent_css(index: usize) -> &'static str {
    ACCENTS[index % ACCENTS.len()].1
}

/// The canvas height in CSS pixels, which is what turns a drag into metres.
fn height(bench: &Rc<RefCell<Option<Bench>>>) -> f32 {
    bench
        .try_borrow()
        .ok()
        .and_then(|slot| slot.as_ref().map(Bench::height))
        .unwrap_or(1.0)
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
            Self::Recorded(source) => Some((source.state.is_loading(), source.state.reason())),
        }
    }
}

#[component]
pub fn Viewport(
    source: SceneSource,
    #[props(default)] resize_revision: u64,
    on_select: EventHandler<Option<Selection>>,
) -> Element {
    let studio = studio();
    let urdf = crate::frontend::app::state::description();
    let bench = use_hook(|| Rc::new(RefCell::new(Option::<Bench>::None)));
    let mut camera = use_signal(Camera::default);
    let mut drag = use_signal(|| Option::<Drag>::None);
    let mut touches = use_signal(BTreeMap::<i32, (f64, f64)>::new);
    let mut pinch = use_signal(|| None::<Pinch>);
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
        (&resize_revision, &source),
        move |(_resize_revision, draw_source)| {
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
            let scene = compose(
                urdf,
                &studio,
                grid,
                &draw_source,
                hovered.as_ref().map(|it| &it.entity),
            );

            let Some(canvas) = canvas() else { return };
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
            bench.draw(&camera, &scene.scene, ratio);
        },
    ));

    let arms = studio.snapshot.read().robots.clone();
    let failure = match urdf.as_ref() {
        Err(error) => Some(format!("The robot description could not be read: {error}")),
        Ok(_) => trouble(),
    };
    // Where each unwell motor sits on the canvas right now. Computed from the
    // scene the crate just drew, so a mark cannot drift away from the part it
    // is about as the camera moves.
    let marks = if source.is_live() {
        condition_marks(&urdf, &studio, &bench, camera(), show_grid())
    } else {
        Vec::new()
    };
    let historical_active = !source.is_live();
    let recorded_status = source.recorded_status();

    rsx! {
        div { class: "relative h-full w-full overflow-hidden bg-viewport",
            canvas {
                id: CANVAS_ID,
                class: "h-full w-full touch-none select-none",
                class: if drag().is_some() {
                    "cursor-grabbing"
                } else if hover().is_some() {
                    "cursor-pointer"
                } else {
                    "cursor-grab"
                },
                role: "application",
                tabindex: "0",
                "aria-label": "Interactive 3D robot viewer. Click a motor or camera to select it and see its readings in the inspector. Drag to orbit, shift-drag or two-finger drag to pan, pinch or scroll to zoom, and press F to frame the selection.",
                onpointerdown: move |event| {
                    capture(&event);
                    if let Some((pointer, position)) = touch(&event) {
                        touches.write().insert(pointer, position);
                        if let Some(current) = Pinch::from_touches(&touches.read()) {
                            pinch.set(Some(current));
                            drag.set(None);
                            return;
                        }
                    }
                    // The middle and right buttons pan, matching every other 3D
                    // viewer; shift with the left button does the same for a
                    // one-button mouse.
                    let panning = event.modifiers().shift()
                        || matches!(
                            event.trigger_button(),
                            Some(MouseButton::Secondary | MouseButton::Auxiliary)
                        );
                    drag.set(Some(Drag::new(point(&event), panning)));
                },
                onpointermove: {
                    let bench = bench.clone();
                    let urdf = urdf.clone();
                    let source = source.clone();
                    move |event| {
                        let position = point(&event);
                        if let Some((pointer, _)) = touch(&event)
                            && touches.read().contains_key(&pointer)
                        {
                            touches.write().insert(pointer, position);
                            if let Some(current) = Pinch::from_touches(&touches.read()) {
                                if let Some(previous) = pinch() {
                                    current.advance_from(
                                        previous,
                                        &mut camera.write(),
                                        height(&bench),
                                    );
                                }
                                pinch.set(Some(current));
                                return;
                            }
                        }
                        if let Some(mut active) = drag() {
                            active.advance(position, &mut camera.write(), height(&bench));
                            drag.set(Some(active));
                            return;
                        }
                        // Only while nothing is being dragged: an orbit is not
                        // a tour of everything the pointer sweeps past.
                        hover.set(hovered(
                            &urdf,
                            &studio,
                            &bench,
                            camera(),
                            show_grid(),
                            &source,
                            position,
                        ));
                    }
                },
                onpointerup: {
                    let bench = bench.clone();
                    let urdf = urdf.clone();
                    let source = source.clone();
                    move |event| {
                        if let Some((pointer, _)) = touch(&event) {
                            touches.write().remove(&pointer);
                            pinch.set(None);
                            drag.set(None);
                            return;
                        }
                        if drag().is_some_and(|active| !active.moved()) {
                            let position = point(&event);
                            let picked = hovered(
                                &urdf,
                                &studio,
                                &bench,
                                camera(),
                                show_grid(),
                                &source,
                                position,
                            );
                            // Clicking bare bench clears the selection, the way
                            // clicking empty space does everywhere else.
                            on_select.call(picked.map(|hit| selection_of(&hit.entity)));
                        }
                        drag.set(None);
                    }
                },
                onpointercancel: move |event| {
                    if let Some((pointer, _)) = touch(&event) {
                        touches.write().remove(&pointer);
                        pinch.set(None);
                    }
                    drag.set(None);
                },
                onpointerleave: move |event| {
                    // Captured mouse pointers continue reporting outside the
                    // canvas. Touch pointers are kept until up/cancel so a
                    // two-finger gesture cannot lose one finger at an edge.
                    if touch(&event).is_none() {
                        drag.set(None);
                    }
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
                            frame_selection(&mut camera, &studio);
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
                        "History · {format_history_time(sample.seconds)}"
                    }
                } else if let Some((loading, reason)) = recorded_status {
                    div {
                        class: "pointer-events-none absolute left-1/2 top-2 z-20 -translate-x-1/2 rounded-sm border border-line bg-surface px-2 py-1 text-[10px] text-ink-soft",
                        if loading { "Loading recorded scene…" } else { "Recorded scene unavailable" }
                        if let Some(reason) = reason {
                            span { class: "ml-1 text-ink-faint", "· {reason}" }
                        }
                    }
                }
                div { class: "viewport-help",
                    "click select · drag orbit · shift-drag pan · scroll zoom · F frame"
                }
                // One group rather than two absolutely positioned buttons, the
                // second of which was pushed off the first by a hard-coded 64px
                // that only held while "Home" was exactly that wide.
                div { class: "viewport-controls",
                    button {
                        r#type: "button",
                        class: "viewport-button",
                        "aria-pressed": show_grid(),
                        onclick: move |_| show_grid.toggle(),
                        title: "Show or hide the scale grid",
                        "Grid"
                    }
                    button {
                        r#type: "button",
                        class: "viewport-button",
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
    let (title, detail) = describe(&studio, &hover.entity, historical);
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
fn describe(
    studio: &crate::frontend::app::state::Studio,
    entity: &RobotEntity,
    historical: bool,
) -> (String, Option<String>) {
    match entity {
        RobotEntity::Structure { robot } => (robot.clone(), None),
        RobotEntity::Camera { robot, role } => (format!("{role} camera"), Some(robot.clone())),
        RobotEntity::Joint { robot, joint } => {
            let label = spiral_robotics_core::arm::so101_joint(joint)
                .map_or_else(|| joint.clone(), |joint| joint.label.to_owned());
            let telemetry = studio.robot_telemetry(robot).and_then(|telemetry| {
                telemetry
                    .joints
                    .into_iter()
                    .find(|candidate| candidate.name == *joint)
            });
            let detail = (!historical)
                .then_some(telemetry)
                .flatten()
                .and_then(|telemetry| {
                    telemetry
                        .temperature_celsius
                        .map(|celsius| format!("{celsius:.0} °C"))
                });
            (label, detail)
        }
    }
}

/// Turn a scene entity into the studio's own idea of what is selected.
fn selection_of(entity: &RobotEntity) -> Selection {
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

/// Whether `entity` is one of the things `selection` refers to.
///
/// Selecting a whole arm selects all of it. The legend row names the robot, not
/// the plinth it stands on, so lighting only the base would answer a different
/// question than the one the operator asked.
fn matches_selection(selection: Option<&Selection>, entity: &RobotEntity) -> bool {
    match selection {
        Some(Selection::Robot(robot)) => entity.robot() == robot,
        Some(Selection::Joint { robot, joint }) => matches!(
            entity,
            RobotEntity::Joint { robot: on, joint: named } if on == robot && named == joint
        ),
        Some(Selection::Camera { robot, role }) => matches!(
            entity,
            RobotEntity::Camera { robot: on, role: named } if on == robot && named == role
        ),
        None => false,
    }
}

/// Build the bench from what the studio currently knows.
///
/// The two closures are where the studio's state meets the scene: one says what
/// the operator is doing with a thing, the other what its hardware is saying
/// about itself. Neither is stored in the scene, because both change far more
/// often than the geometry does.
fn compose(
    urdf: &spiral_robotics_core::urdf::Urdf,
    studio: &crate::frontend::app::state::Studio,
    grid: bool,
    source: &SceneSource,
    hovered: Option<&RobotEntity>,
) -> RobotScene {
    let selected = studio.selected.read().clone();
    // Hovering a joint row in the inspector lights the motor on the bench, the
    // same way hovering the motor lights the row: one object, seen twice.
    let pointed = studio.pointed.read().clone();
    let conditions = if source.is_live() {
        motor_conditions(studio)
    } else {
        Default::default()
    };

    robot::build(
        urdf,
        &placements(studio, source),
        grid,
        |entity| {
            if matches_selection(selected.as_ref(), entity) {
                Interaction::Selected
            } else if hovered == Some(entity) || matches_selection(pointed.as_ref(), entity) {
                Interaction::Hovered
            } else {
                Interaction::Resting
            }
        },
        |entity| match entity {
            RobotEntity::Joint { robot, joint } => conditions
                .get(&(robot.clone(), joint.clone()))
                .copied()
                .unwrap_or_default(),
            _ => Condition::Nominal,
        },
    )
}

/// Every motor's condition, judged once per frame against its own limits.
fn motor_conditions(
    studio: &crate::frontend::app::state::Studio,
) -> BTreeMap<(String, String), Condition> {
    let snapshot = studio.snapshot.read();
    let telemetry = studio.telemetry.read();
    let mut conditions = BTreeMap::new();
    for robot in &telemetry.robots {
        let Some(summary) = snapshot.robots.iter().find(|arm| arm.id == robot.id) else {
            continue;
        };
        for joint in &robot.joints {
            let limits = summary
                .motors
                .iter()
                .find(|motor| motor.name == joint.name)
                .map(|motor| motor.limits)
                .unwrap_or_default();
            let condition = match joint.condition(&limits) {
                MotorCondition::Nominal => Condition::Nominal,
                MotorCondition::Caution => Condition::Caution,
                MotorCondition::Fault => Condition::Fault,
            };
            if condition != Condition::Nominal {
                conditions.insert((robot.id.clone(), joint.name.clone()), condition);
            }
        }
    }
    conditions
}

/// A key, its place on the canvas, and whether it is a fault rather than a
/// caution — one entry per motor currently reporting trouble.
type Mark = (String, f64, f64, bool);

fn condition_marks(
    urdf: &crate::frontend::app::state::Description,
    studio: &crate::frontend::app::state::Studio,
    bench: &Rc<RefCell<Option<Bench>>>,
    camera: Camera,
    grid: bool,
) -> Vec<Mark> {
    let Ok(urdf) = urdf.as_ref().as_ref() else {
        return Vec::new();
    };
    let Ok(slot) = bench.try_borrow() else {
        return Vec::new();
    };
    let Some(bench) = slot.as_ref() else {
        return Vec::new();
    };
    let scene = compose(urdf, studio, grid, &SceneSource::Live, None);
    let rect = canvas().map(|canvas| canvas.get_bounding_client_rect());
    let (left, top) = rect.map_or((0.0, 0.0), |rect| (rect.left(), rect.top()));

    scene
        .scene
        .entities
        .iter()
        .filter(|entity| entity.condition != Condition::Nominal)
        .map(|entity| entity.key)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter_map(|key| {
            let entity = scene.entity(key)?;
            let RobotEntity::Joint { robot, joint } = entity else {
                return None;
            };
            let at = bench.anchor(&camera, scene.scene.anchor_point(key)?)?;
            let fault = scene
                .scene
                .entity(key)
                .is_some_and(|entity| entity.condition == Condition::Fault);
            Some((format!("{robot}/{joint}"), at.0 - left, at.1 - top, fault))
        })
        .collect()
}

/// What the pointer is over, and where the note about it belongs.
fn hovered(
    urdf: &crate::frontend::app::state::Description,
    studio: &crate::frontend::app::state::Studio,
    bench: &Rc<RefCell<Option<Bench>>>,
    camera: Camera,
    grid: bool,
    source: &SceneSource,
    at: (f64, f64),
) -> Option<Hover> {
    let urdf = urdf.as_ref().as_ref().ok()?;
    let slot = bench.try_borrow().ok()?;
    let bench = slot.as_ref()?;
    let scene = compose(urdf, studio, grid, source, None);
    let key = bench.pick(&camera, &scene.scene, at)?;
    let rect = canvas()?.get_bounding_client_rect();
    Some(Hover {
        entity: scene.entity(key)?.clone(),
        at: (at.0 - rect.left(), at.1 - rect.top()),
    })
}

/// Every arm on the bench, posed from what it is currently reporting.
fn placements(
    studio: &crate::frontend::app::state::Studio,
    source: &SceneSource,
) -> Vec<ArmPlacement> {
    let arms = studio.snapshot.read().robots.clone();
    let count = arms.len();
    arms.iter()
        .enumerate()
        .map(|(index, arm)| {
            // An arm that is offline or uncalibrated has no angles worth
            // drawing, so it stands at its rest pose and is faded rather than
            // left out — an empty bench says less than a greyed-out one.
            let historical_angles = source
                .sample()
                .and_then(|sample| sample.arms.iter().find(|sample| sample.arm == arm.id))
                .and_then(|sample| historical_angles(arm, sample));
            let telemetry = source
                .is_live()
                .then(|| studio.robot_telemetry(&arm.id).filter(|_| arm.calibrated))
                .flatten();
            let angles = historical_angles.or_else(|| {
                telemetry.as_ref().map(|robot| {
                    crate::frontend::screens::live::control::measured_angles(&robot.joints)
                })
            });
            ArmPlacement {
                id: arm.id.clone(),
                accent: ACCENTS[index % ACCENTS.len()].0,
                live: angles.is_some(),
                angles: angles.unwrap_or_default(),
                cameras: arm.cameras.iter().filter_map(mount_of).collect(),
                index,
                count,
            }
        })
        .collect()
}

/// Map a stored normalized pose through the arm's current calibration for a
/// faithful scene. This is preview-only: physical seek independently repeats
/// the same calibration validation on the server before it can move anything.
fn historical_angles(arm: &RobotSummary, sample: &TimelineArmSample) -> Option<JointAngles> {
    let degrees = sample
        .normalized
        .iter()
        .filter_map(|(joint, normalized)| {
            let normalized = (*normalized)?;
            let calibration = arm
                .motors
                .iter()
                .find(|motor| motor.name == *joint)?
                .calibration
                .as_ref()?;
            let raw = denormalize_centered_raw(calibration, normalized).ok()?;
            let degrees = counts_to_degrees(raw.saturating_sub(calibration.center));
            let degrees = match calibration.direction {
                Direction::Normal => degrees,
                Direction::Reversed => -degrees,
            };
            Some((joint.clone(), degrees))
        })
        .collect::<BTreeMap<_, _>>();
    let gripper = sample
        .normalized
        .get("gripper")
        .copied()
        .flatten()
        .filter(|value| value.is_finite());
    (!degrees.is_empty() || gripper.is_some()).then(|| {
        JointAngles::from_named_degrees(&degrees)
            .with_gripper_normalized(gripper.unwrap_or_default())
    })
}

fn format_history_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    let minutes = (seconds / 60.0).floor() as u64;
    format!("{minutes}:{:04.1}", seconds - minutes as f64 * 60.0)
}

/// Where a bound camera sits, for the ones whose file says.
fn mount_of(binding: &spiral_robotics_core::api::CameraBinding) -> Option<CameraMount> {
    let mount = binding.mount.as_ref()?;
    Some(CameraMount {
        role: binding.name.clone(),
        frame: mount.frame.clone(),
        xyz: mount.xyz.map(|value| value as f32),
        rpy: mount.rpy.map(|value| value as f32),
        fov_degrees: mount.fov_degrees as f32,
    })
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
                li { key: "{arm.id}",
                    button {
                        r#type: "button",
                        class: if studio.selected.read().as_ref().is_some_and(|selection| selection.matches_robot(&arm.id)) { "viewport-legend-row viewport-legend-selected" } else { "viewport-legend-row" },
                        title: "Select {arm.id}; press F to frame it",
                        onclick: {
                            let id = arm.id.clone();
                            move |_| on_select.call(Some(Selection::Robot(id.clone())))
                        },
                        span { class: "h-1.5 w-1.5 shrink-0 rounded-full", style: "background: {accent_css(index)}" }
                        span { "{arm.id}" }
                        if !arm.connection.connected { span { class: "text-ink-faint", "offline" } }
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
            button {
                r#type: "button", title: "Top view", "aria-label": "Top view",
                onclick: move |_| camera.set(Camera { yaw: -0.65, pitch: 1.42, ..camera() }),
                "Top"
            }
            // Spelled out rather than abbreviated: `F` really is a keyboard
            // shortcut here, so a neighbouring cap reading `R` invited an
            // operator to press the key that starts a recording on live
            // hardware.
            div { class: "view-cube-row",
                button {
                    r#type: "button", title: "Front view", "aria-label": "Front view",
                    onclick: move |_| camera.set(Camera { yaw: -std::f32::consts::FRAC_PI_2, pitch: 0.08, ..camera() }),
                    "Front"
                }
                button {
                    r#type: "button", title: "Right side view", "aria-label": "Right side view",
                    onclick: move |_| camera.set(Camera { yaw: 0.0, pitch: 0.08, ..camera() }),
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

fn frame_selection(camera: &mut Signal<Camera>, studio: &crate::frontend::app::state::Studio) {
    let robot = studio
        .selected
        .read()
        .as_ref()
        .map(|selection| selection.robot_id().to_owned())
        .or_else(|| studio.focused().map(|robot| robot.id));
    let Some(robot) = robot else {
        camera.set(Camera::default());
        return;
    };
    let count = studio.snapshot.read().robots.len();
    let index = studio.position_of(&robot);
    let base = Vec3::new(
        0.0,
        (index as f32 - count.saturating_sub(1) as f32 / 2.0) * BENCH_SPACING,
        0.0,
    );
    camera.set(Camera {
        target: base + Vec3::new(0.03, 0.0, 0.17),
        distance: 0.68,
        ..camera()
    });
}

fn point(event: &Event<PointerData>) -> (f64, f64) {
    let position = event.client_coordinates();
    (position.x, position.y)
}

fn touch(event: &Event<PointerData>) -> Option<(i32, (f64, f64))> {
    let raw = event.downcast::<web_sys::PointerEvent>()?;
    (raw.pointer_type() == "touch").then(|| (raw.pointer_id(), point(event)))
}

/// Keep receiving moves after the pointer leaves the canvas, so a drag that
/// runs off the edge of a panel keeps turning the robot.
#[cfg(feature = "web")]
fn capture(event: &Event<PointerData>) {
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
fn capture(_event: &Event<PointerData>) {}

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
    use spiral_robotics_core::api::{ConnectionStatus, MotorLimits, MotorSummary};
    use spiral_robotics_core::calibration::MotorCalibration;

    use super::*;

    fn robot(calibration: MotorCalibration) -> RobotSummary {
        RobotSummary {
            id: "follower".to_owned(),
            file: "follower.json".to_owned(),
            robot_type: Some("so101_follower".to_owned()),
            port: None,
            motors: vec![MotorSummary {
                name: "shoulder_pan".to_owned(),
                id: 1,
                direction: calibration.direction,
                model: None,
                calibration: Some(calibration),
                limits: MotorLimits::default(),
            }],
            cameras: Vec::new(),
            calibrated: true,
            problem: None,
            connection: ConnectionStatus::default(),
        }
    }

    #[test]
    fn historical_normalized_pose_uses_the_current_arm_calibration() {
        let calibration = MotorCalibration {
            id: 1,
            direction: Direction::Normal,
            range_min: 1_000,
            center: 2_000,
            range_max: 3_000,
        };
        let sample = TimelineArmSample {
            arm: "follower".to_owned(),
            normalized: BTreeMap::from([("shoulder_pan".to_owned(), Some(1.0))]),
            ..TimelineArmSample::default()
        };

        let pose = historical_angles(&robot(calibration), &sample).expect("historical pose");
        assert_eq!(pose.pan_deg, counts_to_degrees(1_000));
    }

    #[test]
    fn historical_pose_does_not_guess_without_calibrated_values() {
        let calibration = MotorCalibration {
            id: 1,
            direction: Direction::Normal,
            range_min: 1_000,
            center: 2_000,
            range_max: 3_000,
        };
        let sample = TimelineArmSample {
            arm: "follower".to_owned(),
            normalized: BTreeMap::from([("shoulder_pan".to_owned(), None)]),
            ..TimelineArmSample::default()
        };
        assert!(historical_angles(&robot(calibration), &sample).is_none());
    }
}
