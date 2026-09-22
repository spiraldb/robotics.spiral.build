//! The mounted Live Timeline: one subject, assembled once per render and read
//! by the Timeline panel, the scene, and every camera.

use std::sync::Arc;

use dioxus::prelude::*;

use super::recording_review::EpisodePreview;
use super::session::LiveSession;
use super::signals::{RecordingTimeline, SignalStore};
use super::timeline::{LiveTimelinePanel, SignalSource, TimelineSignals, TimelineSubject};
use super::transport::{LiveClock, LivePreview, LiveTransport};
use crate::components::time_series::TimelineSignalLane;
use crate::panels::camera::{CameraEditorSource, RecordedCameraSourceRequest};
use crate::panels::camera_panel::AssignedCamera;
use crate::panels::recorded_source::RecordedSource;
use crate::panels::scene::SceneSource;

pub(super) struct LiveTimeline {
    source: Option<RecordedSource<EpisodePreview>>,
    transport: LiveTransport,
    signals: TimelineSignals,
}

impl LiveTimeline {
    pub(super) fn use_timeline(session: LiveSession) -> Self {
        let controller = session.timeline_controller();
        let preview = LivePreview::use_preview(session);
        let subject = session
            .telemetry()
            .recording()
            .map(TimelineSubject::Recording)
            .unwrap_or_else(|| {
                TimelineSubject::empty("Record an episode to begin using the shared clock.")
            });
        preview.use_source_identity(&subject);
        let source = preview.source(&subject);
        let signals = TimelineSignals::new(
            Self::signal_lanes(session, &subject, source.as_ref()),
            Self::signal_source(&subject, source.as_ref()),
        );
        let clock = LiveClock::new(controller.playhead_signal()(), controller.previewing())
            .with_follow_live(controller.follow_live_signal()());
        let transport = LiveTransport::new(preview, subject, clock);
        Self {
            source,
            transport,
            signals,
        }
    }

    /// The traces the lanes draw: the recorded episode's once they arrive, an
    /// empty set while a recorded episode is being read, and live telemetry
    /// when there is no recorded episode in view at all.
    fn signal_lanes(
        session: LiveSession,
        subject: &TimelineSubject,
        source: Option<&RecordedSource<EpisodePreview>>,
    ) -> Arc<[TimelineSignalLane; 6]> {
        if let Some(preview) = source.and_then(RecordedSource::renderable) {
            return Arc::clone(preview.signal_lanes());
        }
        if subject.recorded_episode().is_some() {
            return Arc::new(SignalStore::default().timeline_lanes());
        }
        let live = session.recording_signals();
        let store = live.read();
        Arc::new(store.timeline_lanes())
    }

    fn signal_source(
        subject: &TimelineSubject,
        source: Option<&RecordedSource<EpisodePreview>>,
    ) -> SignalSource {
        let Some(episode) = subject.recorded_episode() else {
            return SignalSource::Live;
        };
        match source.and_then(|source| source.state().reason()) {
            Some(reason) => SignalSource::RecordedUnavailable {
                episode: episode.to_owned(),
                reason: reason.to_owned(),
            },
            None if source.and_then(RecordedSource::renderable).is_some() => {
                SignalSource::RecordedReady
            }
            None => SignalSource::RecordedLoading,
        }
    }

    pub(super) fn scene_source(&self) -> SceneSource {
        match self.transport.subject().recorded_episode() {
            Some(episode) => SceneSource::recorded(episode, self.source.as_ref()),
            None => SceneSource::Live,
        }
    }

    pub(super) fn camera_source(&self, camera: &AssignedCamera) -> CameraEditorSource {
        match self.transport.subject().recorded_episode() {
            Some(episode) => camera.recorded_source(RecordedCameraSourceRequest::new(
                episode,
                self.source.as_ref(),
                self.transport.seconds(),
            )),
            None => camera.live_source(),
        }
    }

    pub(super) fn render(&self) -> Element {
        rsx! { LiveTimelinePanel { transport: self.transport.clone(), signals: self.signals.clone() } }
    }

    /// The transport the workspace binds its recording shortcuts to.
    pub(super) fn transport(&self) -> LiveTransport {
        self.transport.clone()
    }
}
