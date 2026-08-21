//! Turning pointer motion into camera motion.
//!
//! The application adapts its own event type into the calls below; none of the
//! arithmetic that decides how far a drag turns the bench lives in a UI
//! handler any more. That arithmetic is the same whatever draws the canvas,
//! and it is the part that is worth getting right once.

use crate::camera::Camera;

/// Radians of orbit per pixel dragged.
const ORBIT_PER_PIXEL: f32 = 0.007;

/// Pixels a pointer may wander before a click becomes a drag.
///
/// Without it, the tremor of pressing a mouse button turns every click on the
/// bench into a one-pixel orbit and nothing is ever selected.
const CLICK_SLOP: f64 = 3.0;

/// The state of a drag in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    /// Pointer position at the beginning, to separate clicking from orbiting.
    start: (f64, f64),
    /// Pointer position when this drag was last sampled, in client pixels.
    last: (f64, f64),
    panning: bool,
    moved: bool,
}

impl Drag {
    /// Begin a drag. `panning` slides the target instead of turning about it.
    pub fn new(at: (f64, f64), panning: bool) -> Self {
        Self {
            start: at,
            last: at,
            panning,
            moved: false,
        }
    }

    /// True once the pointer has travelled far enough to mean it.
    pub fn moved(&self) -> bool {
        self.moved
    }

    /// Apply one pointer move to `camera`.
    ///
    /// Both gestures are direct manipulation: the drag moves the scene, not the
    /// camera. Dragging down tips the arm's top toward the viewer — the same
    /// way dragging down a globe brings its north pole around — which puts the
    /// camera up.
    pub fn advance(&mut self, to: (f64, f64), camera: &mut Camera, viewport_height: f32) {
        let (dx, dy) = (to.0 - self.last.0, to.1 - self.last.1);
        self.moved |= (to.0 - self.start.0).hypot(to.1 - self.start.1) > CLICK_SLOP;
        self.last = to;
        if self.panning {
            // A pan keeps the point under the pointer under the pointer, so it
            // moves in world metres per pixel.
            let metres = camera.metres_per_pixel(viewport_height);
            camera.pan(-dx as f32 * metres, dy as f32 * metres);
        } else {
            camera.orbit(-dx as f32 * ORBIT_PER_PIXEL, dy as f32 * ORBIT_PER_PIXEL);
        }
    }
}

/// A gesture the application asks the camera to perform.
///
/// Named rather than expressed as raw numbers so a keyboard step and a wheel
/// notch cannot drift apart from the drag they are meant to match.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    OrbitLeft,
    OrbitRight,
    OrbitUp,
    OrbitDown,
    ZoomIn,
    ZoomOut,
    /// Mouse notches or normalized trackpad pixels.
    Wheel(f64),
}

impl Gesture {
    /// Radians one key press turns the bench.
    const KEY_ORBIT: f32 = 0.08;

    pub fn apply(self, camera: &mut Camera) {
        match self {
            Self::OrbitLeft => camera.orbit(Self::KEY_ORBIT, 0.0),
            Self::OrbitRight => camera.orbit(-Self::KEY_ORBIT, 0.0),
            Self::OrbitUp => camera.orbit(0.0, -Self::KEY_ORBIT),
            Self::OrbitDown => camera.orbit(0.0, Self::KEY_ORBIT),
            Self::ZoomIn => camera.zoom(0.88),
            Self::ZoomOut => camera.zoom(1.14),
            // Mice report notches and trackpads report pixels; both arrive
            // normalized so one notch zooms by about a sixth, which is small
            // enough that a trackpad flick does not overshoot.
            Self::Wheel(notches) => camera.zoom((notches.clamp(-4.0, 4.0) as f32 * 0.15).exp()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_that_wanders_a_pixel_is_still_a_click() {
        let mut camera = Camera::default();
        let mut drag = Drag::new((100.0, 100.0), false);

        drag.advance((101.0, 100.5), &mut camera, 600.0);
        assert!(!drag.moved(), "a hand tremor must not eat the selection");

        drag.advance((140.0, 100.0), &mut camera, 600.0);
        assert!(drag.moved());
    }

    #[test]
    fn panning_holds_the_bench_under_the_pointer_at_any_zoom() {
        // A pan is measured in world metres per pixel, so the same drag has to
        // move the target further when the camera is further away.
        let mut near = Camera {
            distance: 0.3,
            ..Camera::default()
        };
        let mut far = Camera {
            distance: 2.0,
            ..Camera::default()
        };
        let origin = (near.target, far.target);

        Drag::new((0.0, 0.0), true).advance((60.0, 0.0), &mut near, 600.0);
        Drag::new((0.0, 0.0), true).advance((60.0, 0.0), &mut far, 600.0);

        let moved_near = (near.target - origin.0).length();
        let moved_far = (far.target - origin.1).length();
        assert!(moved_far > moved_near * 5.0, "{moved_far} vs {moved_near}");
    }
}
