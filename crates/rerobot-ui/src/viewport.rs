//! The studio's 3D view of the arms.
//!
//! Every arm in the workspace is drawn in one scene, posed from its measured
//! joint angles. What gets drawn comes entirely from the robot description in
//! [`rerobot_core::urdf`], so the shape of the arm is a fact about a file, not
//! about this module.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_elements::geometry::WheelDelta;
use dioxus_elements::input_data::MouseButton;
use rerobot_core::api::RobotSummary;
use rerobot_core::arm::{self, JointAngles};
use rerobot_core::glam::{Mat4, Vec3};
use rerobot_core::urdf::Urdf;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;
use web_sys::HtmlCanvasElement;

use crate::scene::{Camera, Instance, Renderer};
use crate::state::{Selection, studio};

/// The canvas the viewport draws into. There is exactly one on the page.
const CANVAS_ID: &str = "robot-viewport";

/// How far apart two arms stand on the bench, in metres.
const BENCH_SPACING: f32 = 0.34;

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

/// One arm, ready to be turned into shapes.
struct Placement {
    id: String,
    accent: [f32; 4],
    /// False for an arm that is configured but not currently reporting, which
    /// is drawn at its rest pose and faded rather than left out.
    live: bool,
    angles: JointAngles,
    base: Mat4,
}

