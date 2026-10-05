//! Consequential recording commands built on the robot connector's HTTP API.
//!
//! Components emit semantic or Timeline intent. The activity controller uses
//! this module to translate consequential intent into admitted commands. The
//! robot connector remains authoritative for recording, hardware, and safety.

use super::client::RecordingTransfer;
use crate::activities::live::RecordingClient;
use dioxus::prelude::*;
use spiral_robotics_common::recording::EpisodeInfo;

use crate::activities::live::session::LiveSession;
use crate::shell::notifications::MessageSource;

impl LiveSession {
    pub(super) fn recording_blocker(self) -> Option<String> {
        if !spiral_robotics_common::identity::DatasetId::is_canonical(
            self.capture().chosen().read().trim(),
        ) {
            Some("Choose a dataset ID using letters, numbers, '.', '_' or '-'.".to_owned())
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
        let video = *self.capture().video().read();
        self.commands().run("", async move {
            RecordingClient::current()
                .start_recording(spiral_robotics_common::recording::RecordingStart::new(
                    dataset_id, pair, video,
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
                self.project().report_live_info(format!(
                    "Saving episode to dataset ‘{}’.",
                    summary.dataset_id()
                ));
            } else {
                self.project().report_success_from(
                    MessageSource::Live,
                    "Recording discarded. Teleoperation stopped.",
                );
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
