//! Turning screen points into entities, and world points into screen points.
//!
//! These two are one module because they are the same transform read in
//! opposite directions, and together they are the whole of what the
//! application needs from the scene that is not "draw it": *which thing is
//! under the pointer*, and *where on the canvas is that thing*. Everything the
//! studio draws over the bench — a hover callout, a condition mark — is built
//! out of those two answers and nothing else.

use glam::{Mat4, Vec3};

use crate::urdf::Geometry;
use web_sys::HtmlCanvasElement;

use crate::camera::Camera;
use crate::render::{RenderFrame, Renderer};
use crate::scene::{EntityKey, Scene};

/// Everything that varies between two rendered frames.
pub struct SceneFrame<'a> {
    camera: &'a Camera,
    scene: &'a Scene,
    pixel_ratio: f64,
}

impl<'a> SceneFrame<'a> {
    pub fn new(camera: &'a Camera, scene: &'a Scene, pixel_ratio: f64) -> Self {
        Self {
            camera,
            scene,
            pixel_ratio,
        }
    }
}

/// The scene and screen position used by one picking query.
pub struct PickRequest<'a> {
    camera: &'a Camera,
    scene: &'a Scene,
    client_position: (f64, f64),
}

impl<'a> PickRequest<'a> {
    pub fn new(camera: &'a Camera, scene: &'a Scene, client_position: (f64, f64)) -> Self {
        Self {
            camera,
            scene,
            client_position,
        }
    }
}

/// A canvas, the renderer bound to it, and the geometry to read against it.
pub struct Viewport {
    canvas: HtmlCanvasElement,
    renderer: Renderer,
}

impl Viewport {
    /// Bind to a canvas, compiling the shader program.
    pub fn new(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let renderer = Renderer::new(&canvas)?;
        Ok(Self { canvas, renderer })
    }

    /// Draw the scene, resizing the drawing buffer to the canvas first.
    pub fn draw(&mut self, frame: SceneFrame<'_>) {
        if let Some(aspect_ratio) = self.renderer.resize(&self.canvas, frame.pixel_ratio) {
            self.renderer
                .draw(RenderFrame::new(frame.camera, aspect_ratio, frame.scene));
        }
    }

    /// The canvas height in CSS pixels, which is what turns a drag into metres.
    pub fn height(&self) -> f32 {
        self.canvas.client_height().max(1) as f32
    }

    /// The nearest pickable entity under a client-space point.
    pub fn pick(&self, request: PickRequest<'_>) -> Option<EntityKey> {
        let (ray_origin, ray_direction) = self.ray(request.camera, request.client_position)?;
        request
            .scene
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
            .map(|(_, key)| key)
    }

    /// Where a world point lands on the canvas, in client pixels.
    ///
    /// `None` behind the camera, so a label for something that has orbited out
    /// of view is not drawn folded onto the opposite edge of the screen.
    pub fn anchor(&self, camera: &Camera, world_position: Vec3) -> Option<(f64, f64)> {
        let rect = self.canvas.get_bounding_client_rect();
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let aspect = (rect.width() / rect.height()) as f32;
        let clip = camera.view_projection(aspect) * world_position.extend(1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if !(-1.0..=1.0).contains(&ndc.z) {
            return None;
        }
        Some((
            rect.left() + (ndc.x as f64 * 0.5 + 0.5) * rect.width(),
            rect.top() + (0.5 - ndc.y as f64 * 0.5) * rect.height(),
        ))
    }

    /// The world-space ray through a client-space point.
    fn ray(&self, camera: &Camera, client_position: (f64, f64)) -> Option<(Vec3, Vec3)> {
        let rect = self.canvas.get_bounding_client_rect();
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let normalized_x = ((client_position.0 - rect.left()) / rect.width() * 2.0 - 1.0) as f32;
        let normalized_y = (1.0 - (client_position.1 - rect.top()) / rect.height() * 2.0) as f32;
        let aspect = (rect.width() / rect.height()) as f32;
        let inverse_projection: Mat4 = camera.view_projection(aspect).inverse();
        let ray_origin =
            inverse_projection.project_point3(Vec3::new(normalized_x, normalized_y, -1.0));
        let ray_end = inverse_projection.project_point3(Vec3::new(normalized_x, normalized_y, 1.0));
        Some((ray_origin, (ray_end - ray_origin).normalize_or(Vec3::X)))
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
