//! Consequential recording commands built on the robot connector's HTTP API.
//!
//! Components emit semantic or Timeline intent. The activity controller uses
//! this module to translate consequential intent into admitted commands. The
//! robot connector remains authoritative for recording, hardware, and safety.

use super::next_take::ControlKind;
use super::policy::PolicyClient;
use super::recording_client::RecordingTransfer;
use crate::activities::rollout::RecordingClient;
use dioxus::prelude::*;
use spiral_robotics_common::recording::{ControlSource, EpisodeInfo, RecordingStart};

use crate::activities::rollout::session::RolloutSession;
use crate::shell::notifications::MessageSource;

impl RolloutSession {
    /// Why Record cannot start a take with its control source now, if it cannot.
    pub(super) fn recording_blocker(self) -> Option<String> {
        if !spiral_robotics_common::identity::DatasetId::is_canonical(
            self.next_take().dataset().read().trim(),
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
            // A take is a whole run: it starts with the policy, not partway in.
            ControlKind::Policy if self.policy().active_session().is_some() => {
                Some("Recording starts with the next policy run.".to_owned())
            }
            ControlKind::Policy => self.policy_blocker(),
        }
    }

    /// Start the next take's control source and record it: the leader and follower
    /// for Teleop, or the chosen checkpoint on the follower for Policy, whose
    /// take begins with its first action once it has loaded.
    pub(super) fn start_recording(self) {
        if let Some(reason) = self.recording_blocker() {
            self.project().report(reason);
            return;
        }
        let setup = self.telemetry().snapshot();
        let (control, policy) = match self.record_control() {
            ControlKind::Teleop => {
                let Some((leader, follower)) = setup.read().default_teleoperation_pair() else {
                    return;
                };
                (ControlSource::Teleoperate { leader, follower }, None)
            }
            ControlKind::Policy => {
                let follower = setup.read().follower().map(|robot| robot.id().to_owned());
                let Some(config) = follower.and_then(|id| self.policy().next_run(&id)) else {
                    return;
                };
                (ControlSource::Policy(config.clone()), Some(config))
            }
        };
        let next_take = self.next_take();
        let request = RecordingStart::new(
            next_take.dataset().read().trim().to_owned(),
            control,
            *next_take.video().read(),
        );
        self.commands().run("", async move {
            next_take.set_starting(true);
            let started = async {
                if let Some(config) = policy.as_ref() {
                    PolicyClient::current().store(config).await?;
                }
                RecordingClient::current().start_recording(request).await
            }
            .await;
            next_take.set_starting(false);
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
            let (dataset, episode) = (summary.dataset_id().to_owned(), summary.id().to_owned());
            (completion.on_completed)(summary);
            // A discarded take is stored too, marked as discarded.
            RecordingTransfer::current()
                .carry(&dataset, &episode)
                .await
                .map_err(|error| {
                    format!("The take stays on the robot connector until Studio stores it: {error}")
                })
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
