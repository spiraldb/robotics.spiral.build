//! Durable state and action dispatch for the Timeline panel.

use dioxus::prelude::*;

use crate::client::commands::{RecordingCompletion, TimelineSeekBody};
use crate::client::historical::{
    HistoricalPreviewRequest, HistoricalPreviewSignals, historical_replay_slot,
    request_historical_preview,
};
use crate::panels::timeline::panel::{
    TimelineAction, TimelineCaptureAction, TimelinePreviewAction, TimelineRobotSyncAction,
};
use crate::shell::activity::{Activity, TimelineSubject, TimelineTarget};
use crate::shell::state::Studio;

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

    #[cfg(any(test, feature = "web"))]
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
    #[cfg(feature = "web")]
    preview_tick_seconds: Signal<Option<f64>>,
}

pub(crate) fn use_timeline_controller() -> TimelineController {
    let controller = TimelineController {
        playhead: use_signal(|| 0.0),
        preview: use_signal(PreviewTransport::default),
        follow_live: use_signal(|| true),
        robot_sync_target: use_signal(|| None),
        #[cfg(feature = "web")]
        preview_tick_seconds: use_signal(|| None),
    };
    use_context_provider(|| controller)
}

pub(crate) fn timeline_controller() -> TimelineController {
    use_context::<TimelineController>()
}

impl TimelineController {
    pub(crate) const fn playhead_signal(self) -> Signal<f64> {
        self.playhead
    }

    pub(crate) const fn follow_live_signal(self) -> Signal<bool> {
        self.follow_live
    }

    pub(crate) const fn robot_sync_target_signal(self) -> Signal<Option<TimelineTarget>> {
        self.robot_sync_target
    }

    #[cfg(feature = "web")]
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
        #[cfg(feature = "web")]
        self.preview_tick_seconds.set(None);
    }

    pub(crate) fn preview_playing(&self) -> bool {
        self.preview.read().is_playing()
    }

    #[cfg(feature = "web")]
    pub(crate) fn preview_target(&self) -> Option<(String, f64)> {
        self.preview.read().target()
    }
}

pub(crate) struct TimelineActionContext {
    session: TimelineActionSession,
    recorded: RecordedTimelineActionState,
    preview_targets: TimelinePreviewTargets,
}

impl TimelineActionContext {
    pub(crate) const fn new(
        session: TimelineActionSession,
        recorded: RecordedTimelineActionState,
        preview_targets: TimelinePreviewTargets,
    ) -> Self {
        Self {
            session,
            recorded,
            preview_targets,
        }
    }
}

pub(crate) struct TimelineActionSession {
    studio: Studio,
    controller: TimelineController,
    subject: TimelineSubject,
}

impl TimelineActionSession {
    pub(crate) const fn new(
        studio: Studio,
        controller: TimelineController,
        subject: TimelineSubject,
    ) -> Self {
        Self {
            studio,
            controller,
            subject,
        }
    }
}

pub(crate) struct RecordedTimelineActionState {
    review_episode: Option<String>,
    seek_context: Option<(String, String, Option<String>)>,
    historical: HistoricalPreviewSignals,
}

impl RecordedTimelineActionState {
    pub(crate) const fn new(
        review_episode: Option<String>,
        seek_context: Option<(String, String, Option<String>)>,
        historical: HistoricalPreviewSignals,
    ) -> Self {
        Self {
            review_episode,
            seek_context,
            historical,
        }
    }
}

pub(crate) struct TimelinePreviewTargets {
    automatic: Signal<Option<(String, u64)>>,
    interactive: Signal<Option<(String, u64)>>,
}

impl TimelinePreviewTargets {
    pub(crate) const fn new(
        automatic: Signal<Option<(String, u64)>>,
        interactive: Signal<Option<(String, u64)>>,
    ) -> Self {
        Self {
            automatic,
            interactive,
        }
    }
}

