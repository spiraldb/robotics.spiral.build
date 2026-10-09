//! A camera mounted on a simulated arm, filmed by drawing the scene.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use image::RgbaImage;
use image::codecs::jpeg::JpegEncoder;
use spiral_robotics_common::robots::{CameraStatus, CameraStreamFormat};
use spiral_robotics_viewport::CameraRenderer;

use super::SimulatedWorld;
use super::world::ParsedModels;
use crate::robotics::cameras::{CameraDevice, DEFAULT_JPEG_QUALITY};

/// A webcam's picture, so a simulated episode records like a real one.
const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const FPS: u32 = 30;

#[derive(Clone)]
pub(crate) struct SimulatedCamera {
    world: SimulatedWorld,
    /// The configured device ID the camera is streamed and recorded under.
    id: String,
    robot: String,
    role: String,
}

impl SimulatedCamera {
    pub(super) fn new(world: SimulatedWorld, (robot, role): (&str, &str), id: &str) -> Self {
        Self {
            world,
            id: id.to_owned(),
            robot: robot.to_owned(),
            role: role.to_owned(),
        }
    }

    /// The encoder drops alpha itself, so the picture goes in as drawn.
    fn encode(rgba: Vec<u8>) -> Result<Vec<u8>, String> {
        let picture = RgbaImage::from_raw(WIDTH, HEIGHT, rgba)
            .ok_or("a camera picture has the wrong size")?;
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, DEFAULT_JPEG_QUALITY)
            .encode_image(&picture)
            .map_err(|error| error.to_string())?;
        Ok(jpeg)
    }
}

impl CameraDevice for SimulatedCamera {
    fn status(&self) -> CameraStatus {
        CameraStatus::new(
            self.id.clone(),
            format!("Simulated {} camera on {}", self.role, self.robot),
            CameraStreamFormat::new(WIDTH, HEIGHT, FPS),
        )
    }

    /// Draw and encode a frame at the camera's rate until `stop` is set.
    fn stream_jpeg_frames(
        &self,
        stop: &AtomicBool,
        publish: &mut dyn FnMut(Vec<u8>, u64),
    ) -> Result<(), String> {
        let mut renderer = pollster::block_on(CameraRenderer::new(WIDTH, HEIGHT))
            .map_err(|error| error.to_string())?;
        let mut models = ParsedModels::default();
        let period = Duration::from_secs(1) / FPS;
        let mut next_frame = Instant::now();
        let mut sequence = 0_u64;
        while !stop.load(Ordering::Relaxed) {
            let frame = self.world.frame(&mut models)?;
            let pixels = renderer
                .capture(&frame, &self.robot, &self.role)
                .map_err(|error| error.to_string())?;
            sequence += 1;
            publish(Self::encode(pixels)?, sequence);

            // An absolute schedule, like the control loop's: a late frame is
            // not followed by a burst of catch-up frames.
            next_frame += period;
            match next_frame.checked_duration_since(Instant::now()) {
                Some(remaining) => thread::sleep(remaining),
                None => next_frame = Instant::now(),
            }
        }
        Ok(())
    }
}
