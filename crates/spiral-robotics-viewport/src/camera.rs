//! The orbit camera.
//!
//! The camera turns about a target rather than flying freely, which is what
//! makes dragging feel like turning the robot around instead of steering past
//! it. Nothing here knows what is being looked at.

use glam::camera::rh::proj::opengl;
use glam::camera::rh::view::look_at_mat4;
use glam::{Mat4, Vec3};

/// Where the viewer is looking from.
///
/// The camera orbits a target rather than flying freely, which is what makes
/// dragging feel like turning the robot around instead of steering past it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Rotation about the vertical axis, radians.
    pub yaw: f32,
    /// Elevation above the horizon, radians.
    pub pitch: f32,
    /// Distance from the target, metres.
    pub distance: f32,
    pub target: Vec3,
}

impl Default for Camera {
    fn default() -> Self {
        // Three-quarter view from the front right, high enough to read the
        // shoulder and elbow apart, framed on the middle of a working arm.
        Self {
            yaw: -0.65,
            pitch: 0.32,
            distance: 0.78,
            target: Vec3::new(0.03, 0.0, 0.17),
        }
    }
}

/// How far the camera may tip before looking straight down the world's up axis,
/// where the view direction and the up vector stop defining a rotation.
const PITCH_LIMIT: f32 = 1.45;
const MIN_DISTANCE: f32 = 0.18;
const MAX_DISTANCE: f32 = 3.0;

impl Camera {
    /// Turn the camera about its target, in radians.
    pub fn orbit(&mut self, yaw: f32, pitch: f32) {
        self.yaw += yaw;
        self.pitch = (self.pitch + pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Move closer or further away. `factor` above one pulls back.
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Slide the target across the screen, in metres of world space.
    pub fn pan(&mut self, right: f32, up: f32) {
        let (right_axis, up_axis) = self.screen_axes();
        self.target += right_axis * right + up_axis * up;
    }

    /// Where the camera sits.
    pub fn eye(&self) -> Vec3 {
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        self.target + Vec3::new(cos_pitch * cos_yaw, cos_pitch * sin_yaw, sin_pitch) * self.distance
    }

    /// The world directions that screen right and screen up correspond to.
    fn screen_axes(&self) -> (Vec3, Vec3) {
        let forward = (self.target - self.eye()).normalize_or(Vec3::X);
        let right = forward.cross(Vec3::Z).normalize_or(Vec3::Y);
        (right, right.cross(forward))
    }

    /// How many metres one screen pixel covers at the target, so panning and
    /// the model keep pace no matter how far out the camera has been pulled.
    pub fn metres_per_pixel(&self, viewport_height: f32) -> f32 {
        if viewport_height <= 0.0 {
            return 0.0;
        }
        2.0 * self.distance * (FIELD_OF_VIEW / 2.0).tan() / viewport_height
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        // Right handed, with the depth range OpenGL and WebGL expect. Near and
        // far bracket a tabletop scene: close enough to push the camera almost
        // into a joint, far enough to pull back off the bench.
        let projection = opengl::perspective(FIELD_OF_VIEW, aspect.max(0.01), 0.02, 20.0);
        projection * look_at_mat4(self.eye(), self.target, Vec3::Z)
    }
}

const FIELD_OF_VIEW: f32 = 0.9;
