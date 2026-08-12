//! Durable state and action dispatch for the Timeline panel.

use dioxus::prelude::*;

use crate::frontend::api::commands;
use crate::frontend::api::historical::{
    HistoricalPreview, historical_replay_slot, request_historical_preview,
};
use crate::frontend::app::activity::{Activity, RecordedSource, TimelineSubject, TimelineTarget};
use crate::frontend::app::state::Studio;
use crate::frontend::panels::timeline::panel::{
    TimelineAction, TimelineCaptureAction, TimelinePreviewAction, TimelineRobotSyncAction,
};

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
    pub playhead: Signal<f64>,
    pub preview: Signal<PreviewTransport>,
    pub follow_live: Signal<bool>,
    pub robot_sync_target: Signal<Option<TimelineTarget>>,
    pub live_review_episode: Signal<Option<String>>,
    #[cfg(feature = "web")]
    pub preview_tick_seconds: Signal<Option<f64>>,
}

pub(crate) fn use_timeline_controller() -> TimelineController {
    let controller = TimelineController {
        playhead: use_signal(|| 0.0),
        preview: use_signal(PreviewTransport::default),
        follow_live: use_signal(|| true),
        robot_sync_target: use_signal(|| None),
        live_review_episode: use_signal(|| None),
        #[cfg(feature = "web")]
        preview_tick_seconds: use_signal(|| None),
    };
    use_context_provider(|| controller)
}

pub(crate) fn timeline_controller() -> TimelineController {
    use_context::<TimelineController>()
}

impl TimelineController {
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
    pub studio: Studio,
    pub controller: TimelineController,
    pub subject: TimelineSubject,
    pub review_episode: Option<String>,
    pub seek_context: Option<(String, String, Option<String>)>,
    pub historical_state: Signal<Option<RecordedSource<HistoricalPreview>>>,
    pub historical_revision: Signal<u64>,
    pub automatic_preview_target: Signal<Option<(String, u64)>>,
    pub interactive_preview_target: Signal<Option<(String, u64)>>,
}

/// Apply a typed Timeline action after validating that it still names the
/// controller's current subject. The controlled Timeline view never mutates
/// transport or calls the backend directly.
pub(crate) fn dispatch_timeline_action(context: TimelineActionContext, action: TimelineAction) {
    let TimelineActionContext {
        studio,
        controller,
        subject,
        review_episode,
        seek_context,
        mut historical_state,
        mut historical_revision,
        mut automatic_preview_target,
        mut interactive_preview_target,
    } = context;
    let mut playhead = controller.playhead;
    let mut follow_live = controller.follow_live;
    let mut robot_sync_target = controller.robot_sync_target;
    let mut live_review_episode = controller.live_review_episode;

    match action {
        TimelineAction::Capture { episode, action } => match action {
            TimelineCaptureAction::Start if subject.activity() == Activity::Live => {
                commands::start_recording(studio);
            }
            TimelineCaptureAction::Pause | TimelineCaptureAction::Resume => {
                if let Some(episode) = episode {
                    let paused = action == TimelineCaptureAction::Pause;
                    commands::set_recording_paused(studio, &episode, paused);
                    follow_live.set(!paused);
                    if !paused {
                        historical_state.set(None);
                    }
                }
            }
            TimelineCaptureAction::Stop | TimelineCaptureAction::Discard => {
                if let Some(episode) = episode {
                    let keep = action == TimelineCaptureAction::Stop;
                    commands::stop_recording(
                        studio,
                        &episode,
                        keep,
                        EventHandler::new(
                            move |summary: spiral_robotics_core::api::EpisodeSummary| {
                                let next_revision = historical_revision.peek().wrapping_add(1);
                                historical_revision.set(next_revision);
                                historical_state.set(None);
                                automatic_preview_target.set(None);
                                interactive_preview_target.set(None);
                                if keep {
                                    let episode = summary.id;
                                    live_review_episode.set(Some(episode.clone()));
                                    request_historical_preview(
                                        studio,
                                        episode,
                                        0.0,
                                        historical_state,
                                        historical_revision,
                                    );
                                } else {
                                    live_review_episode.set(None);
                                }
                            },
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
                        commands::start_replay(studio, summary, playhead());
                    }
                }
                TimelineRobotSyncAction::Pause => {
                    commands::set_replay_paused(studio, &episode, true)
                }
                TimelineRobotSyncAction::Resume => {
                    commands::set_replay_paused(studio, &episode, false)
                }
            }
        }
        TimelineAction::Seek(request) if subject.target().as_ref() == Some(&request.target) => {
            controller.stop_preview();
            playhead.set(request.seconds);
            if let Some(episode) = review_episode.clone() {
                let target = (episode.clone(), historical_replay_slot(request.seconds));
                if request.committed || interactive_preview_target.peek().as_ref() != Some(&target)
                {
                    interactive_preview_target.set(Some(target));
                    request_historical_preview(
                        studio,
                        episode,
                        request.seconds,
                        historical_state,
                        historical_revision,
                    );
                }
            }
            if request.committed
                && request.move_robot
                && let Some((episode, robot, source_arm)) = seek_context
            {
                commands::physical_seek(studio, episode, robot, source_arm, request.seconds);
            }
        }
        TimelineAction::SetFollowLive(next) => {
            follow_live.set(next);
            if next {
                controller.stop_preview();
                if let Some(recording) = studio.recording() {
                    playhead.set(recording.elapsed_seconds);
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
                    && subject.episode_id() == Some(replay.episode.as_str())
                {
                    commands::stop_replay(studio, &replay.episode);
                }
            }
        }
        TimelineAction::ReloadRecorded { episode }
            if review_episode.as_deref() == Some(episode.as_str()) =>
        {
            historical_state.set(None);
            request_historical_preview(
                studio,
                episode,
                playhead(),
                historical_state,
                historical_revision,
            );
        }
        _ => studio.report("The Timeline subject changed before the action was applied."),
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
