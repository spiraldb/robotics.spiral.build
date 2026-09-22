//! Consequential Studio command gateway built on the HTTP API.
//!
//! Components emit semantic or Timeline intent. The activity controller uses
//! this module to translate consequential intent into admitted commands. The
//! backend remains authoritative for recording, replay, hardware, and safety.

use crate::activities::live::RecordingClient;
use crate::robots::RobotClient;
use dioxus::prelude::*;
use spiral_robotics_common::recording::EpisodeInfo;
use spiral_robotics_common::robots::ModeSelection;

use crate::activities::live::session::LiveSession;
use crate::shell::notifications::MessageSource;

impl LiveSession {
    pub(super) fn recording_blocker(self) -> Option<String> {
        if !spiral_robotics_common::identity::DatasetId::is_canonical(
            self.capture().chosen().read().trim(),
        ) {
            Some("Choose a dataset ID using letters, numbers, '.', '_' or '-'.".to_owned())
        } else if matches!(self.telemetry().mode(), ModeSelection::Replay { .. }) {
            Some("Stop replay before recording.".to_owned())
        } else if self
            .telemetry()
            .snapshot()
            .read()
            .default_teleoperation_pair()
            .is_none()
        {
            Some("Connect and calibrate a compatible leader and follower first.".to_owned())
        } else if !self.telemetry().controls_available() {
            Some("Recording is paused while telemetry reconnects.".to_owned())
        } else {
            None
        }
    }

    pub(super) fn start_recording(self) {
        if let Some(reason) = self.recording_blocker() {
            self.project().report(reason);
            return;
        }
        let Some(pair) = self
            .telemetry()
            .snapshot()
            .read()
            .default_teleoperation_pair()
        else {
            return;
        };
        let dataset_id = self.capture().chosen().read().trim().to_owned();
        self.commands().run("", async move {
            RecordingClient::current()
                .start_recording(spiral_robotics_common::recording::RecordingStart::new(
                    dataset_id, pair,
                ))
                .await
                .map_err(|error| error.to_string())
                .map(|_| ())
        });
    }

    pub(super) fn set_recording_paused(self, episode: &str, paused: bool) {
        if self
            .telemetry()
            .recording()
            .as_ref()
            .map(|progress| progress.episode())
            != Some(episode)
        {
            self.project()
                .report("The recording changed before the Timeline action was applied.");
            return;
        }
        let future = async move {
            RecordingClient::current()
                .set_recording_paused(paused)
                .await
                .map_err(|error| error.to_string())
        };
        if paused {
            self.project()
                .run_safety("Recording paused. The episode remains open.", future);
        } else {
            self.commands()
                .run("Recording resumed in the same episode.", future);
        }
    }

    pub(super) fn stop_recording(self, episode: &str, completion: RecordingCompletion) {
        if self
            .telemetry()
            .recording()
            .as_ref()
            .map(|progress| progress.episode())
            != Some(episode)
        {
            self.project()
                .report("The recording changed before the Timeline action was applied.");
            return;
        }
        self.project().run_safety("", async move {
            let summary = RecordingClient::current()
                .stop_recording(completion.keep)
                .await
                .map_err(|error| error.to_string())?;
            if completion.keep {
                self.project().report_info_from(
                    MessageSource::Live,
                    format!("Saving episode to dataset ‘{}’.", summary.dataset_id()),
                );
            } else {
                self.project().report_success_from(
                    MessageSource::Live,
                    "Recording discarded. Teleoperation stopped.",
                );
            }
            (completion.on_completed)(summary);
            Ok(())
        });
    }

    fn replay_target(self, episode: &EpisodeInfo) -> Option<(String, Option<String>)> {
        let ready = self
            .telemetry()
            .connected_robots()
            .into_iter()
            .filter(|robot| robot.calibrated())
            .collect::<Vec<_>>();
        let target = ready
            .iter()
            .find(|robot| {
                robot
                    .robot_type()
                    .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
            })
            .or_else(|| ready.first())?
            .id()
            .to_owned();
        let source = episode
            .arm(&target)
            .map(|arm| arm.id().to_owned())
            .or_else(|| {
                episode
                    .arms()
                    .iter()
                    .find(|arm| arm.id().to_ascii_lowercase().contains("follower"))
                    .map(|arm| arm.id().to_owned())
            })
            .or_else(|| match episode.arms() {
                [only] => Some(only.id().to_owned()),
                _ => None,
            })?;
        Some((target, Some(source)))
    }

