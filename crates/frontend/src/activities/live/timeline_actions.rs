//! Live's recording shortcuts, action dispatch, and robot-synchronization policies.

use super::recording_actions::{RecordingCompletion, TimelineSeekBody};
use crate::activities::data::episode_cache::{
    EpisodePlaybackTime, EpisodePreviewRequest, EpisodePreviewSignals,
};
use crate::activities::live::session::LiveSession;
use crate::panels::timeline::panel::{
    TimelineAction, TimelineCapabilities, TimelineCaptureAction, TimelineGate,
    TimelinePreviewAction, TimelineRobotSyncAction,
};
use crate::panels::timeline::subject::TimelineSubject;
use dioxus::prelude::*;
use spiral_robotics_common::robots::ModeSelection;

#[derive(Clone, Copy)]
pub(super) struct LiveTimelineActions {
    session: LiveSession,
    preview: EpisodePreviewSignals,
    interactive: Signal<Option<(String, u64)>>,
}

impl LiveTimelineActions {
    pub(super) fn new(
        session: LiveSession,
        preview: EpisodePreviewSignals,
        interactive: Signal<Option<(String, u64)>>,
    ) -> Self {
        Self {
            session,
            preview,
            interactive,
        }
    }

    pub(super) fn dispatch(self, subject: TimelineSubject, action: TimelineAction) {
        let controller = self.session.timeline_controller();
        let review_episode = subject.recorded_episode().map(str::to_owned);
        let seek_context = self.session.timeline_seek_context(&subject);
        let (mut episode_preview_state, mut episode_preview_revision, mut episode_data_cache) =
            self.preview.into_signals();
        let mut interactive_preview_target = self.interactive;
        let mut playhead = controller.playhead_signal();
        let mut follow_live = controller.follow_live_signal();
        let mut robot_sync_target = controller.robot_sync_target_signal();
        match action {
            TimelineAction::Capture { episode, action } => match action {
                TimelineCaptureAction::Start => {
                    self.session.start_recording();
                }
                TimelineCaptureAction::Pause | TimelineCaptureAction::Resume => {
                    if let Some(episode) = episode {
                        let paused = action == TimelineCaptureAction::Pause;
                        self.session.set_recording_paused(&episode, paused);
                        follow_live.set(!paused);
                        if !paused {
                            episode_data_cache.write().invalidate(&episode);
                            episode_preview_state.set(None);
                        }
                    }
                }
                TimelineCaptureAction::Stop | TimelineCaptureAction::Discard => {
                    if let Some(episode) = episode {
                        let keep = action == TimelineCaptureAction::Stop;
                        let finished_episode = episode.clone();
                        self.session.stop_recording(
                            &episode,
                            RecordingCompletion::new(
                                keep,
                                move |_: spiral_robotics_common::recording::EpisodeInfo| {
                                    episode_data_cache.write().invalidate(&finished_episode);
                                    let next_revision =
                                        episode_preview_revision.peek().wrapping_add(1);
                                    episode_preview_revision.set(next_revision);
                                    episode_preview_state.set(None);
                                    interactive_preview_target.set(None);
                                },
                            ),
                        );
                    }
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
                            self.session.start_replay(summary, playhead());
                        }
                    }
                    TimelineRobotSyncAction::Pause => {
                        self.session.set_replay_paused(&episode, true)
                    }
                    TimelineRobotSyncAction::Resume => {
                        self.session.set_replay_paused(&episode, false)
                    }
                }
            }
            TimelineAction::Seek(request)
                if subject.target().as_ref() == Some(request.target()) =>
            {
                controller.stop_preview();
                playhead.set(request.seconds());
                if let Some(episode) = review_episode.clone() {
                    let target = (episode.clone(), request.seconds().replay_slot());
                    if request.committed()
                        || interactive_preview_target.peek().as_ref() != Some(&target)
                    {
                        interactive_preview_target.set(Some(target));
                        EpisodePreviewSignals::new(
                            episode_preview_state,
                            episode_preview_revision,
                            episode_data_cache,
                        )
                        .request(EpisodePreviewRequest::new(
                            episode,
                            None,
                            request.seconds(),
                        ));
                    }
                }
                if request.committed()
                    && request.moves_robot()
                    && let Some((episode, robot, source_arm)) = seek_context
                {
                    self.session.physical_seek(
                        episode,
                        TimelineSeekBody::new(robot, source_arm, request.seconds()),
                    );
                }
            }
            TimelineAction::SetFollowLive(next) => {
                follow_live.set(next);
                if next {
                    controller.stop_preview();
                    if let Some(recording) = self.session.telemetry().recording() {
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
                    if let Some(replay) = self.session.telemetry().replay()
                        && subject.episode_id() == Some(replay.episode())
                    {
                        self.session.stop_replay(replay.episode());
                    }
                }
            }
            TimelineAction::ReloadRecorded { episode }
                if review_episode.as_deref() == Some(episode.as_str()) =>
            {
                episode_preview_state.set(None);
                episode_data_cache.write().invalidate(&episode);
                {
                    EpisodePreviewSignals::new(
                        episode_preview_state,
                        episode_preview_revision,
                        episode_data_cache,
                    )
                    .request(EpisodePreviewRequest::new(
                        episode,
                        None,
                        playhead(),
                    ));
                }
            }
            _ => self
                .session
                .project()
                .report("The Timeline subject changed before the action was applied."),
        }
    }
}

