//! Recording, replay, and export in one compact session rail.
//!
//! The workflow is intentionally literal: record episodes while teleoperating,
//! select an episode, replay it on a connected arm, then export the completed
//! session. Quality diagnostics remain in the files and on the wire, but they
//! do not compete with the operator's primary job in this surface.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{
    EpisodeSummary, ExportRequest, ExportResponse, RecordingProgress, StartRecordingRequest,
    StartReplayRequest, StopRecordingRequest,
};

use crate::api;
use crate::icons::Transport;
use crate::state::{Studio, studio};

/// Most episodes shown in the current session strip.
const MAX_VISIBLE_EPISODES: usize = 120;

pub fn start_recording(studio: Studio) {
    let request = StartRecordingRequest::default();
    studio.run("", async move {
        api::send_json::<_, EpisodeSummary>(Method::POST, "/api/episodes/record", Some(&request))
            .await
            .map(|_| ())
    });
}

pub fn stop_recording(studio: Studio, keep: bool) {
    let mut selected = studio.selected_episode;
    let request = StopRecordingRequest { keep };
    let message = if keep {
        "Episode saved."
    } else {
        "Recording cancelled. The episode remains on disk but will not be exported."
    };
    // Stopping is how the partial file becomes a complete episode, so this
    // remains available while normal controls are paused.
    studio.run_safety(message, async move {
        let summary = api::send_json::<_, EpisodeSummary>(
            Method::POST,
            "/api/episodes/record/stop",
            Some(&request),
        )
        .await?;
        selected.set(Some(summary.id));
        Ok(())
    });
}

fn delete(studio: Studio, id: String) {
    let mut selected = studio.selected_episode;
    let path = format!("/api/episodes/{id}");
    studio.run(format!("Deleted {id}."), async move {
        api::send::<()>(Method::DELETE, &path, None).await?;
        selected.set(None);
        Ok(())
    });
}

fn replay(studio: Studio, episode: String, robot: String) {
    let request = StartReplayRequest {
        episode,
        robot,
        source_arm: None,
    };
    studio.run("", async move {
        api::send(Method::POST, "/api/episodes/replay", Some(&request)).await
    });
}

#[component]
pub fn TimelineRail(compact: bool) -> Element {
    let mut studio = studio();
    let episodes = studio.episodes();
    let recording = studio.recording();

    // Land on the newest episode when the rail first opens. The operator can
    // replay immediately instead of first discovering that the detail row is a
    // selection-dependent empty state.
    use_effect(move || {
        if studio.selected_episode.read().is_none()
            && studio.recording().is_none()
            && let Some(latest) = studio.episodes().last()
        {
            studio.selected_episode.set(Some(latest.id.clone()));
        }
    });

    if compact {
        return rsx! {
            section { class: "timeline-rail timeline-rail-compact", "aria-label": "Dataset recording",
                div { class: "timeline-toolbar",
                    TransportBlock { recording }
                    p { class: "timeline-collapsed-note",
                        "Recording continues. Open the dataset panel to see the episode."
                    }
                }
            }
        };
    }

    rsx! {
        section { class: "timeline-rail", "aria-label": "Dataset recording and replay",
            div { class: "timeline-toolbar",
                TransportBlock { recording: recording.clone() }
                SessionSummary { episodes: episodes.clone() }
            }
            EpisodeStrip { episodes, recording }
            EpisodeDetail { selected: studio.focused_episode() }
        }
    }
}