    pub(super) fn replay_blocker(self, episode: &EpisodeInfo) -> Option<String> {
        if !episode.usable() {
            Some("This episode did not finish and cannot be replayed.".to_owned())
        } else if self.telemetry().mode_active()
            && !self
                .telemetry()
                .replay()
                .is_some_and(|progress| progress.episode() == episode.id())
        {
            Some("Stop the current robot mode before replaying this episode.".to_owned())
        } else if !self.telemetry().controls_available() {
            Some("Replay is unavailable while telemetry reconnects.".to_owned())
        } else if self.replay_target(episode).is_none() {
            Some("Connect and calibrate a compatible replay target first.".to_owned())
        } else {
            None
        }
    }

    pub(super) fn start_replay(self, episode: &EpisodeInfo, start_seconds: f64) {
        if let Some(reason) = self.replay_blocker(episode) {
            self.project().report(reason);
            return;
        }
        let Some((robot, source_arm)) = self.replay_target(episode) else {
            return;
        };
        let episode = episode.id().to_owned();
        self.commands().run("", async move {
            RecordingClient::current()
                .start_replay(
                    spiral_robotics_common::recording::ReplayStart::new(
                        episode,
                        robot,
                        start_seconds,
                    )
                    .with_source_arm(source_arm),
                )
                .await
                .map_err(|error| error.to_string())
        });
    }

    pub(super) fn set_replay_paused(self, episode: &str, paused: bool) {
        if self
            .telemetry()
            .replay()
            .as_ref()
            .map(|progress| progress.episode())
            != Some(episode)
        {
            self.project()
                .report("The robot replay changed before the Timeline action was applied.");
            return;
        }
        let future = async move {
            RecordingClient::current()
                .set_replay_paused(paused)
                .await
                .map_err(|error| error.to_string())
        };
        if paused {
            self.project()
                .run_safety("Robot replay paused at the current frame.", future);
        } else {
            self.commands().run("Robot replay resumed.", future);
        }
    }

    pub(super) fn stop_replay(self, episode: &str) {
        if self
            .telemetry()
            .replay()
            .as_ref()
            .map(|progress| progress.episode())
            != Some(episode)
        {
            self.project()
                .report("The robot replay changed before the Timeline action was applied.");
            return;
        }
        self.project().run_safety(
            "Robot replay stopped. Motors may still be holding position.",
            async move {
                RobotClient::current()
                    .set_mode(ModeSelection::Idle)
                    .await
                    .map_err(|error| error.to_string())
            },
        );
    }
}

pub(crate) struct RecordingCompletion {
    keep: bool,
    on_completed: Box<dyn FnOnce(EpisodeInfo)>,
}

impl RecordingCompletion {
    pub(super) fn new(keep: bool, on_completed: impl FnOnce(EpisodeInfo) + 'static) -> Self {
        Self {
            keep,
            on_completed: Box::new(on_completed),
        }
    }
}

pub(crate) struct TimelineSeekBody {
    robot: String,
    source_arm: Option<String>,
    seconds: f64,
}

impl TimelineSeekBody {
    pub(super) fn new(robot: String, source_arm: Option<String>, seconds: f64) -> Self {
        Self {
            robot,
            source_arm,
            seconds,
        }
    }
}

impl LiveSession {
    pub(super) fn physical_seek(self, episode: String, request: TimelineSeekBody) {
        self.commands().run("", async move {
            RecordingClient::current()
                .seek_episode(
                    episode,
                    spiral_robotics_common::recording::EpisodeSeek::new(
                        request.robot,
                        request.seconds,
                    )
                    .with_source_arm(request.source_arm),
                )
                .await
                .map_err(|error| error.to_string())
        });
    }
}
