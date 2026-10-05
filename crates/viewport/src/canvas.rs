//! A browser canvas the bench draws into.
//!
//! The canvas's WebGL2 context becomes a wgpu surface, so the browser draws
//! with exactly the renderer the robot connector films its cameras with.

use web_sys::HtmlCanvasElement;

use crate::render::{Gpu, RenderFrame, Renderer, ViewportError};
use crate::robot::Frame;

/// A canvas, the surface over it, and the renderer that draws there.
pub(crate) struct CanvasOutput {
    canvas: HtmlCanvasElement,
    surface: wgpu::Surface<'static>,
    configuration: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    renderer: Renderer,
}

impl CanvasOutput {
    /// Bind to `canvas`, or say why the browser could not.
    pub(crate) async fn new(canvas: HtmlCanvasElement) -> Result<Self, ViewportError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = Self::canvas_surface(&instance, &canvas)?;
        let (gpu, adapter) = Gpu::request(&instance, Some(&surface)).await?;
        let capabilities = surface.get_capabilities(&adapter);
        // Colours are written as the display shows them, as the canvas's own
        // WebGL always did, rather than re-encoded as sRGB.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| ViewportError::Rendering("the canvas offers no colour format".into()))?;
        let alpha_mode = capabilities
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        let configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            // Sized on the first draw, once the canvas has been laid out.
            width: 0,
            height: 0,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: Vec::new(),
        };
        let renderer = Renderer::new(&gpu, format);
        Ok(Self {
            canvas,
            surface,
            configuration,
            gpu,
            renderer,
        })
    }

    #[cfg(target_arch = "wasm32")]
    fn canvas_surface(
        instance: &wgpu::Instance,
        canvas: &HtmlCanvasElement,
    ) -> Result<wgpu::Surface<'static>, ViewportError> {
        instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|error| {
                ViewportError::Rendering(format!("this browser refused a WebGL2 canvas: {error}"))
            })
    }

    /// The studio's server build renders components too, but only a browser
    /// has a canvas to draw on.
    #[cfg(not(target_arch = "wasm32"))]
    fn canvas_surface(
        _instance: &wgpu::Instance,
        _canvas: &HtmlCanvasElement,
    ) -> Result<wgpu::Surface<'static>, ViewportError> {
        Err(ViewportError::Rendering(
            "only a browser has a canvas to draw on".into(),
        ))
    }

    pub(crate) const fn canvas(&self) -> &HtmlCanvasElement {
        &self.canvas
    }

    /// Upload the meshes of the robots in `frame`, saying whether they changed.
    pub(crate) fn load_models(&mut self, frame: &Frame) -> bool {
        self.renderer.load_models(&self.gpu, frame)
    }

    /// Match the drawing buffer to the space the canvas actually occupies.
    ///
    /// Returns the aspect ratio, or `None` while the canvas has no area — a
    /// panel that is still laid out at zero height would otherwise produce a
    /// degenerate projection.
    pub(crate) fn resize(&mut self, pixel_ratio: f64) -> Option<f32> {
        let scale = pixel_ratio.clamp(1.0, 3.0);
        let largest = self.gpu.device().limits().max_texture_dimension_2d;
        let pixels = |css: i32| ((css.max(0) as f64 * scale).round() as u32).min(largest);
        let (width, height) = (
            pixels(self.canvas.client_width()),
            pixels(self.canvas.client_height()),
        );
        if width == 0 || height == 0 {
            return None;
        }
        if (self.configuration.width, self.configuration.height) != (width, height) {
            self.canvas.set_width(width);
            self.canvas.set_height(height);
            self.configuration.width = width;
            self.configuration.height = height;
            self.surface
                .configure(self.gpu.device(), &self.configuration);
        }
        Some(width as f32 / height as f32)
    }

    /// Draw one frame and show it, reporting whether the canvas took it.
    pub(crate) fn draw(&mut self, frame: RenderFrame<'_>) -> Result<bool, ViewportError> {
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface
                    .configure(self.gpu.device(), &self.configuration);
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(ViewportError::Rendering(
                    "the canvas surface is misconfigured".into(),
                ));
            }
        };
        self.renderer.draw(&self.gpu, &texture.texture, frame);
        self.gpu.queue().present(texture);
        Ok(true)
    }
}
