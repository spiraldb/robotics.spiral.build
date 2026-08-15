//! Consequential Studio command gateway built on the HTTP transport.
//!
//! Components emit semantic or Timeline intent. The activity controller uses
//! this module to translate consequential intent into wire requests. The
//! backend remains authoritative for recording, replay, hardware, and safety.

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_contracts::api::{
    EpisodeSummary, ModeSelection, SetRecordingPausedRequest, SetReplayPausedRequest,
    StartRecordingRequest, StartReplayRequest, StopRecordingRequest, default_teleoperation_pair,
};

use crate::frontend::api::http as api;
use crate::frontend::app::state::Studio;
use crate::frontend::format::format_clock;

pub(crate) fn recording_blocker(studio: Studio) -> Option<String> {
    if !spiral_robotics_contracts::spiral::is_dataset_id(studio.selected_dataset.read().trim()) {
        Some("Choose a dataset ID using letters, numbers, '.', '_' or '-'.".to_owned())
    } else if matches!(studio.mode(), ModeSelection::Replay { .. }) {
        Some("Stop replay before recording.".to_owned())
    } else if default_teleoperation_pair(&studio.snapshot.read().robots).is_none() {
        Some("Connect and calibrate a compatible leader and follower first.".to_owned())
    } else if !studio.controls_available() {
        Some("Recording is paused while telemetry reconnects.".to_owned())
    } else {
        None
    }
}

pub(crate) fn start_recording(studio: Studio) {
    if let Some(reason) = recording_blocker(studio) {
        studio.report(reason);
        return;
    }
    let request = StartRecordingRequest {
        dataset_id: studio.selected_dataset.read().trim().to_owned(),
        ..StartRecordingRequest::default()
    };
    studio.run("", async move {
        api::send_json::<_, EpisodeSummary>(Method::POST, "/api/episodes/record", Some(&request))
            .await
            .map(|_| ())
    });
}

pub(crate) fn set_recording_paused(studio: Studio, episode: &str, paused: bool) {
    if studio
        .recording()
        .as_ref()
        .map(|progress| progress.episode.as_str())
        != Some(episode)
    {
        studio.report("The recording changed before the Timeline action was applied.");
        return;
    }
    let request = SetRecordingPausedRequest { paused };
    let future =
        async move { api::send(Method::POST, "/api/episodes/record/pause", Some(&request)).await };
    if paused {
        studio.run_safety("Recording paused. The episode remains open.", future);
    } else {
        studio.run("Recording resumed in the same episode.", future);
    }
}

pub(crate) fn stop_recording(
    studio: Studio,
    episode: &str,
    keep: bool,
    on_completed: EventHandler<EpisodeSummary>,
) {
    if studio
        .recording()
        .as_ref()
        .map(|progress| progress.episode.as_str())
        != Some(episode)
    {
        studio.report("The recording changed before the Timeline action was applied.");
        return;
    }
    let request = StopRecordingRequest { keep };
    // "Episode saved." named nothing, so the one moment the operator has
    // earned something was also the moment the interface forgot what it was.
    // The real facts only exist once the server answers, so the safety message
    // covers the mode change and the summary replaces it with the take.
    let message = if keep {
        "Saving the episode. Teleoperation stopped."
    } else {
        "Recording discarded. Teleoperation stopped."
    };
    studio.run_safety(message, async move {
        let summary = api::send_json::<_, EpisodeSummary>(
            Method::POST,
            "/api/episodes/record/stop",
            Some(&request),
        )
        .await?;
        studio.upsert_episode(summary.clone());
        if keep {
            studio.report_episode_success(
                format!(
                    "Episode {} saved to Data · {} — {}, {} frames.",
                    crate::frontend::screens::data::workbench::short_id(&summary.id),
                    summary.dataset_id,
                    format_clock(summary.duration_seconds),
                    summary.frames
                ),
                summary.id.clone(),
            );
        }
        on_completed.call(summary);
        Ok(())
    });
}

