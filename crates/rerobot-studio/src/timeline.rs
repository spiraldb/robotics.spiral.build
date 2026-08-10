//! Recording, replay, and export in one compact session rail.
//!
//! The workflow is intentionally literal: record episodes while teleoperating,
//! select an episode, replay it on a connected arm, then export the completed
//! session. Quality diagnostics remain in the files and on the wire, but they
//! do not compete with the operator's primary job in this surface.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{
    EpisodeSummary, ImportResponse, ModeSelection, RecordingProgress, StartRecordingRequest,
    StartReplayRequest, StopRecordingRequest, default_teleoperation_pair,
};

use crate::api;
use crate::icons::{Transport, Trash};
use crate::state::{Studio, studio};

/// Most episodes shown in the current session strip.
const MAX_VISIBLE_EPISODES: usize = 120;

pub fn start_recording(studio: Studio) {
    // The keyboard shortcut and the visible button share this guard. A disabled
    // button that `R` can bypass is not actually a safety boundary.
    if let Some(reason) = recording_blocker(studio) {
        studio.report(reason);
        return;
    }
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
        "Episode saved. Teleoperation stopped."
    } else {
        "Recording discarded. Teleoperation stopped. The episode remains on disk but will not be exported."
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

fn replay(studio: Studio, episode: String, robot: String, source_arm: Option<String>) {
    let request = StartReplayRequest {
        episode,
        robot,
        source_arm,
    };
    studio.run("", async move {
        api::send(Method::POST, "/api/episodes/replay", Some(&request)).await
    });
}

/// End a replay from the rail that started it.
fn stop_replay(studio: Studio) {
    studio.run_safety(
        "Replay stopped. Motors may still be holding position.",
        async move { api::send(Method::POST, "/api/mode", Some(&ModeSelection::Idle)).await },
    );
}

#[component]
pub fn TimelineRail(compact: bool, on_toggle: EventHandler<()>) -> Element {
    let mut studio = studio();
    let episodes = studio.episodes();
    let recording = studio.recording();

    // Land on the newest usable episode. An incomplete artifact remains in the
    // strip, but it must not turn a successful session into a disabled default.
    use_effect(move || {
        if studio.selected_episode.read().is_none() && studio.recording().is_none() {
            let episodes = studio.episodes();
            if let Some(latest) = preferred_episode(&episodes) {
                studio.selected_episode.set(Some(latest.id.clone()));
            }
        }
    });

    if compact {
        return rsx! {
            section { class: "timeline-rail timeline-rail-compact", "aria-label": "Dataset recording",
                div { class: "timeline-toolbar",
                    TransportBlock { recording }
                    button {
                        r#type: "button",
                        class: "dataset-sheet-toggle",
                        "aria-label": "Open Dataset",
                        onclick: move |_| on_toggle.call(()),
                        "Dataset"
                    }
                    p { class: "timeline-collapsed-note",
                        if studio.recording().is_some() {
                            "Recording continues. Open Dataset to see this episode."
                        } else {
                            "Open Dataset for episodes, replay, import, and download."
                        }
                    }
                }
            }
        };
    }

    rsx! {
        section { class: "timeline-rail", "aria-label": "Dataset recording and replay",
            header { class: "dataset-sheet-header",
                strong { "Dataset" }
                button {
                    r#type: "button",
                    class: "dataset-sheet-close",
                    "aria-label": "Collapse Dataset",
                    onclick: move |_| on_toggle.call(()),
                    "Done"
                }
            }
            // Transport, episodes and session actions share one row. They used
            // to be two, with the toolbar's middle left empty across the full
            // width of the window while the episodes sat on a row of their own
            // underneath — so the strip now takes the space that was already
            // there rather than paying for a second row to sit in.
            div { class: "timeline-toolbar",
                TransportBlock { recording: recording.clone() }
                EpisodeStrip { episodes: episodes.clone(), recording }
                SessionSummary { episodes: episodes.clone() }
            }
            // With nothing recorded, the strip's own empty state is the whole
            // answer; a second row saying "select an episode" underneath it
            // would be asking for something that does not exist yet.
            if !episodes.is_empty() {
                EpisodeDetail { selected: studio.focused_episode() }
            }
        }
    }
}

/// Record transport with the elapsed time visible from across the bench.
#[component]
fn TransportBlock(recording: Option<RecordingProgress>) -> Element {
    let studio = studio();
    let live = recording.is_some();
    let blocked = if live {
        None
    } else {
        recording_blocker(studio)
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
                "aria-label": if live { "Stop teleoperation and save this episode" } else { "Start teleoperation and record an episode" },
                title: if live { "Stop teleoperation and save this episode" } else { "Release the leader, hold the follower, and record" },
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
            // The elapsed clock itself stays silent — announcing it ten times a
            // second would bury everything else — so the transitions a screen
            // reader actually needs are announced once each instead.
            span { class: "sr-only", role: "status",
                if live { "Recording." } else { "Not recording." }
            }
            if live {
                // The way out of a bad take is printed the same way the way in
                // is. It used to live only in a tooltip, on the one control an
                // operator reaches for while both eyes are on the arm.
                div { class: "take-cancel-block",
                    button {
                        r#type: "button",
                        class: "take-cancel",
                        onclick: move |_| stop_recording(studio, false),
                        "Cancel"
                    }
                    span { class: "timeline-readout-label", "X to discard" }
                }
            }
            if let Some(reason) = blocked {
                span { class: "take-blocked", "{reason}" }
            }
        }
    }
}