impl LiveTimelineActions {
    pub(super) fn on_key(self, event: KeyboardEvent) {
        if event.modifiers().ctrl()
            || event.modifiers().meta()
            || event.modifiers().alt()
            || !self.session.shortcuts_enabled()
        {
            return;
        }
        let recording = self.session.telemetry().recording();
        let subject = recording
            .clone()
            .map(TimelineSubject::Recording)
            .unwrap_or_else(|| {
                TimelineSubject::empty("Live", "Record an episode to begin using the shared clock.")
            });
        let capture = match event.key() {
            Key::Character(key) if key.eq_ignore_ascii_case("r") => Some(if recording.is_some() {
                crate::panels::timeline::panel::TimelineCaptureAction::Stop
            } else {
                crate::panels::timeline::panel::TimelineCaptureAction::Start
            }),
            Key::Character(key) if key.eq_ignore_ascii_case("x") && recording.is_some() => {
                if event.modifiers().shift() {
                    Some(crate::panels::timeline::panel::TimelineCaptureAction::Discard)
                } else {
                    event.prevent_default();
                    self.session.project().report_info_from(
                        crate::shell::notifications::MessageSource::Live,
                        "Press Shift+X to discard this take.",
                    );
                    None
                }
            }
            _ => None,
        };
        if let Some(action) = capture {
            event.prevent_default();
            self.dispatch(
                subject,
                TimelineAction::Capture {
                    episode: recording.map(|recording| recording.episode().to_owned()),
                    action,
                },
            );
        }
    }
}
impl LiveSession {
    pub(super) fn live_timeline_capabilities(
        self,
        subject: &TimelineSubject,
    ) -> TimelineCapabilities {
        let record = if let Some(reason) = self.recording_blocker() {
            TimelineGate::blocked(reason)
        } else {
            TimelineGate::allowed()
        };
        let preview = if subject.recorded_episode().is_none() {
            TimelineGate::blocked(
                "Select a completed episode or pause the current recording first.",
            )
        } else if !subject.available() {
            TimelineGate::blocked(
                subject
                    .problem()
                    .unwrap_or("This Timeline subject is unavailable."),
            )
        } else {
            TimelineGate::allowed()
        };
        let seek = preview.clone();
        let synced_playback = match subject.episode() {
            Some(episode) => self
                .replay_blocker(episode)
                .map(TimelineGate::blocked)
                .unwrap_or_else(TimelineGate::allowed),
            None => TimelineGate::blocked("Select a completed episode to play it with robot sync."),
        };
        let robot_sync = if !subject.is_paused_recording() && subject.episode().is_none() {
            TimelineGate::blocked(
                "Pause or select a completed episode before synchronizing a robot.",
            )
        } else if !self.telemetry().controls_available() {
            TimelineGate::blocked("Reconnect telemetry before synchronizing a robot.")
        } else if self.telemetry().mode_active()
            && !matches!(self.telemetry().mode(), ModeSelection::Replay { .. })
        {
            TimelineGate::blocked("Stop the active control mode before synchronizing a robot.")
        } else if self.timeline_seek_context(subject).is_none() {
            TimelineGate::blocked("Connect and calibrate a compatible follower to synchronize it.")
        } else {
            TimelineGate::allowed()
        };
        TimelineCapabilities::new(record, preview, seek)
            .with_robot_controls(synced_playback, robot_sync)
    }

    fn timeline_seek_context(
        self,
        subject: &TimelineSubject,
    ) -> Option<(String, String, Option<String>)> {
        let follower = self
            .telemetry()
            .connected_robots()
            .into_iter()
            .find(|robot| {
                robot.calibrated()
                    && robot
                        .robot_type()
                        .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
            })?;
        let (episode, arms) = match subject {
            TimelineSubject::Recording(recording) if recording.paused() => {
                (recording.episode().to_owned(), recording.arms().to_owned())
            }
            TimelineSubject::Episode { episode, .. } => (
                episode.id().to_owned(),
                episode
                    .arms()
                    .iter()
                    .map(|arm| arm.id().to_owned())
                    .collect(),
            ),
            TimelineSubject::Empty { .. } | TimelineSubject::Recording(_) => return None,
        };
        let source_arm = arms
            .iter()
            .find(|arm| **arm == follower.id())
            .or_else(|| {
                arms.iter()
                    .find(|arm| arm.to_ascii_lowercase().contains("follower"))
            })
            .or_else(|| arms.first())
            .cloned();
        Some((episode, follower.id().to_owned(), source_arm))
    }
}