pub(crate) fn replay_target(
    studio: Studio,
    episode: &EpisodeSummary,
) -> Option<(String, Option<String>)> {
    let ready = studio
        .connected_robots()
        .into_iter()
        .filter(|robot| robot.calibrated)
        .collect::<Vec<_>>();
    let target = ready
        .iter()
        .find(|robot| {
            robot
                .robot_type
                .as_deref()
                .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
        })
        .or_else(|| ready.first())?
        .id
        .clone();
    let source = episode
        .arm(&target)
        .map(|arm| arm.id.clone())
        .or_else(|| {
            episode
                .arms
                .iter()
                .find(|arm| arm.id.to_ascii_lowercase().contains("follower"))
                .map(|arm| arm.id.clone())
        })
        .or_else(|| match episode.arms.as_slice() {
            [only] => Some(only.id.clone()),
            _ => None,
        })?;
    Some((target, Some(source)))
}

pub(crate) fn replay_blocker(studio: Studio, episode: &EpisodeSummary) -> Option<String> {
    if !episode.usable() {
        Some("This episode did not finish and cannot be replayed.".to_owned())
    } else if studio.mode_active()
        && !studio
            .replay()
            .is_some_and(|progress| progress.episode == episode.id)
    {
        Some("Stop the current robot mode before replaying this episode.".to_owned())
    } else if !studio.controls_available() {
        Some("Replay is unavailable while telemetry reconnects.".to_owned())
    } else if replay_target(studio, episode).is_none() {
        Some("Connect and calibrate a compatible replay target first.".to_owned())
    } else {
        None
    }
}

pub(crate) fn start_replay(studio: Studio, episode: &EpisodeSummary, start_seconds: f64) {
    if let Some(reason) = replay_blocker(studio, episode) {
        studio.report(reason);
        return;
    }
    let Some((robot, source_arm)) = replay_target(studio, episode) else {
        return;
    };
    let request = StartReplayRequest {
        episode: episode.id.clone(),
        robot,
        source_arm,
        start_seconds,
    };
    studio.run("", async move {
        api::send(Method::POST, "/api/episodes/replay", Some(&request)).await
    });
}

pub(crate) fn set_replay_paused(studio: Studio, episode: &str, paused: bool) {
    if studio
        .replay()
        .as_ref()
        .map(|progress| progress.episode.as_str())
        != Some(episode)
    {
        studio.report("The robot replay changed before the Timeline action was applied.");
        return;
    }
    let request = SetReplayPausedRequest { paused };
    let future =
        async move { api::send(Method::POST, "/api/episodes/replay/pause", Some(&request)).await };
    if paused {
        studio.run_safety("Robot replay paused at the current frame.", future);
    } else {
        studio.run("Robot replay resumed.", future);
    }
}

pub(crate) fn stop_replay(studio: Studio, episode: &str) {
    if studio
        .replay()
        .as_ref()
        .map(|progress| progress.episode.as_str())
        != Some(episode)
    {
        studio.report("The robot replay changed before the Timeline action was applied.");
        return;
    }
    studio.run_safety(
        "Robot replay stopped. Motors may still be holding position.",
        async move { api::send(Method::POST, "/api/mode", Some(&ModeSelection::Idle)).await },
    );
}

#[derive(serde::Serialize)]
struct TimelineSeekBody {
    robot: String,
    source_arm: Option<String>,
    seconds: f64,
}

pub(crate) fn physical_seek(
    studio: Studio,
    episode: String,
    robot: String,
    source_arm: Option<String>,
    seconds: f64,
) {
    let path = format!("/api/episodes/{episode}/seek");
    let request = TimelineSeekBody {
        robot,
        source_arm,
        seconds,
    };
    studio.run("", async move {
        api::send_json::<_, serde_json::Value>(Method::POST, &path, Some(&request))
            .await
            .map(|_| ())
    });
}

/// Whether global transport shortcuts may act without stealing a text-editing
/// keystroke.
pub(crate) fn shortcuts_enabled() -> bool {
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