/// Apply a typed Timeline action after validating that it still names the
/// controller's current subject. The controlled Timeline view never mutates
/// transport or calls the backend directly.
impl TimelineActionContext {
    pub(crate) fn dispatch(self, action: TimelineAction) {
        let TimelineActionContext {
            session:
                TimelineActionSession {
                    studio,
                    controller,
                    subject,
                },
            recorded:
                RecordedTimelineActionState {
                    review_episode,
                    seek_context,
                    historical,
                },
            preview_targets:
                TimelinePreviewTargets {
                    automatic: mut automatic_preview_target,
                    interactive: mut interactive_preview_target,
                },
        } = self;
        let (mut historical_state, mut historical_revision, mut historical_cache) =
            historical.into_signals();
        let mut playhead = controller.playhead;
        let mut follow_live = controller.follow_live;
        let mut robot_sync_target = controller.robot_sync_target;

        match action {
            TimelineAction::Capture { episode, action } => match action {
                TimelineCaptureAction::Start if subject.activity() == Activity::Live => {
                    studio.start_recording();
                }
                TimelineCaptureAction::Pause | TimelineCaptureAction::Resume => {
                    if let Some(episode) = episode {
                        let paused = action == TimelineCaptureAction::Pause;
                        studio.set_recording_paused(&episode, paused);
                        follow_live.set(!paused);
                        if !paused {
                            historical_cache.write().invalidate(&episode);
                            historical_state.set(None);
                        }
                    }
                }
                TimelineCaptureAction::Stop | TimelineCaptureAction::Discard => {
                    if let Some(episode) = episode {
                        let keep = action == TimelineCaptureAction::Stop;
                        let finished_episode = episode.clone();
                        studio.stop_recording(
                            &episode,
                            RecordingCompletion::new(
                                keep,
                                EventHandler::new(
                                    move |_: spiral_robotics_api::recording::EpisodeSummary| {
                                        historical_cache.write().invalidate(&finished_episode);
                                        let next_revision =
                                            historical_revision.peek().wrapping_add(1);
                                        historical_revision.set(next_revision);
                                        historical_state.set(None);
                                        automatic_preview_target.set(None);
                                        interactive_preview_target.set(None);
                                    },
                                ),
                            ),
                        );
                    }
                }
                TimelineCaptureAction::Start => {
                    studio.report("Recording is only available in Live.");
                }
            },
            TimelineAction::Preview { episode, action }
                if subject.episode_id() == Some(episode.as_str()) =>
            {
                match action {
                    TimelinePreviewAction::Play => {
                        controller.start_preview(episode, subject.duration_seconds());
                        follow_live.set(false);
                    }
                    TimelinePreviewAction::Pause => controller.stop_preview(),
                }
            }
            TimelineAction::RobotSyncTransport { episode, action }
                if subject.episode_id() == Some(episode.as_str()) =>
            {
                match action {
                    TimelineRobotSyncAction::Start => {
                        controller.stop_preview();
                        follow_live.set(false);
                        if let Some(summary) = subject.episode() {
                            studio.start_replay(summary, playhead());
                        }
                    }
                    TimelineRobotSyncAction::Pause => studio.set_replay_paused(&episode, true),
                    TimelineRobotSyncAction::Resume => studio.set_replay_paused(&episode, false),
                }
            }
            TimelineAction::Seek(request)
                if subject.target().as_ref() == Some(request.target()) =>
            {
                controller.stop_preview();
                playhead.set(request.seconds());
                if let Some(episode) = review_episode.clone() {
                    let target = (episode.clone(), historical_replay_slot(request.seconds()));
                    if request.committed()
                        || interactive_preview_target.peek().as_ref() != Some(&target)
                    {
                        interactive_preview_target.set(Some(target));
                        request_historical_preview(
                            studio,
                            HistoricalPreviewRequest::new(episode, None, request.seconds()),
                            HistoricalPreviewSignals::new(
                                historical_state,
                                historical_revision,
                                historical_cache,
                            ),
                        );
                    }
                }
                if request.committed()
                    && request.moves_robot()
                    && let Some((episode, robot, source_arm)) = seek_context
                {
                    studio.physical_seek(
                        episode,
                        TimelineSeekBody::new(robot, source_arm, request.seconds()),
                    );
                }
            }
            TimelineAction::SetFollowLive(next) => {
                follow_live.set(next);
                if next {
                    controller.stop_preview();
                    if let Some(recording) = studio.recording() {
                        playhead.set(recording.elapsed_seconds());
                    }
                }
            }
            TimelineAction::SetRobotSync { target, enabled }
                if subject.target().as_ref() == Some(&target) =>
            {
                if enabled {
                    controller.stop_preview();
                    robot_sync_target.set(Some(target));
                } else {
                    robot_sync_target.set(None);
                    if let Some(replay) = studio.replay()
                        && subject.episode_id() == Some(replay.episode())
                    {
                        studio.stop_replay(replay.episode());
                    }
                }
            }
            TimelineAction::ReloadRecorded { episode }
                if review_episode.as_deref() == Some(episode.as_str()) =>
            {
                historical_state.set(None);
                historical_cache.write().invalidate(&episode);
                request_historical_preview(
                    studio,
                    HistoricalPreviewRequest::new(episode, None, playhead()),
                    HistoricalPreviewSignals::new(
                        historical_state,
                        historical_revision,
                        historical_cache,
                    ),
                );
            }
            _ => studio.report("The Timeline subject changed before the action was applied."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PreviewTransport;

    #[test]
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
