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
pub(crate) struct Camera {
    /// Rotation about the vertical axis, radians.
    yaw: f32,
    /// Elevation above the horizon, radians.
    pitch: f32,
    /// Distance from the target, metres.
    distance: f32,
    target: Vec3,
    scale: f32,
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
            scale: 1.0,
        }
    }
}

/// How far the camera may tip before looking straight down the world's up axis,
/// where the view direction and the up vector stop defining a rotation.
const PITCH_LIMIT: f32 = 1.45;
const MIN_DISTANCE: f32 = 0.18;
const MAX_DISTANCE: f32 = 3.0;

impl Camera {
    pub(crate) fn framed_on(target: Vec3, distance: f32) -> Self {
        Self {
            target,
            distance,
            scale: distance / 0.78,
            ..Self::default()
        }
    }
    /// Return this camera with a different viewing direction.
    pub(crate) fn oriented(mut self, yaw: f32, pitch: f32) -> Self {
        self.yaw = yaw;
        self.pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self
    }

    /// Return this camera framed on `target` from `distance` metres away.
    #[cfg(test)]
    pub(crate) fn focused_on(mut self, target: Vec3, distance: f32) -> Self {
        self.target = target;
        self.distance = distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
        self
    }

    /// Turn the camera about its target, in radians.
    pub(crate) fn orbit(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.yaw += yaw_delta;
        self.pitch = (self.pitch + pitch_delta).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Move closer or further away. `factor` above one pulls back.
    pub(crate) fn zoom(&mut self, factor: f32) {
        self.distance =
            (self.distance * factor).clamp(MIN_DISTANCE * self.scale, MAX_DISTANCE * self.scale);
    }

    /// Slide the target across the screen, in metres of world space.
    pub(crate) fn pan(&mut self, right_metres: f32, up_metres: f32) {
        let (right_axis, up_axis) = self.screen_axes();
        self.target += right_axis * right_metres + up_axis * up_metres;
    }

    /// Where the camera sits.
    fn eye(&self) -> Vec3 {
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        self.target + Vec3::new(cos_pitch * cos_yaw, cos_pitch * sin_yaw, sin_pitch) * self.distance
    }

    #[cfg(test)]
    pub(crate) fn target(&self) -> Vec3 {
        self.target
    }

    /// The world directions that screen right and screen up correspond to.
    fn screen_axes(&self) -> (Vec3, Vec3) {
        let forward_axis = (self.target - self.eye()).normalize_or(Vec3::X);
        let right_axis = forward_axis.cross(Vec3::Z).normalize_or(Vec3::Y);
        (right_axis, right_axis.cross(forward_axis))
    }

    /// How many metres one screen pixel covers at the target, so panning and
    /// the model keep pace no matter how far out the camera has been pulled.
    pub(crate) fn metres_per_pixel(&self, viewport_height: f32) -> f32 {
        if viewport_height <= 0.0 {
            return 0.0;
        }
        2.0 * self.distance * (FIELD_OF_VIEW / 2.0).tan() / viewport_height
    }

    pub(crate) fn view_projection(&self, aspect_ratio: f32) -> Mat4 {
        // Right handed, with the depth range OpenGL and WebGL expect. Near and
        // far bracket a tabletop scene: close enough to push the camera almost
        // into a joint, far enough to pull back off the bench.
        let projection = opengl::perspective(
            FIELD_OF_VIEW,
            aspect_ratio.max(0.01),
            0.02 * self.scale,
            20.0 * self.scale,
        );
        projection * look_at_mat4(self.eye(), self.target, Vec3::Z)
    }
}

const FIELD_OF_VIEW: f32 = 0.9;