/// Record transport with the elapsed time visible from across the bench.
#[component]
fn TransportBlock(recording: Option<RecordingProgress>) -> Element {
    let studio = studio();
    let connected = studio.connected_robots();
    let live = recording.is_some();
    let blocked = if live {
        None
    } else if connected.is_empty() {
        Some("Connect an arm before recording")
    } else if !studio.controls_available() {
        Some("Recording is paused while telemetry reconnects")
    } else {
        None
    };
    let elapsed = recording
        .as_ref()
        .map(|progress| progress.elapsed_seconds)
        .unwrap_or_default();

    rsx! {
        div { class: "timeline-transport",
            button {
                r#type: "button",
                class: if live { "take-button take-button-live" } else { "take-button" },
                disabled: blocked.is_some(),
                "aria-label": if live { "Stop recording and save this episode" } else { "Record an episode" },
                onclick: move |_| if live { stop_recording(studio, true) } else { start_recording(studio) },
                Transport { recording: live }
                span { if live { "Stop & save" } else { "Record" } }
            }
            div { class: "timeline-clock",
                span {
                    class: "timeline-elapsed",
                    role: if live { "timer" } else { "status" },
                    "aria-live": "off",
                    if live { "{format_clock(elapsed)}" } else { "0:00.0" }
                }
                span { class: "timeline-readout-label",
                    if let Some(progress) = &recording {
                        "{progress.frames} frames"
                    } else {
                        "R to record"
                    }
                }
            }
            if live {
                button {
                    r#type: "button",
                    class: "take-cancel",
                    title: "Stop this recording and leave it out of exports (X)",
                    onclick: move |_| stop_recording(studio, false),
                    "Cancel"
                }
            }
            if let Some(reason) = blocked {
                span { class: "take-blocked", "{reason}" }
            }
        }
    }
}

/// A compact, readable list of episodes in capture order.
#[component]
fn EpisodeStrip(episodes: Vec<EpisodeSummary>, recording: Option<RecordingProgress>) -> Element {
    let studio = studio();
    let mut selected = studio.selected_episode;
    let chosen = studio.selected_episode.read().clone();
    let hidden = episodes.len().saturating_sub(MAX_VISIBLE_EPISODES);
    let shown = &episodes[hidden..];

    if shown.is_empty() && recording.is_none() {
        return rsx! {
            div { class: "episode-strip episode-strip-empty",
                strong { "No episodes yet" }
                span { "Start teleoperation, then press Record or R." }
            }
        };
    }

    rsx! {
        div { class: "episode-strip-wrap",
            div { class: "episode-strip-heading",
                strong { "Episodes" }
                span { "Select one to replay" }
                if hidden > 0 { span { class: "ml-auto", "Newest {MAX_VISIBLE_EPISODES} shown" } }
            }
            ol { class: "episode-strip", "aria-label": "Recorded episodes, oldest first",
                for episode in shown.iter().cloned() {
                    li { key: "{episode.id}",
                        button {
                            r#type: "button",
                            class: episode_button_class(&episode, chosen.as_deref() == Some(episode.id.as_str())),
                            "aria-pressed": chosen.as_deref() == Some(episode.id.as_str()),
                            "aria-label": "{episode_label(&episode)}",
                            onclick: {
                                let id = episode.id.clone();
                                move |_| selected.set(Some(id.clone()))
                            },
                            span { class: "episode-index", "{short_id(&episode.id)}" }
                            span { class: "episode-duration", {format!("{}s", format_seconds(episode.duration_seconds))} }
                            span { class: "episode-state",
                                if episode.problem.is_some() {
                                    "Incomplete"
                                } else if episode.discarded {
                                    "Cancelled"
                                } else {
                                    "Recorded"
                                }
                            }
                        }
                    }
                }
                if let Some(progress) = recording {
                    li {
                        div { class: "episode-item episode-item-live", "aria-label": "Recording {progress.episode}",
                            span { class: "episode-index", "{short_id(&progress.episode)}" }
                            span { class: "episode-duration", {format_clock(progress.elapsed_seconds)} }
                            span { class: "episode-state", "Recording" }
                        }
                    }
                }
            }
        }
    }
}

