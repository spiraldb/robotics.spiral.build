//! Durable state and action dispatch for the Timeline panel.

use dioxus::prelude::*;

use super::subject::TimelineTarget;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum PreviewTransport {
    #[default]
    Stopped,
    Playing {
        episode: String,
        duration_seconds: f64,
    },
}

impl PreviewTransport {
    pub(crate) fn is_playing(&self) -> bool {
        matches!(self, Self::Playing { .. })
    }

    pub(crate) fn target(&self) -> Option<(String, f64)> {
        match self {
            Self::Stopped => None,
            Self::Playing {
                episode,
                duration_seconds,
            } => Some((episode.clone(), *duration_seconds)),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TimelineController {
    playhead: Signal<f64>,
    preview: Signal<PreviewTransport>,
    follow_live: Signal<bool>,
    robot_sync_target: Signal<Option<TimelineTarget>>,
    preview_tick_seconds: Signal<Option<f64>>,
}

impl TimelineController {
    pub(crate) fn use_controller() -> Self {
        crate::shell::view_state::ViewState::use_retained(|| Self {
            playhead: Signal::new(0.0),
            preview: Signal::new(PreviewTransport::default()),
            follow_live: Signal::new(true),
            robot_sync_target: Signal::new(None),
            preview_tick_seconds: Signal::new(None),
        })
    }

    pub(crate) const fn playhead_signal(self) -> Signal<f64> {
        self.playhead
    }

    pub(crate) const fn follow_live_signal(self) -> Signal<bool> {
        self.follow_live
    }

    pub(crate) const fn robot_sync_target_signal(self) -> Signal<Option<TimelineTarget>> {
        self.robot_sync_target
    }

    pub(crate) const fn preview_tick_signal(self) -> Signal<Option<f64>> {
        self.preview_tick_seconds
    }

    pub(crate) fn start_preview(mut self, episode: String, duration_seconds: f64) {
        self.preview.set(PreviewTransport::Playing {
            episode,
            duration_seconds,
        });
    }

    pub(crate) fn stop_preview(mut self) {
        self.preview.set(PreviewTransport::Stopped);
        self.preview_tick_seconds.set(None);
    }

    pub(crate) fn preview_playing(&self) -> bool {
        self.preview.read().is_playing()
    }

    pub(crate) fn preview_target(&self) -> Option<(String, f64)> {
        self.preview.read().target()
    }
}

#[cfg(test)]
mod tests {
    use super::PreviewTransport;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_preview_cannot_be_playing_without_a_target() {
        assert!(!PreviewTransport::Stopped.is_playing());
        assert_eq!(PreviewTransport::Stopped.target(), None);

        let playing = PreviewTransport::Playing {
            episode: "take-1".to_owned(),
            duration_seconds: 4.2,
        };
        assert!(playing.is_playing());
        assert_eq!(playing.target(), Some(("take-1".to_owned(), 4.2)));
    }
}
