//! The mounted Live Timeline: one subject, assembled once per render and read
//! by the Timeline panel, the scene, and every camera.

use std::sync::Arc;

use dioxus::prelude::*;

use super::recording_review::EpisodePreview;
use super::session::LiveSession;
use super::signals::{RecordingTimeline, SignalStore};
use super::timeline::{
    LiveTimelinePanel, SignalSource, TimelineSignals, TimelineSubject, TimelineTarget,
};
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
        let mut robot_sync_target = controller.robot_sync_target_signal();
        let preview = LivePreview::use_preview(session);
        let subject = session
            .telemetry()
            .recording()
            .map(TimelineSubject::Recording)
            .unwrap_or_else(|| {
                TimelineSubject::empty("Live", "Record an episode to begin using the shared clock.")
            });
        preview.use_source_identity(&subject);
        let source = preview.source(&subject);
        let signals = TimelineSignals::new(
            Self::signal_lanes(session, &subject, source.as_ref()),
            Self::signal_source(&subject, source.as_ref()),
        );
        let replay = session
            .telemetry()
            .replay()
            .filter(|replay| subject.episode_id() == Some(replay.episode()));
        let target = subject.target();
        // Two `None`s compare equal, so with no episode on the clock this read as
        // "armed" and the physical-motion toggle rendered checked at rest. Nothing
        // is armed until there is a target to arm it against.
        let armed = target.is_some() && robot_sync_target.read().as_ref() == target.as_ref();
        let clock = LiveClock::new(
            controller.playhead_signal()(),
            controller.phase(replay.as_ref()),
        )
        .with_options(controller.follow_live_signal()(), armed);
        let transport = LiveTransport::new(preview, subject, clock);
        let must_disarm = armed && transport.robot_sync_blocker().is_some();
        use_effect(use_reactive(
            (&target, &must_disarm),
            move |(current_target, must_disarm)| {
                let armed = robot_sync_target.peek().to_owned();
                let target_changed = armed.is_some() && armed.as_ref() != current_target.as_ref();
                if !target_changed && !must_disarm {
                    return;
                }
                robot_sync_target.set(None);
                let armed_episode = armed.as_ref().map(TimelineTarget::episode);
                if let Some(replay) = session.telemetry().replay()
                    && armed_episode == Some(replay.episode())
                {
                    session.stop_replay(replay.episode());
                }
            },
        ));
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
