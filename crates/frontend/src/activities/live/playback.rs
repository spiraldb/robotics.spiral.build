//! Live's recording policies and cross-panel playback synchronization.

use super::timeline_actions::LiveTimelineActions;
use crate::activities::data::episode_cache::{
    EpisodePlaybackTime, EpisodePreview, EpisodePreviewRequest, EpisodePreviewSignals,
};
use crate::panels::recorded_source::RecordedSource;
use crate::panels::timeline::panel::{
    RecordedTimelineTraces, RecordingTimeline, SignalSource, StudioTimelineState, TimelinePlayback,
    TimelineVisualization,
};
use crate::panels::timeline::playback::TimelineReplay;
use crate::panels::timeline::subject::{TimelineSubject, TimelineTarget};
use crate::shell::state::Studio;
use dioxus::prelude::*;
use std::sync::Arc;

#[derive(Clone, Copy)]
struct LivePreview {
    studio: Studio,
    preview: EpisodePreviewSignals,
    sampled: Signal<Option<(String, u64)>>,
    interactive: Signal<Option<(String, u64)>>,
}

impl LivePreview {
    fn use_preview(studio: Studio) -> Self {
        let preview = EpisodePreviewSignals::use_signals();
        // Throttling is local to the mounted screen; remounting must sample again.
        let session = Self {
            studio,
            preview,
            sampled: use_signal(|| None),
            interactive: use_signal(|| None),
        };
        let on_sample = use_callback(move |(episode, seconds)| session.sample(episode, seconds));
        studio.timeline_controller().use_clock(on_sample);
        session.use_recording_clock();
        session
    }

    fn sample(mut self, episode: String, seconds: f64) {
        let Some(recording) = self
            .studio
            .recording()
            .filter(|recording| recording.paused() && recording.episode() == episode)
        else {
            return;
        };
        let target = (episode.clone(), seconds.replay_slot());
        if self.sampled.peek().as_ref() == Some(&target) {
            return;
        }
        self.sampled.set(Some(target));
        self.studio.request_episode_preview(
            EpisodePreviewRequest::new(episode, Some(recording.frames()), seconds),
            self.preview,
        );
    }

    fn use_recording_clock(self) {
        let studio = self.studio;
        let timeline = studio.timeline_controller();
        let mut playhead = timeline.playhead_signal();
        let follow_live = timeline.follow_live_signal();
        use_effect(move || {
            let recording = studio.recording();
            if let Some(recording) = recording.as_ref().filter(|recording| !recording.paused()) {
                if follow_live() {
                    playhead.set(recording.elapsed_seconds());
                }
                timeline.stop_preview();
                return;
            }
            if recording.is_none() {
                timeline.stop_preview();
                return;
            }
            if let Some(replay) = studio.replay().filter(|replay| {
                replay.owns_timeline_transport()
                    && replay.matches_timeline_subject(
                        recording.as_ref().map(|recording| recording.episode()),
                    )
            }) {
                playhead.set(replay.elapsed_seconds());
                timeline.stop_preview();
                self.sample(replay.episode().to_owned(), replay.elapsed_seconds());
            }
        });
    }

    fn use_source_identity(mut self, subject: &TimelineSubject) {
        let identity = subject
            .recording()
            .filter(|recording| recording.paused())
            .map(|recording| {
                (
                    recording.episode().to_owned(),
                    recording.frames(),
                    recording.elapsed_seconds(),
                )
            });
        use_effect(use_reactive((&identity,), move |(identity,)| {
            let (mut source, mut revision, _) = self.preview.into_signals();
            // Finalization, resume, and source revision changes invalidate in-flight reads.
            let next = revision.peek().wrapping_add(1);
            revision.set(next);
            source.set(None);
            self.sampled.set(None);
            self.interactive.set(None);
            if let Some((episode, frames, seconds)) = identity {
                let mut playhead = self.studio.timeline_controller().playhead_signal();
                playhead.set(seconds);
                self.studio.request_episode_preview(
                    EpisodePreviewRequest::new(episode, Some(frames), seconds),
                    self.preview,
                );
            }
        }));
    }

    fn source(self, subject: &TimelineSubject) -> Option<RecordedSource<EpisodePreview>> {
        self.preview
            .into_signals()
            .0
            .read()
            .clone()
            .filter(|source| Some(source.episode()) == subject.recorded_episode())
    }

    fn actions(self) -> LiveTimelineActions {
        LiveTimelineActions::new(self.studio, self.preview, self.interactive)
    }
}

pub(super) struct LiveTimeline {
    subject: TimelineSubject,
    source: Option<RecordedSource<EpisodePreview>>,
    playback: TimelinePlayback,
    state: StudioTimelineState,
    visualization: TimelineVisualization,
    actions: LiveTimelineActions,
}