/// The selected episode and the action it exists to support: replay.
#[component]
fn EpisodeDetail(selected: Option<EpisodeSummary>) -> Element {
    let studio = studio();
    let replaying = studio.replay();
    let Some(episode) = selected else {
        return rsx! {
            div { class: "episode-detail episode-detail-empty",
                "Select an episode to see its details and replay it."
            }
        };
    };

    let ready = studio
        .connected_robots()
        .into_iter()
        .filter(|robot| robot.calibrated)
        .collect::<Vec<_>>();
    let target = episode
        .arms
        .iter()
        .map(|arm| arm.id.clone())
        .find(|id| ready.iter().any(|robot| &robot.id == id))
        .or_else(|| ready.first().map(|robot| robot.id.clone()));
    let busy = studio.mode_active() || replaying.is_some();
    let replay_blocked = if !episode.usable() {
        Some("This episode did not finish recording".to_owned())
    } else if target.is_none() {
        Some("Connect and calibrate an arm before replaying".to_owned())
    } else if busy {
        Some("Stop the current mode before replaying".to_owned())
    } else if !studio.controls_available() {
        Some("Replay is paused while telemetry reconnects".to_owned())
    } else {
        None
    };

    let id = episode.id.clone();
    let delete_id = id.clone();
    let blocked_note = replay_blocked.clone();
    rsx! {
        div { class: "episode-detail",
            div { class: "episode-detail-title",
                span { class: "status-dot", style: if episode.problem.is_some() { "background: var(--tone-danger)" } else { "background: var(--tone-good)" } }
                strong { "{episode.id}" }
                if episode.discarded { span { class: "chip", "Cancelled · excluded from export" } }
            }
            dl { class: "episode-facts",
                Fact { label: "Duration", value: format!("{}s", format_seconds(episode.duration_seconds)) }
                Fact { label: "Frames", value: episode.frames.to_string() }
                Fact { label: "Rate", value: format!("{:.0} Hz", episode.sample_hz) }
                Fact { label: "Recorded in", value: episode.source.clone() }
            }
            div { class: "episode-actions",
                button {
                    r#type: "button",
                    class: "btn btn-primary episode-replay",
                    disabled: replay_blocked.is_some(),
                    onclick: {
                        let (id, target) = (id.clone(), target.clone());
                        move |_| if let Some(robot) = target.clone() {
                            replay(studio, id.clone(), robot);
                        }
                    },
                    {
                        match &target {
                            Some(robot) => format!("Replay on {robot}"),
                            None => "Replay episode".to_owned(),
                        }
                    }
                }
                DeleteEpisode { id: delete_id }
                if let Some(reason) = blocked_note {
                    span { class: "take-blocked", "{reason}" }
                }
            }
            if let Some(problem) = &episode.problem {
                p { class: "episode-problem", "{problem}" }
            }
        }
    }
}

