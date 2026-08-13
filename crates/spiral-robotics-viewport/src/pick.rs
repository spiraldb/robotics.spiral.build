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
use crate::render::Renderer;
use crate::scene::{EntityKey, Scene};

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
    pub fn draw(&mut self, camera: &Camera, scene: &Scene, pixel_ratio: f64) {
        if let Some(aspect) = self.renderer.resize(&self.canvas, pixel_ratio) {
            self.renderer.draw(camera, aspect, scene);
        }
    }

    /// The canvas height in CSS pixels, which is what turns a drag into metres.
    pub fn height(&self) -> f32 {
        self.canvas.client_height().max(1) as f32
    }

    /// The nearest pickable entity under a client-space point.
    pub fn pick(&self, camera: &Camera, scene: &Scene, client: (f64, f64)) -> Option<EntityKey> {
        let (near, direction) = self.ray(camera, client)?;
        scene
            .entities
            .iter()
            .filter(|entity| entity.pickable)
            .filter_map(|entity| {
                let radius = bounding_radius(&entity.geometry)?;
                let to_center = entity.origin() - near;
                let along = to_center.dot(direction);
                if along < 0.0 {
                    return None;
                }
                let offset = to_center.length_squared() - along * along;
                if offset > radius * radius {
                    return None;
                }
                Some((along - (radius * radius - offset).sqrt(), entity.key))
            })
            .min_by(|(left, _), (right, _)| left.total_cmp(right))
            .map(|(_, key)| key)
    }

    /// Where a world point lands on the canvas, in client pixels.
    ///
    /// `None` behind the camera, so a label for something that has orbited out
    /// of view is not drawn folded onto the opposite edge of the screen.
    pub fn anchor(&self, camera: &Camera, world: Vec3) -> Option<(f64, f64)> {
        let rect = self.canvas.get_bounding_client_rect();
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let aspect = (rect.width() / rect.height()) as f32;
        let clip = camera.view_projection(aspect) * world.extend(1.0);
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
    fn ray(&self, camera: &Camera, client: (f64, f64)) -> Option<(Vec3, Vec3)> {
        let rect = self.canvas.get_bounding_client_rect();
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let x = ((client.0 - rect.left()) / rect.width() * 2.0 - 1.0) as f32;
        let y = (1.0 - (client.1 - rect.top()) / rect.height() * 2.0) as f32;
        let aspect = (rect.width() / rect.height()) as f32;
        let inverse: Mat4 = camera.view_projection(aspect).inverse();
        let near = inverse.project_point3(Vec3::new(x, y, -1.0));
        let far = inverse.project_point3(Vec3::new(x, y, 1.0));
        Some((near, (far - near).normalize_or(Vec3::X)))
    }
}

/// A sphere that contains the primitive, for a first-pass ray test.
///
/// Meshes have no bounds this crate can compute without loading them, so they
/// are not picked rather than picked at a guessed size.
fn bounding_radius(geometry: &Geometry) -> Option<f32> {
    match geometry {
        Geometry::Box { size } => Some(size.length() * 0.5),
        Geometry::Cylinder { radius, length } => {
            Some((radius * radius + length * length * 0.25).sqrt())
        }
        Geometry::Sphere { radius } => Some(*radius),
        Geometry::Mesh { .. } => None,
    }
}