impl LiveTimeline {
    pub(super) fn use_timeline(studio: Studio) -> Self {
        let timeline = studio.timeline_controller();
        let playhead = timeline.playhead_signal();
        let follow_live = timeline.follow_live_signal();
        let mut robot_sync_target = timeline.robot_sync_target_signal();
        let preview_session = LivePreview::use_preview(studio);
        let subject = studio
            .recording()
            .map(TimelineSubject::Recording)
            .unwrap_or_else(|| {
                TimelineSubject::empty("Live", "Record an episode to begin using the shared clock.")
            });
        preview_session.use_source_identity(&subject);
        let live_signal_lanes = Arc::new({
            let recording_signals = studio.recording_signals();
            let store = recording_signals.read();
            store.timeline_lanes()
        });
        let review_episode = subject.recorded_episode().map(str::to_owned);
        let episode_preview_source = preview_session.source(&subject);
        let episode_preview = episode_preview_source
            .as_ref()
            .and_then(RecordedSource::renderable)
            .cloned();
        let recorded_episode_active = review_episode.is_some();
        let signal_lanes = match episode_preview.as_ref() {
            Some(preview) => Arc::clone(preview.signal_lanes()),
            None if recorded_episode_active => {
                Arc::new(RecordedTimelineTraces::default().signal_lanes())
            }
            None => live_signal_lanes,
        };
        let signal_source = match review_episode.as_ref() {
            None => SignalSource::Live,
            Some(episode) => {
                let failure = episode_preview_source
                    .as_ref()
                    .and_then(|source| source.state().reason().map(str::to_owned));
                match failure {
                    Some(reason) => SignalSource::RecordedUnavailable {
                        episode: episode.clone(),
                        reason,
                    },
                    None if episode_preview.is_some() => SignalSource::RecordedReady,
                    None => SignalSource::RecordedLoading,
                }
            }
        };
        let timeline_target = subject.target();
        // Two `None`s compare equal, so with no episode on the clock this read as
        // "armed" and the physical-motion toggle rendered checked at rest. Nothing
        // is armed until there is a target to arm it against.
        let requested_robot_sync = timeline_target.is_some()
            && robot_sync_target.read().as_ref() == timeline_target.as_ref();
        let capabilities = studio.live_timeline_capabilities(&subject);
        let replay = studio
            .replay()
            .filter(|replay| subject.episode_id() == Some(replay.episode()));
        let playback = timeline.playback(replay.as_ref());
        let robot_sync = requested_robot_sync;
        let disarm_robot_sync = requested_robot_sync && !capabilities.robot_sync().is_allowed();
        use_effect(use_reactive(
            (&timeline_target, &disarm_robot_sync),
            move |(current_target, must_disarm)| {
                let armed = robot_sync_target.peek().to_owned();
                let target_changed = armed.is_some() && armed.as_ref() != current_target.as_ref();
                if !target_changed && !must_disarm {
                    return;
                }
                robot_sync_target.set(None);
                let armed_episode = armed.as_ref().map(TimelineTarget::episode);
                if let Some(replay) = studio.replay()
                    && armed_episode == Some(replay.episode())
                {
                    studio.stop_replay(replay.episode());
                }
            },
        ));
        let subject_for_state = subject.clone();
        Self {
            subject,
            source: episode_preview_source,
            playback,
            state: StudioTimelineState::new(subject_for_state, playhead(), playback)
                .with_controls(follow_live(), robot_sync)
                .with_label("Live Timeline"),
            visualization: TimelineVisualization::new(signal_lanes, signal_source, capabilities),
            actions: preview_session.actions(),
        }
    }

    pub(super) fn scene_source(&self) -> crate::panels::scene::SceneSource {
        match self.subject.recorded_episode() {
            Some(episode) => {
                crate::panels::scene::SceneSource::recorded(episode, self.source.as_ref())
            }
            None => crate::panels::scene::SceneSource::Live,
        }
    }

    pub(super) fn camera_source(
        &self,
        camera: &crate::panels::camera_panel::AssignedCamera,
    ) -> crate::panels::camera::CameraEditorSource {
        match self.subject.recorded_episode() {
            Some(episode) => {
                camera.recorded_source(crate::panels::camera::RecordedCameraSourceRequest::new(
                    episode,
                    self.source.as_ref(),
                    self.state.playhead_seconds(),
                ))
            }
            None => camera.live_source(),
        }
    }

    pub(super) fn media_playing(&self) -> bool {
        matches!(
            self.playback,
            TimelinePlayback::PreviewPlaying | TimelinePlayback::RobotSyncPlaying
        )
    }

    pub(super) fn render(&self) -> Element {
        let subject = self.subject.clone();
        let actions = self.actions;
        rsx! { crate::panels::timeline::panel::StudioTimeline {
            state: self.state.clone(), visualization: self.visualization.clone(),
            on_action: move |action| actions.dispatch(subject.clone(), action),
        } }
    }

    pub(super) fn actions(&self) -> LiveTimelineActions {
        self.actions
    }
}