#[component]
pub fn Viewport() -> Element {
    let mut studio = studio();
    let urdf = crate::state::description();
    let renderer = use_hook(|| Rc::new(RefCell::new(Option::<Renderer>::None)));
    let mut camera = use_signal(Camera::default);
    let mut drag = use_signal(|| Option::<Drag>::None);
    let mut touches = use_signal(BTreeMap::<i32, (f64, f64)>::new);
    let mut pinch = use_signal(|| None::<Pinch>);
    let mut trouble = use_signal(|| Option::<String>::None);
    let mut show_grid = use_signal(|| true);
    // Bumped by the window resize listener: the draw below reads it purely so
    // that a resize redraws even when no telemetry is arriving.
    let mut resized = use_signal(|| 0_u32);

    // Held for the life of the component, and dropped — unsubscribing — with it.
    use_hook(|| {
        Rc::new(WindowListener::new("resize", move || {
            let next = resized.peek().wrapping_add(1);
            resized.set(next);
        }))
    });

    let selected = studio.selected.read().clone();
    let scene_instances = urdf
        .as_ref()
        .as_ref()
        .map(|urdf| instances(urdf, &placements(&studio), selected.as_ref()))
        .unwrap_or_default();
    let scene_urdf = urdf.clone();
    let scene_renderer = renderer.clone();
    use_effect(move || {
        let _ = resized();
        let camera = camera();
        let grid = show_grid();
        let Ok(urdf) = scene_urdf.as_ref() else {
            return;
        };
        let selected = studio.selected.read().clone();
        let draw_instances = instances(urdf, &placements(&studio), selected.as_ref());

        let Some(canvas) = canvas() else { return };
        let mut slot = scene_renderer.borrow_mut();
        if slot.is_none() {
            match Renderer::new(&canvas) {
                Ok(ready) => *slot = Some(ready),
                Err(error) => {
                    trouble.set(Some(error));
                    return;
                }
            }
        }
        let Some(renderer) = slot.as_mut() else {
            return;
        };
        let ratio = web_sys::window().map_or(1.0, |window| window.device_pixel_ratio());
        if let Some(aspect) = renderer.resize(&canvas, ratio) {
            renderer.draw(&camera, aspect, &draw_instances, grid);
        }
    });

    let arms = studio.snapshot.read().robots.clone();
    let failure = match urdf.as_ref() {
        Err(error) => Some(format!("The robot description could not be read: {error}")),
        Ok(_) => trouble(),
    };

    rsx! {
        div { class: "relative h-full w-full overflow-hidden bg-viewport",
            canvas {
                id: CANVAS_ID,
                class: "h-full w-full touch-none select-none",
                class: if drag().is_some() { "cursor-grabbing" } else { "cursor-grab" },
                role: "application",
                tabindex: "0",
                "aria-label": "Interactive 3D robot viewer. Click an arm to select it. Drag to orbit, shift-drag or two-finger drag to pan, pinch or scroll to zoom, and press F to frame the selection.",
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
                    drag.set(Some(Drag::start(&event)));
                },
                onpointermove: move |event| {
                    let position = point(&event);
                    if let Some((pointer, _)) = touch(&event) {
                        if touches.read().contains_key(&pointer) {
                            touches.write().insert(pointer, position);
                            if let Some(current) = Pinch::from_touches(&touches.read()) {
                                if let Some(previous) = pinch() {
                                    let mut camera = camera.write();
                                    if current.distance > 1.0 && previous.distance > 1.0 {
                                        camera.zoom(previous.distance / current.distance);
                                    }
                                    let metres = camera.metres_per_pixel(viewport_height());
                                    camera.pan(
                                        -(current.center.0 - previous.center.0) as f32 * metres,
                                        (current.center.1 - previous.center.1) as f32 * metres,
                                    );
                                }
                                pinch.set(Some(current));
                                return;
                            }
                        }
                    }
                    let Some(active) = drag() else { return };
                    let (dx, dy) = (
                        position.0 - active.last.0,
                        position.1 - active.last.1,
                    );
                    let moved = active.moved
                        || (position.0 - active.start.0).hypot(position.1 - active.start.1) > 3.0;
                    drag.set(Some(Drag { last: position, moved, ..active }));
                    let mut camera = camera.write();
                    // Both gestures are direct manipulation: the drag moves the
                    // scene, not the camera. Dragging down tips the arm's top
                    // toward the viewer — the same way dragging down a globe
                    // brings its north pole around — which puts the camera up.
                    if active.panning {
                        // A pan should keep the point under the pointer under
                        // the pointer, so it moves in world metres per pixel.
                        let metres = camera.metres_per_pixel(viewport_height());
                        camera.pan(-dx as f32 * metres, dy as f32 * metres);
                    } else {
                        camera.orbit(-dx as f32 * 0.007, dy as f32 * 0.007);
                    }
                },
                onpointerup: {
                    let scene_instances = scene_instances.clone();
                    move |event| {
                        if let Some((pointer, _)) = touch(&event) {
                            touches.write().remove(&pointer);
                            pinch.set(None);
                            drag.set(None);
                            return;
                        }
                        if drag().is_some_and(|active| !active.moved) {
                            if let Some(selection) = pick(&camera(), &scene_instances, point(&event)) {
                                studio.selected.set(Some(selection));
                            }
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
                    if touch(&event).is_none() { drag.set(None); }
                },
                oncontextmenu: move |event| event.prevent_default(),
                onwheel: move |event| {
                    // Mice report notches and trackpads report pixels; both are
                    // normalized so one notch zooms by about a sixth, which is
                    // small enough that a trackpad flick does not overshoot.
                    let notches = match event.delta() {
                        WheelDelta::Pixels(delta) => delta.y / 120.0,
                        WheelDelta::Lines(delta) => delta.y / 3.0,
                        WheelDelta::Pages(delta) => delta.y,
                    };
                    camera.write().zoom((notches.clamp(-4.0, 4.0) as f32 * 0.15).exp());
                },
                onkeydown: move |event| match event.key() {
                    Key::ArrowLeft => camera.write().orbit(0.08, 0.0),
                    Key::ArrowRight => camera.write().orbit(-0.08, 0.0),
                    Key::ArrowUp => camera.write().orbit(0.0, -0.08),
                    Key::ArrowDown => camera.write().orbit(0.0, 0.08),
                    Key::Character(value) if value == "+" || value == "=" => {
                        camera.write().zoom(0.88)
                    }
                    Key::Character(value) if value == "-" || value == "_" => {
                        camera.write().zoom(1.14)
                    }
                    Key::Character(value) if value == "0" => camera.set(Camera::default()),
                    Key::Character(value) if value.eq_ignore_ascii_case("f") => {
                        frame_selection(&mut camera, &studio)
                    }
                    _ => {}
                },
            }

            // The viewport is dark in both themes, so its own chrome is spelled
            // out in white at low opacity rather than in the page's ink tones.
            if let Some(failure) = failure {
                div { class: "absolute inset-0 flex items-center justify-center p-6",
                    p { class: "max-w-sm rounded border border-white/10 px-4 py-3 text-center text-[13px] text-white/70",
                        "{failure}"
                    }
                }
            } else {
                Legend { arms }
                div { class: "viewport-help",
                    "click select · drag orbit · shift-drag pan · scroll zoom · F frame"
                }
                button {
                    r#type: "button",
                    class: "viewport-button absolute top-2 right-2",
                    onclick: move |_| camera.set(Camera::default()),
                    title: "Restore the default camera (0)",
                    "Home"
                }
                button {
                    r#type: "button",
                    class: "viewport-button absolute top-2 right-16",
                    "aria-pressed": show_grid(),
                    onclick: move |_| show_grid.toggle(),
                    title: "Show or hide the scale grid",
                    "Grid"
                }
                ViewCube { camera }
                div { class: "viewport-axis-key", span { class: "axis-x", "X" } span { class: "axis-y", "Y" } span { class: "axis-z", "Z" } }
            }
        }
    }
}

/// Which arm is which colour in the scene.
#[component]
fn Legend(arms: Vec<RobotSummary>) -> Element {
    let mut studio = studio();
    if arms.is_empty() {
        return rsx! {
            div { class: "pointer-events-none absolute inset-0 flex items-center justify-center",
                p { class: "text-[13px] text-white/40", "Add an arm to see it here." }
            }
        };
    }

    rsx! {
        ul { class: "absolute top-2 left-3 flex flex-col gap-0.5",
            for (index, arm) in arms.into_iter().enumerate() {
                li { key: "{arm.id}",
                    button {
                        r#type: "button",
                        class: if studio.selected.read().as_ref().is_some_and(|selection| selection.matches_robot(&arm.id)) { "viewport-legend-row viewport-legend-selected" } else { "viewport-legend-row" },
                        title: "Select {arm.id}; press F to frame it",
                        onclick: {
                            let id = arm.id.clone();
                            move |_| studio.selected.set(Some(Selection::Robot(id.clone())))
                        },
                        span { class: "h-2 w-2 shrink-0 rounded-full", style: "background: {accent_css(index)}" }
                        span { "{arm.id}" }
                        if !arm.connection.connected { span { class: "text-white/45", "offline" } }
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
                "TOP"
            }
            div { class: "view-cube-row",
                button {
                    r#type: "button", title: "Front view", "aria-label": "Front view",
                    onclick: move |_| camera.set(Camera { yaw: -std::f32::consts::FRAC_PI_2, pitch: 0.08, ..camera() }),
                    "F"
                }
                button {
                    r#type: "button", title: "Right side view", "aria-label": "Right side view",
                    onclick: move |_| camera.set(Camera { yaw: 0.0, pitch: 0.08, ..camera() }),
                    "R"
                }
            }
        }
    }
}

fn frame_selection(camera: &mut Signal<Camera>, studio: &crate::state::Studio) {
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
    let base = bench_position(index, count).transform_point3(Vec3::ZERO);
    camera.set(Camera {
        target: base + Vec3::new(0.03, 0.0, 0.17),
        distance: 0.68,
        ..camera()
    });
}

/// Ray-cast the visible primitive bounds and return the nearest semantic entity.
fn pick(camera: &Camera, instances: &[Instance], client: (f64, f64)) -> Option<Selection> {
    let canvas = canvas()?;
    let rect = canvas.get_bounding_client_rect();
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let x = ((client.0 - rect.left()) / rect.width() * 2.0 - 1.0) as f32;
    let y = (1.0 - (client.1 - rect.top()) / rect.height() * 2.0) as f32;
    let aspect = (rect.width() / rect.height()) as f32;
    let inverse = camera.view_projection(aspect).inverse();
    let near = inverse.project_point3(Vec3::new(x, y, -1.0));
    let far = inverse.project_point3(Vec3::new(x, y, 1.0));
    let direction = (far - near).normalize_or(Vec3::X);

    instances
        .iter()
        .filter_map(|instance| {
            let radius = match &instance.geometry {
                rerobot_core::urdf::Geometry::Box { size } => size.length() * 0.5,
                rerobot_core::urdf::Geometry::Cylinder { radius, length } => {
                    (radius * radius + length * length * 0.25).sqrt()
                }
                rerobot_core::urdf::Geometry::Sphere { radius } => *radius,
                rerobot_core::urdf::Geometry::Mesh { .. } => return None,
            };
            let center = instance.transform.transform_point3(Vec3::ZERO);
            let to_center = center - near;
            let along = to_center.dot(direction);
            if along < 0.0 {
                return None;
            }
            let distance_squared = to_center.length_squared() - along * along;
            if distance_squared > radius * radius {
                return None;
            }
            let hit = along - (radius * radius - distance_squared).sqrt();
            let selection = instance.joint.as_ref().map_or_else(
                || Selection::Robot(instance.robot.clone()),
                |joint| Selection::Joint {
                    robot: instance.robot.clone(),
                    joint: joint.clone(),
                },
            );
            Some((hit, selection))
        })
        .min_by(|(left, _), (right, _)| left.total_cmp(right))
        .map(|(_, selection)| selection)
}

/// Read every configured arm's current pose out of the studio state.
fn placements(studio: &crate::state::Studio) -> Vec<Placement> {
    let arms = studio.snapshot.read().robots.clone();
    let count = arms.len();
    arms.iter()
        .enumerate()
        .map(|(index, arm)| {
            // An arm that is offline or uncalibrated has no angles worth
            // drawing, so it stands at its rest pose and is faded rather than
            // left out — an empty bench says less than a greyed-out one.
            let telemetry = studio.robot_telemetry(&arm.id).filter(|_| arm.calibrated);
            Placement {
                id: arm.id.clone(),
                accent: ACCENTS[index % ACCENTS.len()].0,
                live: telemetry.is_some(),
                angles: telemetry
                    .map(|robot| crate::control::measured_angles(&robot.joints))
                    .unwrap_or_default(),
                base: bench_position(index, count),
            }
        })
        .collect()
}

/// Stand the arms in a row, centered on the origin.
fn bench_position(index: usize, count: usize) -> Mat4 {
    let offset = (index as f32 - (count.saturating_sub(1)) as f32 / 2.0) * BENCH_SPACING;
    Mat4::from_translation(Vec3::new(0.0, offset, 0.0))
}

/// Turn posed arms into the shapes the renderer draws.
fn instances(urdf: &Urdf, placements: &[Placement], selected: Option<&Selection>) -> Vec<Instance> {
    let mut instances = Vec::new();
    for placement in placements {
        let positions = arm::so101_joint_positions(urdf, &placement.angles);
        let transforms = urdf.pose(&positions);
        for (link_index, link) in urdf.links.iter().enumerate() {
            let joint = controlling_joint(urdf, link_index);
            for visual in &link.visuals {
                let color = match visual.material.as_deref() {
                    Some("accent") => placement.accent,
                    _ => visual.color,
                };
                let is_selected = match selected {
                    Some(Selection::Robot(robot)) => robot == &placement.id,
                    Some(Selection::Joint { robot, joint: selected_joint }) => {
                        robot == &placement.id && joint.as_deref() == Some(selected_joint.as_str())
                    }
                    _ => false,
                };
                let color = if is_selected { highlighted(color) } else { color };
                instances.push(Instance {
                    geometry: visual.geometry.clone(),
                    transform: placement.base * transforms[link_index] * visual.origin.matrix(),
                    color: if placement.live { color } else { faded(color) },
                    robot: placement.id.clone(),
                    joint: joint.clone(),
                });
            }
        }
    }
    instances
}

fn controlling_joint(urdf: &Urdf, mut link: usize) -> Option<String> {
    while link != urdf.root() {
        let joint = urdf.joints.iter().find(|joint| joint.child == link)?;
        if joint.kind.is_movable() {
            return Some(joint.name.clone());
        }
        link = joint.parent;
    }
    None
}

fn highlighted(color: [f32; 4]) -> [f32; 4] {
    [
        (color[0] * 1.22 + 0.08).min(1.0),
        (color[1] * 1.22 + 0.08).min(1.0),
        (color[2] * 1.22 + 0.08).min(1.0),
        color[3],
    ]
}

/// Wash a colour out toward the background, marking an arm that is not
/// reporting so a stale pose is never mistaken for a live one.
fn faded(color: [f32; 4]) -> [f32; 4] {
    const TOWARD: [f32; 3] = [0.30, 0.30, 0.29];
    let mix = |value: f32, target: f32| value + (target - value) * 0.72;
    [
        mix(color[0], TOWARD[0]),
        mix(color[1], TOWARD[1]),
        mix(color[2], TOWARD[2]),
        color[3],
    ]
}

/// Where the gripper tip sits for `angles`, in millimetres from the arm's base.
///
/// The description carries a tool frame for exactly this, so no length written
/// here has to agree with one written there.
pub fn tool_position(urdf: &Urdf, angles: &JointAngles) -> Option<Vec3> {
    let index = urdf.link_index("tool_link")?;
    let transforms = urdf.pose(&arm::so101_joint_positions(urdf, angles));
    Some(transforms.get(index)?.transform_point3(Vec3::ZERO) * 1000.0)
}

/// The state of a drag in progress.
#[derive(Clone, Copy, PartialEq)]
struct Drag {
    /// Pointer position at the beginning, to separate clicking from orbiting.
    start: (f64, f64),
    /// Pointer position when this drag was last sampled, in client pixels.
    last: (f64, f64),
    panning: bool,
    moved: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct Pinch {
    center: (f64, f64),
    distance: f32,
}

impl Pinch {
    fn from_touches(touches: &BTreeMap<i32, (f64, f64)>) -> Option<Self> {
        let mut points = touches.values();
        let (first, second) = (*points.next()?, *points.next()?);
        Some(Self {
            center: ((first.0 + second.0) * 0.5, (first.1 + second.1) * 0.5),
            distance: (second.0 - first.0).hypot(second.1 - first.1) as f32,
        })
    }
}

impl Drag {
    fn start(event: &Event<PointerData>) -> Self {
        // The middle and right buttons pan, matching every other 3D viewer;
        // shift with the left button does the same for a one-button mouse.
        let panning = event.modifiers().shift()
            || matches!(
                event.trigger_button(),
                Some(MouseButton::Secondary | MouseButton::Auxiliary)
            );
        Self {
            start: point(event),
            last: point(event),
            panning,
            moved: false,
        }
    }
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

fn canvas() -> Option<HtmlCanvasElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id(CANVAS_ID)?
        .dyn_into()
        .ok()
}

/// The canvas height in CSS pixels, used to turn a drag into world metres.
fn viewport_height() -> f32 {
    canvas().map_or(1.0, |canvas| canvas.client_height().max(1) as f32)
}

/// A window event subscription that ends when the viewport does.
struct WindowListener {
    event: &'static str,
    callback: Closure<dyn FnMut()>,
}

impl WindowListener {
    fn new(event: &'static str, handler: impl FnMut() + 'static) -> Option<Self> {
        let callback = Closure::wrap(Box::new(handler) as Box<dyn FnMut()>);
        web_sys::window()?
            .add_event_listener_with_callback(event, callback.as_ref().unchecked_ref())
            .ok()?;
        Some(Self { event, callback })
    }
}

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
