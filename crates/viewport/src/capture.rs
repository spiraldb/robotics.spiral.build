//! What a camera mounted on the bench sees, drawn off screen.
//!
//! The robot connector films its simulated arms with this: the scene the
//! operator's bench draws, seen from a mounted camera instead of the orbit
//! camera, and read back as pixels.

use crate::render::{Gpu, RenderFrame, Renderer, ViewportError};
use crate::robot::{Frame, Frustums, RobotScene};

/// Pictures are plain 8-bit RGBA, ready to hand to an image encoder.
const PICTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A graphics device that draws mounted cameras' views of one size.
pub struct CameraRenderer {
    gpu: Gpu,
    renderer: Renderer,
    picture: Picture,
}

impl CameraRenderer {
    /// Open a graphics device for pictures `width` by `height` pixels.
    ///
    /// `WGPU_BACKEND` may name the backend, as for any wgpu program.
    pub async fn new(width: u32, height: u32) -> Result<Self, ViewportError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let (gpu, _) = Gpu::request(&instance, None).await?;
        let largest = gpu.device().limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > largest || height > largest {
            return Err(ViewportError::Rendering(format!(
                "a {width}×{height} picture is outside what this device can draw"
            )));
        }
        Ok(Self {
            renderer: Renderer::new(&gpu, PICTURE_FORMAT),
            picture: Picture::new(gpu.device(), width, height),
            gpu,
        })
    }

    /// What the camera mounted on `robot` as `role` sees in `frame`: RGBA
    /// rows from the top of the picture down.
    pub fn capture(
        &mut self,
        frame: &Frame,
        robot: &str,
        role: &str,
    ) -> Result<Vec<u8>, ViewportError> {
        let view = frame.camera_view(robot, role).ok_or_else(|| {
            ViewportError::Rendering(format!("no camera {role:?} is mounted on {robot:?}"))
        })?;
        self.renderer.load_models(&self.gpu, frame);
        let scene = RobotScene::compose(frame, Frustums::Hidden);
        let aspect_ratio = self.picture.width as f32 / self.picture.height as f32;
        self.renderer.draw(
            &self.gpu,
            &self.picture.texture,
            RenderFrame::new(view.view_projection(aspect_ratio), scene.scene()),
        );
        self.picture.read(&self.gpu)
    }
}

/// The texture a capture resolves into, and the buffer it is read back through.
struct Picture {
    texture: wgpu::Texture,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
}

impl Picture {
    const BYTES_PER_PIXEL: u32 = 4;

    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("camera picture"),
            size: Self::extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PICTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera picture readback"),
            size: u64::from(Self::padded_row(width)) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            texture,
            readback,
            width,
            height,
        }
    }

    const fn extent(width: u32, height: u32) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        }
    }

    /// A copy's rows start on wgpu's 256-byte boundaries.
    const fn padded_row(width: u32) -> u32 {
        (width * Self::BYTES_PER_PIXEL).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
    }

    /// Copy the drawn picture out of the GPU, waiting for it to finish.
    fn read(&self, gpu: &Gpu) -> Result<Vec<u8>, ViewportError> {
        let padded_row = Self::padded_row(self.width);
        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("camera picture readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            Self::extent(self.width, self.height),
        );
        gpu.queue().submit([encoder.finish()]);

        let slice = self.readback.slice(..);
        let (mapped, ready) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = mapped.send(result);
        });
        let failed = |detail: String| {
            ViewportError::Rendering(format!("reading a camera picture: {detail}"))
        };
        gpu.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| failed(error.to_string()))?;
        ready
            .recv()
            .map_err(|error| failed(error.to_string()))?
            .map_err(|error| failed(error.to_string()))?;

        let row = (self.width * Self::BYTES_PER_PIXEL) as usize;
        let mapped = slice
            .get_mapped_range()
            .map_err(|error| failed(error.to_string()))?;
        let mut pixels = Vec::with_capacity(row * self.height as usize);
        for padded in mapped.chunks(padded_row as usize) {
            pixels.extend_from_slice(&padded[..row]);
        }
        drop(mapped);
        self.readback.unmap();
        Ok(pixels)
    }
}

/// Native only: a browser test cannot block on the graphics device.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::collections::BTreeMap;
    use std::rc::Rc;

    use super::*;
    use crate::{ArmPresentation, CameraAttachment, CameraMount, RobotModel};

    const RED_ARM: &str = r#"<robot name="arm"><link name="base"/><link name="arm"><visual><origin xyz="0 0 0.05"/><geometry><box size="0.1 0.1 0.1"/></geometry><material name="red"><color rgba="0.9 0.1 0.1 1"/></material></visual></link><joint name="hinge" type="prismatic"><parent link="base"/><child link="arm"/><axis xyz="0 1 0"/><limit lower="-1" upper="1"/></joint></robot>"#;

    /// A camera standing 60 cm out along +X, looking back at the arm.
    fn frame(slide: f32) -> Frame {
        let model = Rc::new(RobotModel::parse(RED_ARM, &BTreeMap::new()).unwrap());
        let mut frame = Frame::new(false);
        frame
            .add_arm(
                "arm",
                ArmPresentation::new(model, BTreeMap::from([("hinge".into(), slide)])),
            )
            .unwrap();
        let facing_the_arm = CameraAttachment::new(
            None::<String>,
            [0.6, 0.0, 0.05],
            [0.0, -std::f64::consts::FRAC_PI_2, 0.0],
        );
        frame
            .mount_camera("arm", CameraMount::new("front", facing_the_arm, 60.0))
            .unwrap();
        frame
    }

    fn is_red(pixel: &[u8]) -> bool {
        pixel[0] > 150 && pixel[1] < 90 && pixel[2] < 90
    }

    #[test]
    fn a_mounted_camera_films_the_arm_it_points_at() {
        let (width, height) = (64, 48);
        let mut camera = pollster::block_on(CameraRenderer::new(width, height)).unwrap();

        let centred = camera.capture(&frame(0.0), "arm", "front").unwrap();
        assert_eq!(centred.len(), (width * height * 4) as usize);
        let centre = ((height / 2 * width + width / 2) * 4) as usize;
        assert!(
            is_red(&centred[centre..centre + 4]),
            "the arm fills the middle"
        );
        assert!(!is_red(&centred[..4]), "the bench shows around it");

        let moved = camera.capture(&frame(0.5), "arm", "front").unwrap();
        assert!(
            !is_red(&moved[centre..centre + 4]),
            "a moved arm leaves the middle"
        );
        assert!(camera.capture(&frame(0.0), "arm", "wrist").is_err());
    }
}