#[component]
fn DeleteEpisode(id: String) -> Element {
    let studio = studio();
    let mut confirming = use_signal(|| false);
    let current = use_signal(|| id.clone());
    use_effect(move || {
        let mut current = current;
        if current() != id {
            current.set(id.clone());
            confirming.set(false);
        }
    });

    let id = current();
    rsx! {
        if confirming() {
            span { class: "take-confirm",
                button {
                    r#type: "button", class: "btn btn-danger",
                    onclick: {
                        let id = id.clone();
                        move |_| {
                            confirming.set(false);
                            delete(studio, id.clone());
                        }
                    },
                    "Delete episode"
                }
                button { r#type: "button", class: "btn", onclick: move |_| confirming.set(false), "Cancel" }
            }
        } else {
            button {
                r#type: "button", class: "btn btn-danger",
                title: "Permanently remove this episode from disk",
                onclick: move |_| confirming.set(true),
                "Delete"
            }
        }
    }
}

/// Session size and the single action that leaves the studio: export.
#[component]
fn SessionSummary(episodes: Vec<EpisodeSummary>) -> Element {
    let studio = studio();
    let mut exporting = use_signal(|| false);
    let mut destination = use_signal(String::new);
    let exportable = episodes
        .iter()
        .filter(|episode| episode.usable() && !episode.discarded)
        .collect::<Vec<_>>();
    let seconds = exportable
        .iter()
        .map(|episode| episode.duration_seconds)
        .sum::<f64>();
    let incomplete = episodes.len().saturating_sub(exportable.len());

    rsx! {
        div { class: "timeline-summary",
            div { class: "session-count",
                strong { "{exportable.len()}" }
                span { if exportable.len() == 1 { "episode" } else { "episodes" } }
                span { class: "session-duration", {format!("{} total", format_total_duration(seconds))} }
                if incomplete > 0 { span { class: "session-excluded", "{incomplete} not exported" } }
            }
            if exporting() {
                form {
                    class: "export-form",
                    onsubmit: move |event| {
                        event.prevent_default();
                        let name = destination().trim().to_owned();
                        if name.is_empty() {
                            studio.report("Name the directory to export into.");
                            return;
                        }
                        let request = ExportRequest { directory: name };
                        exporting.set(false);
                        destination.set(String::new());
                        let mut notice = studio.notice;
                        studio.run("", async move {
                            let result = api::send_json::<_, ExportResponse>(
                                Method::POST,
                                "/api/episodes/export",
                                Some(&request),
                            )
                            .await?;
                            notice.set(Some(crate::state::Notice {
                                text: format!(
                                    "Exported {} episodes ({} frames) to {}{}.",
                                    result.episodes,
                                    result.frames,
                                    result.directory,
                                    if result.skipped > 0 {
                                        format!(", skipping {} incomplete or cancelled", result.skipped)
                                    } else {
                                        String::new()
                                    },
                                ),
                                failed: false,
                            }));
                            Ok(())
                        });
                    },
                    label { class: "sr-only", r#for: "export-directory", "Export directory" }
                    input {
                        id: "export-directory",
                        class: "field export-field",
                        value: "{destination}",
                        placeholder: "dataset-name",
                        autofocus: true,
                        oninput: move |event| destination.set(event.value()),
                    }
                    button { r#type: "submit", class: "btn btn-primary", "Export" }
                    button { r#type: "button", class: "btn", onclick: move |_| exporting.set(false), "Cancel" }
                }
            } else {
                button {
                    r#type: "button",
                    class: "btn btn-outline",
                    disabled: exportable.is_empty(),
                    onclick: move |_| exporting.set(true),
                    "Export dataset"
                }
                if exportable.is_empty() {
                    span { class: "take-blocked", "Record an episode before exporting" }
                }
            }
        }
    }
}

#[component]
fn Fact(label: String, value: String) -> Element {
    rsx! {
        div { class: "episode-fact",
            dt { "{label}" }
            dd { class: "font-mono", "{value}" }
        }
    }
}

fn episode_button_class(episode: &EpisodeSummary, selected: bool) -> &'static str {
    match (selected, episode.problem.is_some(), episode.discarded) {
        (true, _, _) => "episode-item episode-item-selected",
        (false, true, _) => "episode-item episode-item-problem",
        (false, false, true) => "episode-item episode-item-cancelled",
        _ => "episode-item",
    }
}

fn short_id(id: &str) -> String {
    id.strip_prefix("episode_")
        .and_then(|number| number.parse::<u64>().ok())
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| id.to_owned())
}

fn episode_label(episode: &EpisodeSummary) -> String {
    let state = if episode.problem.is_some() {
        "incomplete"
    } else if episode.discarded {
        "cancelled"
    } else {
        "recorded"
    };
    format!(
        "{}, {}, {} seconds, {} frames",
        episode.id,
        state,
        format_seconds(episode.duration_seconds),
        episode.frames
    )
}

fn format_clock(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let minutes = (seconds / 60.0).floor() as u64;
    let remainder = seconds - minutes as f64 * 60.0;
    format!("{minutes}:{remainder:04.1}")
}

fn format_seconds(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0.0".to_owned();
    }
    if seconds < 60.0 {
        return format!("{seconds:.1}");
    }
    format_clock(seconds)
}

fn format_total_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "0:00".to_owned();
    }
    let rounded = seconds.round() as u64;
    format!("{}:{:02}", rounded / 60, rounded % 60)
}

/// Whether the studio is in a state where the record shortcut should act.
pub fn shortcuts_enabled() -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(id: &str) -> EpisodeSummary {
        EpisodeSummary {
            id: id.to_owned(),
            file: format!("{id}.jsonl"),
            arms: Vec::new(),
            frames: 1_478,
            duration_seconds: 24.7,
            sample_hz: 60.0,
            started_ms: 0,
            source: "Teleoperation · leader → follower".to_owned(),
            max_follow_error_degrees: Some(2.4),
            discarded: false,
            problem: None,
        }
    }

    #[test]
    fn clocks_read_like_a_stopwatch() {
        assert_eq!(format_clock(0.0), "0:00.0");
        assert_eq!(format_clock(9.44), "0:09.4");
        assert_eq!(format_clock(74.2), "1:14.2");
        assert_eq!(format_clock(f64::NAN), "0:00.0");
    }

    #[test]
    fn generated_episode_ids_become_human_take_numbers() {
        assert_eq!(short_id("episode_000024"), "#24");
        assert_eq!(short_id("calibration-pass"), "calibration-pass");
    }

    #[test]
    fn episode_labels_do_not_depend_on_colour() {
        let summary = episode("episode_000024");
        let label = episode_label(&summary);
        assert!(label.contains("recorded"));
        assert!(label.contains("24.7 seconds"));
        assert!(label.contains("1478 frames"));
    }
}
