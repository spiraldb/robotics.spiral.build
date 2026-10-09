//! Consequential recording commands built on the robot connector's HTTP API.
//!
//! Components emit semantic or Timeline intent. The activity controller uses
//! this module to translate consequential intent into admitted commands. The
//! robot connector remains authoritative for recording, hardware, and safety.

use super::next_episode::ControlKind;
use super::policy::PolicyControl;
use crate::activities::rollout::RecordingClient;
use dioxus::prelude::*;
use spiral_robotics_common::recording::{ControlSource, EpisodeInfo, RecordingStart};

use crate::activities::rollout::session::RolloutSession;
use crate::shell::notifications::MessageSource;

impl RolloutSession {
    /// Why Record cannot start an episode with its control source now, if it cannot.
    pub(super) fn recording_blocker(self) -> Option<String> {
        if !spiral_robotics_common::identity::DatasetId::is_canonical(
            self.next_episode().dataset().read().trim(),
        ) {
            return Some("Choose a dataset ID using letters, numbers, '.', '_' or '-'.".to_owned());
        }
        match self.record_control() {
            ControlKind::Teleop
                if self
                    .telemetry()
                    .snapshot()
                    .read()
                    .default_teleoperation_pair()
                    .is_none() =>
            {
                Some("Connect and calibrate a compatible leader and follower first.".to_owned())
            }
            ControlKind::Teleop if !self.telemetry().controls_available() => {
                Some("Recording is paused while telemetry reconnects.".to_owned())
            }
            ControlKind::Teleop => None,
            ControlKind::Policy => self.policy_blocker(),
        }
    }

    /// Start the next episode's control source and record it: the leader and
    /// follower for Teleop. Recording a policy is not supported yet.
    pub(super) fn start_recording(self) {
        if let Some(reason) = self.recording_blocker() {
            self.project().report(reason);
            return;
        }
        let control = match self.record_control() {
            ControlKind::Teleop => {
                let pair = self
                    .telemetry()
                    .snapshot()
                    .read()
                    .default_teleoperation_pair();
                let Some((leader, follower)) = pair else {
                    return;
                };
                ControlSource::Teleoperate { leader, follower }
            }
            ControlKind::Policy => {
                self.project().report(PolicyControl::UNSUPPORTED);
                return;
            }
        };
        let next_episode = self.next_episode();
        let request = RecordingStart::new(
            next_episode.dataset().read().trim().to_owned(),
            control,
            *next_episode.video().read(),
        );
        self.commands().run("", async move {
            next_episode.set_starting(true);
            let started = RecordingClient::current().start_recording(request).await;
            next_episode.set_starting(false);
            started.map(|_| ()).map_err(|error| error.to_string())
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
                .run_reported("Recording paused. The episode remains open.", future);
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
        self.project().run_reported("", async move {
            let summary = RecordingClient::current()
                .stop_recording(completion.keep)
                .await
                .map_err(|error| error.to_string())?;
            if completion.keep {
                self.project().report_rollout_info(format!(
                    "Saving episode to dataset ‘{}’.",
                    summary.dataset_id()
                ));
            } else {
                self.project()
                    .report_success_from(MessageSource::Rollout, "Recording discarded.");
            }
            // The robot connector uploads the episode, a discarded one too,
            // marked as discarded; Rollout stores it once Studio holds it.
            (completion.on_completed)(summary);
            Ok(())
        });
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