fn recording_blocker(studio: Studio) -> Option<&'static str> {
    if matches!(studio.mode(), ModeSelection::Replay { .. }) {
        Some("Stop replay before recording")
    } else if default_teleoperation_pair(&studio.snapshot.read().robots).is_none() {
        Some("Connect and calibrate two compatible arms before recording")
    } else if !studio.controls_available() {
        Some("Recording is paused while telemetry reconnects")
    } else {
        None
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
            div { class: "episode-strip-wrap episode-strip-empty",
                strong { "No episodes yet" }
                span { "Press Record or R to begin." }
            }
        };
    }

    rsx! {
        div { class: "episode-strip-wrap",
            ol { class: "episode-strip", "aria-label": "Recorded episodes, oldest first",
                // The cap is still stated, but at the head of the strip where
                // the older takes actually are, rather than in a heading row
                // that also spent its width restating what the strip is.
                if hidden > 0 {
                    li {
                        span {
                            class: "episode-strip-older",
                            title: "The newest {MAX_VISIBLE_EPISODES} episodes are shown",
                            "+{hidden} older"
                        }
                    }
                }
                for episode in shown.iter().cloned() {
                    li { key: "{episode.id}", class: "episode-card",
                        button {
                            r#type: "button",
                            class: episode_button_class(&episode, chosen.as_deref() == Some(episode.id.as_str())),
                            "aria-pressed": chosen.as_deref() == Some(episode.id.as_str()),
                            "aria-label": "{episode_label(&episode)}",
                            onclick: {
                                let id = episode.id.clone();
                                move |_| selected.set(Some(id.clone()))
                            },
                            span { class: "episode-item-head",
                                span { class: "episode-index", "{short_id(&episode.id)}" }
                                span { class: "episode-duration", {format!("{}s", format_seconds(episode.duration_seconds))} }
                            }
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
                        DeleteEpisode { id: episode.id.clone() }
                    }
                }
                if let Some(progress) = recording {
                    li {
                        div { class: "episode-item episode-item-live", "aria-label": "Recording {progress.episode}",
                            span { class: "episode-item-head",
                                span { class: "episode-index", "{short_id(&progress.episode)}" }
                                span { class: "episode-duration", {format_clock(progress.elapsed_seconds)} }
                            }
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
    let target = ready
        .iter()
        .find(|robot| {
            robot
                .robot_type
                .as_deref()
                .is_some_and(|kind| kind.contains("follower"))
        })
        .map(|robot| robot.id.clone());
    let source = target
        .as_ref()
        .filter(|target| episode.arm(target).is_some())
        .cloned()
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
        });
    // Only this episode's own replay turns the action into a transport; another
    // episode playing still reads as a blocker on this one.
    let playing = replaying
        .clone()
        .filter(|progress| progress.episode == episode.id);
    let busy = studio.mode_active() || replaying.is_some();
    let replay_blocked = if !episode.usable() {
        Some("Replay is unavailable because this episode did not finish.".to_owned())
    } else if target.is_none() {
        Some("Connect and calibrate the follower before replaying".to_owned())
    } else if source.is_none() {
        Some("This episode does not identify a follower trajectory".to_owned())
    } else if busy {
        Some("Stop the current mode before replaying".to_owned())
    } else if !studio.controls_available() {
        Some("Replay is paused while telemetry reconnects".to_owned())
    } else {
        None
    };

    let id = episode.id.clone();
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
                Fact { label: "Source", value: episode.source.clone() }
            }
            div { class: "episode-actions",
                // While this take is the one driving an arm, the rail stops
                // offering a disabled Replay and starts reporting the motion it
                // began: the escape belongs where the action was, not in a
                // header button that has quietly relabelled itself.
                if let Some(progress) = playing {
                    // Fixed width: this counts up sixty times a second, and a
                    // label that grows a character at frame 10, 100 and 1000
                    // walks the Stop button along with it.
                    span { class: "episode-progress", role: "status",
                        if progress.frame == 0 {
                            "Moving to the first pose…"
                        } else {
                            {format!("Frame {}/{}", progress.frame, progress.frames)}
                        }
                    }
                    button {
                        r#type: "button",
                        class: "btn btn-danger episode-replay",
                        onclick: move |_| stop_replay(studio),
                        "Stop replay"
                    }
                } else {
                    button {
                        r#type: "button",
                        class: "btn btn-primary episode-replay",
                        disabled: replay_blocked.is_some(),
                        onclick: {
                            let (id, target, source) = (id.clone(), target.clone(), source.clone());
                            move |_| if let Some(robot) = target.clone() {
                                replay(studio, id.clone(), robot, source.clone());
                            }
                        },
                        {
                            match &target {
                                Some(robot) => format!("Replay on {robot}"),
                                None => "Replay episode".to_owned(),
                            }
                        }
                    }
                    if let Some(reason) = blocked_note {
                        span { class: "take-blocked", "{reason}" }
                    }
                }
            }
            if let Some(problem) = &episode.problem {
                details { class: "technical-details episode-technical-details",
                    summary { "Technical details" }
                    code { "{problem}" }
                }
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
            span { class: "episode-delete-confirm",
                button {
                    r#type: "button", class: "episode-delete-yes",
                    title: "Permanently delete {id}",
                    onclick: {
                        let id = id.clone();
                        move |_| {
                            confirming.set(false);
                            delete(studio, id.clone());
                        }
                    },
                    "Delete"
                }
                button {
                    r#type: "button",
                    class: "episode-delete-no",
                    "aria-label": "Cancel deletion",
                    onclick: move |_| confirming.set(false),
                    "Cancel"
                }
            }
        } else {
            button {
                r#type: "button", class: "episode-delete",
                title: "Delete {id}",
                "aria-label": "Delete {id}",
                onclick: move |_| confirming.set(true),
                Trash {}
            }
        }
    }
}

/// Session size and the single action that leaves the studio: export.
#[component]
fn SessionSummary(episodes: Vec<EpisodeSummary>) -> Element {
    let studio = studio();
    let mut importing = use_signal(|| false);
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
            label {
                class: if importing() { "btn btn-outline dataset-import dataset-importing" } else { "btn btn-outline dataset-import" },
                title: "Import a LeRobot v2 or v3 ZIP",
                input {
                    class: "sr-only",
                    r#type: "file",
                    accept: ".zip,application/zip",
                    disabled: importing(),
                    onchange: move |event| {
                        let Some(file) = event.files().first().cloned() else { return };
                        importing.set(true);
                        let mut notice = studio.notice;
                        let mut selected = studio.selected_episode;
                        studio.run_utility("", async move {
                            let result = async {
                                let bytes = file
                                    .read_bytes()
                                    .await
                                    .map_err(|error| format!("Could not read {}: {error}", file.name()))?;
                                api::upload_json::<ImportResponse>(
                                    "/api/episodes/import",
                                    &bytes,
                                    "application/zip",
                                )
                                .await
                            }
                            .await;
                            importing.set(false);
                            match result {
                                Ok(result) => {
                                    if let Some(id) = result.first_episode.clone() {
                                        selected.set(Some(id));
                                    }
                                    notice.set(Some(crate::state::Notice::new(
                                        format!(
                                            "Imported {} {} ({} frames).",
                                            result.episodes,
                                            if result.episodes == 1 { "episode" } else { "episodes" },
                                            result.frames,
                                        ),
                                        false,
                                    )));
                                    Ok(())
                                }
                                Err(error) => Err(error),
                            }
                        });
                    }
                }
                if importing() { "Importing…" } else { "Import" }
            }
            if exportable.is_empty() {
                button { r#type: "button", class: "btn btn-outline dataset-download", disabled: true, "Download dataset" }
                span { class: "take-blocked", "Record an episode before downloading" }
            } else {
                a {
                    class: "btn btn-outline dataset-download",
                    href: "/api/episodes/export.zip",
                    download: "rerobot-lerobot-v3-dataset.zip",
                    title: "Download this session as a LeRobot v3 ZIP",
                    "Download dataset"
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

fn preferred_episode(episodes: &[EpisodeSummary]) -> Option<&EpisodeSummary> {
    episodes
        .iter()
        .rev()
        .find(|episode| episode.usable() && !episode.discarded)
        .or_else(|| episodes.last())
}

/// A generated episode id as the take number an operator counts in.
pub(crate) fn short_id(id: &str) -> String {
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
            recording_gaps: Vec::new(),
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

    #[test]
    fn newest_usable_episode_wins_over_a_later_incomplete_one() {
        let usable = episode("episode_000001");
        let mut incomplete = episode("episode_000002");
        incomplete.problem = Some("recording did not finish".to_owned());
        let episodes = vec![usable, incomplete];
        assert_eq!(preferred_episode(&episodes).unwrap().id, "episode_000001");
    }
}
