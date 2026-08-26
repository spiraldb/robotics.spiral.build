//! Consequential Studio command gateway built on the HTTP transport.
//!
//! Components emit semantic or Timeline intent. The activity controller uses
//! this module to translate consequential intent into wire requests. The
//! backend remains authoritative for recording, replay, hardware, and safety.

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_api::recording::{
    EpisodeInfo, SetRecordingPausedRequest, SetReplayPausedRequest, StartRecordingRequest,
    StartReplayRequest, StopRecordingRequest,
};
use spiral_robotics_api::robots::ModeSelection;

use crate::activities::data::state::DataController;
use crate::activities::data::workbench::EpisodeIdPresentation;
use crate::client::http as api;
use crate::format::ClockPresentation;
use crate::shell::state::Studio;

impl Studio {
    pub(crate) fn recording_blocker(self) -> Option<String> {
        let studio = self;
        if !spiral_robotics_api::identity::DatasetId::is_canonical(
            studio.selected_dataset().read().trim(),
        ) {
            Some("Choose a dataset ID using letters, numbers, '.', '_' or '-'.".to_owned())
        } else if matches!(studio.mode(), ModeSelection::Replay { .. }) {
            Some("Stop replay before recording.".to_owned())
        } else if studio
            .snapshot()
            .read()
            .default_teleoperation_pair()
            .is_none()
        {
            Some("Connect and calibrate a compatible leader and follower first.".to_owned())
        } else if !studio.controls_available() {
            Some("Recording is paused while telemetry reconnects.".to_owned())
        } else {
            None
        }
    }

    pub(crate) fn start_recording(self) {
        let studio = self;
        if let Some(reason) = studio.recording_blocker() {
            studio.report(reason);
            return;
        }
        let request = StartRecordingRequest::new(studio.selected_dataset().read().trim());
        studio.run("", async move {
            api::Http::send_json::<_, EpisodeInfo>(
                Method::POST,
                "/api/episodes/record",
                Some(&request),
            )
            .await
            .map(|_| ())
        });
    }

    pub(crate) fn set_recording_paused(self, episode: &str, paused: bool) {
        let studio = self;
        if studio
            .recording()
            .as_ref()
            .map(|progress| progress.episode())
            != Some(episode)
        {
            studio.report("The recording changed before the Timeline action was applied.");
            return;
        }
        let request = SetRecordingPausedRequest::new(paused);
        let future = async move {
            api::Http::send(Method::POST, "/api/episodes/record/pause", Some(&request)).await
        };
        if paused {
            studio.run_safety("Recording paused. The episode remains open.", future);
        } else {
            studio.run("Recording resumed in the same episode.", future);
        }
    }

    pub(crate) fn stop_recording(self, episode: &str, completion: RecordingCompletion) {
        let studio = self;
        if studio
            .recording()
            .as_ref()
            .map(|progress| progress.episode())
            != Some(episode)
        {
            studio.report("The recording changed before the Timeline action was applied.");
            return;
        }
        let request = StopRecordingRequest::new(completion.keep);
        // "Episode saved." named nothing, so the one moment the operator has
        // earned something was also the moment the interface forgot what it was.
        // The real facts only exist once the server answers, so the safety message
        // covers the mode change and the summary replaces it with the take.
        let message = if completion.keep {
            "Saving the episode. Teleoperation stopped."
        } else {
            "Recording discarded. Teleoperation stopped."
        };
        studio.run_safety(message, async move {
            let summary = api::Http::send_json::<_, EpisodeInfo>(
                Method::POST,
                "/api/episodes/record/stop",
                Some(&request),
            )
            .await?;
            completion.data.upsert_episode(summary.clone());
            if completion.keep {
                studio.report_episode_success(
                    format!(
                        "Episode {} saved to Data · {} — {}, {} frames.",
                        summary.id().short_episode_id(),
                        summary.dataset_id(),
                        summary.duration_seconds().clock_label(),
                        summary.frames()
                    ),
                    summary.id().to_owned(),
                );
            }
            completion.on_completed.call(summary);
            Ok(())
        });
    }

