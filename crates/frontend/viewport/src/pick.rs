//! Turning screen points into entities, and world points into screen points.
//!
//! These two are one module because they are the same transform read in
//! opposite directions, and together they are the whole of what the
//! application needs from the scene that is not "draw it": *which thing is
//! under the pointer*, and *where on the canvas is that thing*. Everything the
//! studio draws over the bench — a hover callout, a condition mark — is built
//! out of those two answers and nothing else.

use glam::{Mat4, Vec3};
use std::fmt;

use crate::urdf::Geometry;
use web_sys::HtmlCanvasElement;

use crate::RobotModel;
use crate::camera::Camera;
use crate::gesture::{Command, Drag, DragUpdate};
use crate::render::{RenderFrame, Renderer};
use crate::robot::{Frame, RobotScene, Target};
use std::rc::Rc;

/// Why a canvas could not become a viewport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewportError {
    Rendering(String),
}

impl fmt::Display for ViewportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rendering(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for ViewportError {}

/// A canvas, the renderer bound to it, and the geometry to read against it.
pub struct Viewport {
    canvas: HtmlCanvasElement,
    renderer: Renderer,
    models: Vec<(String, Rc<RobotModel>)>,
    camera: Camera,
    frame: Frame,
    scene: RobotScene,
    drag: Option<Drag>,
}

impl Viewport {
    /// Bind to a canvas, compiling the shader program.
    pub fn new(canvas: HtmlCanvasElement) -> Result<Self, ViewportError> {
        let renderer = Renderer::new(&canvas).map_err(ViewportError::Rendering)?;

        Ok(Self {
            canvas,
            renderer,
            models: Vec::new(),
            camera: Camera::default(),
            frame: Frame::default(),
            scene: RobotScene::default(),
            drag: None,
        })
    }

    /// Replace and draw the current frame.
    ///
    /// Picking and anchors subsequently read this exact composed scene, so an
    /// overlay can never be calculated from a separately rebuilt pose.
    pub fn draw(&mut self, frame: &Frame, pixel_ratio: f64) -> Result<bool, ViewportError> {
        self.frame = frame.clone();
        let models = frame
            .models()
            .map(|(id, model)| (id.to_owned(), model.clone()))
            .collect::<Vec<_>>();
        if models.len() != self.models.len()
            || models
                .iter()
                .zip(&self.models)
                .any(|((id, model), (old_id, old))| id != old_id || !Rc::ptr_eq(model, old))
        {
            self.renderer
                .load_meshes(
                    &models
                        .iter()
                        .flat_map(|(id, model)| {
                            model
                                .meshes
                                .iter()
                                .map(move |(name, values)| (format!("{id}/{name}"), values.clone()))
                        })
                        .collect(),
                )
                .map_err(ViewportError::Rendering)?;
            self.frame_robot(None);
            self.models = models;
        }
        self.scene = RobotScene::compose(frame);
        if let Some(aspect_ratio) = self.renderer.resize(&self.canvas, pixel_ratio) {
            self.renderer.draw(RenderFrame::new(
                &self.camera,
                aspect_ratio,
                self.scene.scene(),
            ));
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn height(&self) -> f32 {
        self.canvas.client_height().max(1) as f32
    }

    /// Begin direct manipulation at a client-space pointer position.
    pub fn begin_drag(&mut self, client_position: (f64, f64), panning: bool) {
        self.drag = client_position
            .is_finite()
            .then(|| Drag::new(client_position, panning));
    }

    /// Move an active drag and report whether there was one to move.
    pub fn drag_to(&mut self, client_position: (f64, f64)) -> bool {
        if !client_position.is_finite() {
            return false;
        }
        let height = self.height();
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        drag.advance(&mut self.camera, DragUpdate::new(client_position, height));
        true
    }

    /// Whether the active pointer sequence has crossed the click slop.
    pub fn drag_moved(&self) -> bool {
        self.drag.is_some_and(|drag| drag.moved())
    }

    pub fn end_drag(&mut self) {
        self.drag = None;
    }

    /// Apply one discrete navigation command.
    pub fn command(&mut self, command: Command) {
        if command == Command::Home {
            self.frame_robot(None);
        } else {
            command.apply(&mut self.camera);
        }
    }

    /// Frame one arm in its current pose, or all arms when no arm is named.
    pub fn frame_robot(&mut self, robot: Option<&str>) -> bool {
        let Some((target, distance)) = self.frame.framing(robot) else {
            return false;
        };
        let aspect = self.canvas.client_width().max(1) as f32 / self.height();
        self.camera = Camera::framed_on(target, distance / aspect.min(1.0));
        true
    }

    /// The nearest pickable entity under a client-space point.
    pub fn pick(&self, client_position: (f64, f64)) -> Option<Target> {
        let (ray_origin, ray_direction) = self.ray(client_position)?;
        let key = self
            .scene
            .scene()
            .entities()
            .filter(|entity| entity.is_pickable())
            .filter_map(|entity| {
                let radius = entity.geometry().bounding_radius()?;
                let center_offset = entity.origin() - ray_origin;
                let distance_along_ray = center_offset.dot(ray_direction);
                if distance_along_ray < 0.0 {
                    return None;
                }
                let perpendicular_distance_squared =
                    center_offset.length_squared() - distance_along_ray * distance_along_ray;
                if perpendicular_distance_squared > radius * radius {
                    return None;
                }
                Some((
                    distance_along_ray - (radius * radius - perpendicular_distance_squared).sqrt(),
                    entity.key(),
                ))
            })
            .min_by(|(left, _), (right, _)| left.total_cmp(right))
            .map(|(_, key)| key)?;
        self.scene.target(key).cloned()
    }

    /// Where a target in the current frame lands inside the canvas, in CSS pixels.
    ///
    /// `None` behind the camera, so a label for something that has orbited out
    /// of view is not drawn folded onto the opposite edge of the screen.
    pub fn anchor(&self, target: &Target) -> Option<(f64, f64)> {
        let key = self.scene.key(target)?;
        let world_position = self.scene.scene().anchor_point(key)?;
        let rect = self.canvas.get_bounding_client_rect();
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let aspect = (rect.width() / rect.height()) as f32;
        let clip = self.camera.view_projection(aspect) * world_position.extend(1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if !(-1.0..=1.0).contains(&ndc.z) {
            return None;
        }
        Some((
            (ndc.x as f64 * 0.5 + 0.5) * rect.width(),
            (0.5 - ndc.y as f64 * 0.5) * rect.height(),
        ))
    }

    /// The world-space ray through a client-space point.
    fn ray(&self, client_position: (f64, f64)) -> Option<(Vec3, Vec3)> {
        let rect = self.canvas.get_bounding_client_rect();
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let normalized_x = ((client_position.0 - rect.left()) / rect.width() * 2.0 - 1.0) as f32;
        let normalized_y = (1.0 - (client_position.1 - rect.top()) / rect.height() * 2.0) as f32;
        let aspect = (rect.width() / rect.height()) as f32;
        let inverse_projection: Mat4 = self.camera.view_projection(aspect).inverse();
        let ray_origin =
            inverse_projection.project_point3(Vec3::new(normalized_x, normalized_y, -1.0));
        let ray_end = inverse_projection.project_point3(Vec3::new(normalized_x, normalized_y, 1.0));
        Some((ray_origin, (ray_end - ray_origin).normalize_or(Vec3::X)))
    }
}

trait FinitePoint {
    fn is_finite(&self) -> bool;
}

impl FinitePoint for (f64, f64) {
    fn is_finite(&self) -> bool {
        self.0.is_finite() && self.1.is_finite()
    }
}

/// A sphere that contains the primitive, for a first-pass ray test.
///
/// Meshes have no bounds this crate can compute without loading them, so they
/// are not picked rather than picked at a guessed size.
impl Geometry {
    fn bounding_radius(&self) -> Option<f32> {
        match self {
            Self::Box { size } => Some(size.length() * 0.5),
            Self::Cylinder { radius, length } => {
                Some((radius * radius + length * length * 0.25).sqrt())
            }
            Self::Sphere { radius } => Some(*radius),
            Self::Mesh { .. } => None,
        }
    }
}
