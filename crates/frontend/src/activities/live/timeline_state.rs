//! Durable state and action dispatch for the Timeline panel.

use dioxus::prelude::*;

use super::timeline_subject::TimelineTarget;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct TimelineController {
    playhead: Signal<f64>,
    preview: Signal<Option<(String, f64)>>,
    follow_live: Signal<bool>,
    robot_sync_target: Signal<Option<TimelineTarget>>,
}

impl TimelineController {
    pub(super) fn use_controller() -> Self {
        use_hook(|| Self {
            playhead: Signal::new(0.0),
            preview: Signal::new(None),
            follow_live: Signal::new(true),
            robot_sync_target: Signal::new(None),
        })
    }

    pub(super) const fn playhead_signal(self) -> Signal<f64> {
        self.playhead
    }

    pub(super) const fn follow_live_signal(self) -> Signal<bool> {
        self.follow_live
    }

    pub(super) const fn robot_sync_target_signal(self) -> Signal<Option<TimelineTarget>> {
        self.robot_sync_target
    }

    pub(super) fn start_preview(mut self, episode: String, duration_seconds: f64) {
        if (self.playhead)() >= duration_seconds {
            self.playhead.set(0.0);
        }
        self.preview.set(Some((episode, duration_seconds)));
    }

    pub(super) fn stop_preview(mut self) {
        self.preview.set(None);
    }

    pub(super) fn preview_playing(&self) -> bool {
        self.preview.read().is_some()
    }

    pub(super) fn preview_target(&self) -> Option<(String, f64)> {
        self.preview.read().clone()
    }
}