    pub(crate) fn replay_target(self, episode: &EpisodeInfo) -> Option<(String, Option<String>)> {
        let studio = self;
        let ready = studio
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

    pub(crate) fn replay_blocker(self, episode: &EpisodeInfo) -> Option<String> {
        let studio = self;
        if !episode.usable() {
            Some("This episode did not finish and cannot be replayed.".to_owned())
        } else if studio.mode_active()
            && !studio
                .replay()
                .is_some_and(|progress| progress.episode() == episode.id())
        {
            Some("Stop the current robot mode before replaying this episode.".to_owned())
        } else if !studio.controls_available() {
            Some("Replay is unavailable while telemetry reconnects.".to_owned())
        } else if studio.replay_target(episode).is_none() {
            Some("Connect and calibrate a compatible replay target first.".to_owned())
        } else {
            None
        }
    }

    pub(crate) fn start_replay(self, episode: &EpisodeInfo, start_seconds: f64) {
        let studio = self;
        if let Some(reason) = studio.replay_blocker(episode) {
            studio.report(reason);
            return;
        }
        let Some((robot, source_arm)) = studio.replay_target(episode) else {
            return;
        };
        let request = StartReplayRequest::new(episode.id(), robot)
            .with_source_arm(source_arm)
            .starting_at(start_seconds);
        studio.run("", async move {
            api::Http::send(Method::POST, "/api/episodes/replay", Some(&request)).await
        });
    }

    pub(crate) fn set_replay_paused(self, episode: &str, paused: bool) {
        let studio = self;
        if studio.replay().as_ref().map(|progress| progress.episode()) != Some(episode) {
            studio.report("The robot replay changed before the Timeline action was applied.");
            return;
        }
        let request = SetReplayPausedRequest::new(paused);
        let future = async move {
            api::Http::send(Method::POST, "/api/episodes/replay/pause", Some(&request)).await
        };
        if paused {
            studio.run_safety("Robot replay paused at the current frame.", future);
        } else {
            studio.run("Robot replay resumed.", future);
        }
    }

    pub(crate) fn stop_replay(self, episode: &str) {
        let studio = self;
        if studio.replay().as_ref().map(|progress| progress.episode()) != Some(episode) {
            studio.report("The robot replay changed before the Timeline action was applied.");
            return;
        }
        studio.run_safety(
            "Robot replay stopped. Motors may still be holding position.",
            async move { api::Http::send(Method::POST, "/api/mode", Some(&ModeSelection::Idle)).await },
        );
    }
}

pub(crate) struct RecordingCompletion {
    data: DataController,
    keep: bool,
    on_completed: EventHandler<EpisodeInfo>,
}

impl RecordingCompletion {
    pub(crate) const fn new(
        data: DataController,
        keep: bool,
        on_completed: EventHandler<EpisodeInfo>,
    ) -> Self {
        Self {
            data,
            keep,
            on_completed,
        }
    }
}

#[derive(serde::Serialize)]
pub(crate) struct TimelineSeekBody {
    robot: String,
    source_arm: Option<String>,
    seconds: f64,
}

impl TimelineSeekBody {
    pub(crate) fn new(robot: String, source_arm: Option<String>, seconds: f64) -> Self {
        Self {
            robot,
            source_arm,
            seconds,
        }
    }
}

impl Studio {
    pub(crate) fn physical_seek(self, episode: String, request: TimelineSeekBody) {
        let studio = self;
        let path = format!("/api/episodes/{episode}/seek");
        studio.run("", async move {
            api::Http::send_json::<_, serde_json::Value>(Method::POST, &path, Some(&request))
                .await
                .map(|_| ())
        });
    }
}

impl Studio {
    /// Whether global transport shortcuts may act without stealing a text-editing
    /// keystroke.
    pub(crate) fn shortcuts_enabled(self) -> bool {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return false;
        };
        document
            .active_element()
            .map(|element| {
                !matches!(
                    element.tag_name().to_uppercase().as_str(),
                    "INPUT" | "TEXTAREA" | "SELECT"
                )
            })
            .unwrap_or(true)
    }
}
