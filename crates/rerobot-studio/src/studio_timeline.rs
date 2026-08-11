//! The shared episode timeline used by both the Live and Data workbenches.
//!
//! This component owns presentation and interaction intent only. Recording,
//! playback, seeking, and motor motion remain server-owned operations. In
//! particular, playback and a committed scrub can request physical motion only
//! when the caller passes an allowed robot-sync [`TimelineGate`] and the
//! operator has explicitly enabled “Sync robot”.

use dioxus::prelude::*;
use rerobot_core::arm::SO101_JOINTS;
use serde::Deserialize;

use crate::activity::{Activity, TimelineSubject, TimelineTarget};
use crate::recording_signals::{SignalStore, signal_color};

const TRACE_WIDTH: f64 = 1_000.0;
const TRACE_HEIGHT: f64 = 32.0;
const TRACE_POINT_BUDGET: usize = 256;

/// Whether the Timeline clock is stopped, previewing recorded data, or being
/// driven by the server-owned consequence of robot synchronization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TimelinePlayback {
    #[default]
    Stopped,
    PreviewPlaying,
    RobotSyncSeeking,
    RobotSyncPlaying,
    RobotSyncPaused,
}

/// Controlled interaction state shared by every Timeline subject.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StudioTimelineState {
    pub(crate) subject: TimelineSubject,
    pub(crate) playhead_seconds: f64,
    pub(crate) playback: TimelinePlayback,
    pub(crate) follow_live: bool,
    pub(crate) sync_robot: bool,
}

/// One measured point. `None` creates a visible gap rather than bridging a
/// missing sample with a line the hardware never reported.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineSignalPoint {
    pub seconds: f64,
    pub value: Option<f64>,
}

/// One arm's trace within a joint lane.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimelineSignalSeries {
    pub arm: String,
    pub points: Vec<TimelineSignalPoint>,
}

/// One of the six fixed SO-101 joint lanes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimelineSignalLane {
    pub joint: String,
    pub label: String,
    pub color: String,
    pub series: Vec<TimelineSignalSeries>,
}

/// Wire response from the bounded historical trace endpoint.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct RecordedTimelineTraces {
    pub episode: String,
    pub samples: u64,
    pub buckets: usize,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    pub lanes: Vec<RecordedTimelineTraceLane>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct RecordedTimelineTraceLane {
    pub joint: String,
    pub series: Vec<RecordedTimelineTraceSeries>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct RecordedTimelineTraceSeries {
    pub arm: String,
    pub points: Vec<RecordedTimelineTracePoint>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
pub struct RecordedTimelineTracePoint {
    pub seconds: f64,
    pub value: Option<f64>,
}

impl RecordedTimelineTraces {
    /// Adapt the wire response to the same fixed view model used by live
    /// [`SignalStore`] data. Missing or unknown lanes cannot disturb the
    /// canonical SO-101 order.
    pub fn signal_lanes(&self) -> [TimelineSignalLane; 6] {
        std::array::from_fn(|index| {
            let definition = SO101_JOINTS[index];
            let series = self
                .lanes
                .iter()
                .find(|lane| lane.joint == definition.name)
                .map(|lane| {
                    lane.series
                        .iter()
                        .map(|series| TimelineSignalSeries {
                            arm: series.arm.clone(),
                            points: series
                                .points
                                .iter()
                                .map(|point| TimelineSignalPoint {
                                    seconds: point.seconds,
                                    value: point.value,
                                })
                                .collect(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            TimelineSignalLane {
                joint: definition.name.to_owned(),
                label: definition.label.to_owned(),
                color: signal_color(definition.name).to_owned(),
                series,
            }
        })
    }
}

/// Fetch bounded historical traces for a completed or paused episode.
pub async fn load_recorded_timeline_traces(
    episode: &str,
    buckets: usize,
) -> Result<RecordedTimelineTraces, String> {
    let path = format!(
        "{}/traces?buckets={}",
        crate::api::episode_timeline_url(episode),
        buckets.clamp(1, 512)
    );
    crate::api::get(&path).await
}

/// Convert the studio's existing take signal store into the timeline's fixed
/// six-lane view model. Each lane overlays the arms that reported that joint.
pub fn signal_lanes_from_store(store: &SignalStore) -> [TimelineSignalLane; 6] {
    std::array::from_fn(|index| {
        let definition = SO101_JOINTS[index];
        let series = store
            .arms()
            .iter()
            .filter_map(|arm| {
                let joint = arm
                    .joints
                    .iter()
                    .find(|joint| joint.name == definition.name)?;
                let points = store
                    .times()
                    .iter()
                    .zip(&joint.values)
                    .map(|(seconds, value)| TimelineSignalPoint {
                        seconds: f64::from(*seconds),
                        value: value.map(f64::from),
                    })
                    .collect();
                Some(TimelineSignalSeries {
                    arm: arm.id.clone(),
                    points,
                })
            })
            .collect();

        TimelineSignalLane {
            joint: definition.name.to_owned(),
            label: definition.label.to_owned(),
            color: signal_color(definition.name).to_owned(),
            series,
        }
    })
}

/// Whether a Timeline action is currently available, with a concrete recovery
/// path when it is not.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineGate {
    pub(crate) allowed: bool,
    pub(crate) reason: Option<String>,
}

impl TimelineGate {
    pub(crate) fn allowed() -> Self {
        Self {
            allowed: true,
            reason: None,
        }
    }

    pub(crate) fn blocked(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineCapabilities {
    pub(crate) record: TimelineGate,
    pub(crate) preview: TimelineGate,
    pub(crate) seek: TimelineGate,
    /// Whether full-episode playback may drive the compatible robot.
    pub(crate) synced_playback: TimelineGate,
    /// Whether this Timeline subject may be armed to drive the robot.
    pub(crate) robot_sync: TimelineGate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TimelineCaptureAction {
    Start,
    Pause,
    Resume,
    Stop,
    Discard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TimelinePreviewAction {
    Play,
    Pause,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TimelineRobotSyncAction {
    Start,
    Pause,
    Resume,
}

/// One seek against a named recording or episode. `move_robot` can only be true
/// after the robot-sync gate and operator option have both passed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineSeekRequest {
    pub(crate) target: TimelineTarget,
    pub(crate) seconds: f64,
    pub(crate) move_robot: bool,
    pub(crate) committed: bool,
}

/// The Timeline's one user-operation vocabulary in every activity and state.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TimelineAction {
    Capture {
        episode: Option<String>,
        action: TimelineCaptureAction,
    },
    Preview {
        episode: String,
        action: TimelinePreviewAction,
    },
    RobotSyncTransport {
        episode: String,
        action: TimelineRobotSyncAction,
    },
    Seek(TimelineSeekRequest),
    SetFollowLive(bool),
    SetRobotSync {
        target: TimelineTarget,
        enabled: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimelineTransportIntent {
    Preview(TimelinePreviewAction),
    RobotSync(TimelineRobotSyncAction),
}

fn transport_intent(playback: TimelinePlayback, sync_robot: bool) -> TimelineTransportIntent {
    match playback {
        TimelinePlayback::PreviewPlaying => {
            TimelineTransportIntent::Preview(TimelinePreviewAction::Pause)
        }
        TimelinePlayback::RobotSyncSeeking | TimelinePlayback::RobotSyncPlaying => {
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Pause)
        }
        TimelinePlayback::RobotSyncPaused => {
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Resume)
        }
        TimelinePlayback::Stopped if sync_robot => {
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Start)
        }
        TimelinePlayback::Stopped => TimelineTransportIntent::Preview(TimelinePreviewAction::Play),
    }
}

/// The canonical timeline visualization shared by Live and Data.
///
/// The component is controlled: the activity controller owns the projection and
/// feeds requested actions back through [`StudioTimelineState`]. Scrubbing is
/// held locally until the range control commits, so physical motion is never
/// requested for every pointer move.
#[component]
pub(crate) fn StudioTimeline(
    state: StudioTimelineState,
    signal_lanes: [TimelineSignalLane; 6],
    capabilities: TimelineCapabilities,
    on_action: EventHandler<TimelineAction>,
) -> Element {
    let recording = state.subject.recording().cloned();
    let duration = state.subject.duration_seconds();
    let external_playhead = if recording.is_some() && state.follow_live {
        duration
    } else {
        state.playhead_seconds.clamp(0.0, duration)
    };
    let mut scrubbing = use_signal(|| false);
    let mut draft_playhead = use_signal(|| external_playhead);

    use_effect(move || {
        if !scrubbing() {
            draft_playhead.set(external_playhead);
        }
    });

    let shown_playhead = if scrubbing() {
        draft_playhead().clamp(0.0, duration)
    } else {
        external_playhead
    };
    let episode_id = state
        .subject
        .episode_id()
        .unwrap_or("No episode")
        .to_owned();
    let frames = state.subject.frames();
    let playback = state.playback;
    let preview_playing = playback == TimelinePlayback::PreviewPlaying;
    let robot_sync_active = matches!(
        state.playback,
        TimelinePlayback::RobotSyncSeeking | TimelinePlayback::RobotSyncPlaying
    );
    let transport_playing = preview_playing || robot_sync_active;
    let follow_live = state.follow_live;
    let range_max = duration.max(0.001);
    let playhead_percent = percent(shown_playhead, duration);
    let sync_robot = capabilities.robot_sync.allowed && state.sync_robot;
    let subject_label = match &state.subject {
        TimelineSubject::Empty { activity, .. } => activity.label(),
        TimelineSubject::Recording(_) => "Recording",
        TimelineSubject::Episode { .. } => "Episode",
    };
    let empty_reason = match &state.subject {
        TimelineSubject::Empty { reason, .. } => Some(reason.clone()),
        _ => None,
    };
    let status = match &state.subject {
        TimelineSubject::Empty { .. } => None,
        _ => state.subject.problem().map(str::to_owned),
    };
    let target = state.subject.target();
    let ticks = timeline_ticks(duration);

    let record_title = capabilities
        .record
        .reason
        .clone()
        .unwrap_or_else(|| "Start Teleop and record a new episode".to_owned());
    let transport_allowed = match state.playback {
        TimelinePlayback::RobotSyncSeeking
        | TimelinePlayback::RobotSyncPlaying
        | TimelinePlayback::RobotSyncPaused => true,
        TimelinePlayback::Stopped if sync_robot => capabilities.synced_playback.allowed,
        TimelinePlayback::Stopped | TimelinePlayback::PreviewPlaying => {
            capabilities.preview.allowed
        }
    };
    let transport_title = match state.playback {
        TimelinePlayback::PreviewPlaying => "Pause preview playback".to_owned(),
        TimelinePlayback::RobotSyncSeeking => "Pause synchronized robot positioning".to_owned(),
        TimelinePlayback::RobotSyncPlaying => "Pause synchronized robot playback".to_owned(),
        TimelinePlayback::RobotSyncPaused => "Resume synchronized robot playback".to_owned(),
        TimelinePlayback::Stopped if sync_robot => capabilities
            .synced_playback
            .reason
            .clone()
            .unwrap_or_else(|| {
                "Play the episode with the compatible robot synchronized".to_owned()
            }),
        TimelinePlayback::Stopped => capabilities
            .preview
            .reason
            .clone()
            .unwrap_or_else(|| "Play the recorded episode in the visualizations".to_owned()),
    };
    let transport_label = match state.playback {
        TimelinePlayback::PreviewPlaying
        | TimelinePlayback::RobotSyncSeeking
        | TimelinePlayback::RobotSyncPlaying => "Pause",
        TimelinePlayback::RobotSyncPaused => "Resume",
        TimelinePlayback::Stopped => "Play",
    };
    let sync_title = capabilities.robot_sync.reason.clone().unwrap_or_else(|| {
        "Keep the compatible robot synchronized while playing or seeking".to_owned()
    });
    let seek_title =
        capabilities.seek.reason.clone().unwrap_or_else(|| {
            "Drag or use the keyboard to seek the shared episode clock".to_owned()
        });
    let input_target = target.clone();
    let commit_target = target.clone();
    let preview_episode = state.subject.episode_id().map(str::to_owned);
    let sync_target = target.clone();
    let scrub_sync_episode = preview_episode.clone();
    let next_transport = transport_intent(playback, sync_robot);
    let capture_episode = recording.as_ref().map(|progress| progress.episode.clone());
    let capture_paused = recording.as_ref().is_some_and(|progress| progress.paused);
    let timeline_label = match state.subject.activity() {
        Activity::Live => "Live Timeline",
        Activity::Data => "Data Timeline",
    };

    rsx! {
        section {
            class: "studio-timeline flex min-h-0 flex-col border-t border-line bg-canvas text-ink",
            "aria-label": "{timeline_label} for {episode_id}",

            header { class: "flex min-h-9 flex-wrap items-center gap-2 border-b border-line px-2 py-1",
                if let Some(episode) = capture_episode.clone() {
                    button {
                        r#type: "button",
                        class: "btn btn-outline",
                        onclick: move |_| on_action.call(TimelineAction::Capture {
                            episode: Some(episode.clone()),
                            action: if capture_paused { TimelineCaptureAction::Resume } else { TimelineCaptureAction::Pause },
                        }),
                        if capture_paused { "Resume recording" } else { "Pause recording" }
                    }
                    button {
                        r#type: "button",
                        class: "btn btn-outline",
                        onclick: {
                            let episode = capture_episode.clone();
                            move |_| on_action.call(TimelineAction::Capture {
                                episode: episode.clone(),
                                action: TimelineCaptureAction::Stop,
                            })
                        },
                        "Stop"
                    }
                    button {
                        r#type: "button",
                        class: "btn btn-danger",
                        onclick: {
                            let episode = capture_episode.clone();
                            move |_| on_action.call(TimelineAction::Capture {
                                episode: episode.clone(),
                                action: TimelineCaptureAction::Discard,
                            })
                        },
                        "Discard"
                    }
                } else {
                    button {
                        r#type: "button",
                        class: "btn btn-primary",
                        disabled: !capabilities.record.allowed,
                        title: "{record_title}",
                        onclick: move |_| on_action.call(TimelineAction::Capture {
                            episode: None,
                            action: TimelineCaptureAction::Start,
                        }),
                        "Record"
                    }
                }

                button {
                    r#type: "button",
                    class: "btn btn-outline h-6 w-6 px-0",
                    disabled: !transport_allowed,
                    title: "{transport_title}",
                    "aria-label": "{transport_label}",
                    onclick: {
                        let episode = preview_episode.clone();
                        move |_| if let Some(episode) = episode.clone() {
                            match next_transport {
                                TimelineTransportIntent::Preview(action) => on_action.call(TimelineAction::Preview {
                                    episode,
                                    action,
                                }),
                                TimelineTransportIntent::RobotSync(action) => on_action.call(TimelineAction::RobotSyncTransport {
                                    episode,
                                    action,
                                }),
                            }
                        }
                    },
                    PlaybackIcon { playing: transport_playing }
                }

                label {
                    class: if capabilities.robot_sync.allowed { "flex min-h-6 cursor-pointer items-center gap-1 text-[11px]" } else { "flex min-h-6 items-center gap-1 text-[11px] text-ink-faint" },
                    title: "{sync_title}",
                    input {
                        r#type: "checkbox",
                        checked: state.sync_robot,
                        disabled: !capabilities.robot_sync.allowed,
                        onchange: {
                            let target = sync_target.clone();
                            move |event| if let Some(target) = target.clone() {
                                on_action.call(TimelineAction::SetRobotSync {
                                    target,
                                    enabled: event.checked(),
                                });
                            }
                        },
                    }
                    span { "Sync robot" }
                }

                div { class: "flex min-w-0 items-baseline gap-1",
                    strong { class: "truncate text-[12px]", "{episode_id}" }
                    span { class: "text-[10px] text-ink-faint", "{subject_label}" }
                }
                div { class: "ml-auto flex items-baseline gap-1 font-mono tabular-nums",
                    span { class: "text-[12px] font-medium", "{format_clock(shown_playhead)}" }
                    span { class: "text-[10px] text-ink-faint", "/ {format_clock(duration)}" }
                    span { class: "ml-1 text-[10px] text-ink-faint", "{frames} frames" }
                }
                if recording.is_some() {
                    button {
                        r#type: "button",
                        class: if follow_live { "btn btn-outline text-accent" } else { "btn btn-outline" },
                        "aria-pressed": follow_live,
                        title: if follow_live { "Stop following the newest recorded sample" } else { "Return the playhead to the newest recorded sample" },
                        onclick: move |_| on_action.call(TimelineAction::SetFollowLive(!follow_live)),
                        if follow_live { "Following live" } else { "Follow live" }
                    }
                }
            }

            if let Some(reason) = empty_reason {
                p { class: "m-0 border-b border-line bg-sunken px-2 py-1 text-[11px] text-ink-soft", role: "status",
                    "{reason}"
                }
            }
            if let Some(problem) = status {
                p { class: "m-0 border-b border-line bg-danger-soft px-2 py-1 text-[11px] text-danger", role: "alert",
                    "{problem}"
                }
            }

            div { class: "min-h-0 overflow-auto",
                div {
                    class: "relative grid min-w-[42rem]",
                    style: "grid-template-columns: 8.5rem minmax(30rem, 1fr)",

                    div { class: "border-b border-r border-line bg-sunken px-2 py-1 text-[10px] text-ink-faint", "Time" }
                    div { class: "relative h-8 border-b border-line bg-sunken",
                        for tick in ticks.iter() {
                            div {
                                key: "tick-{tick.ordinal}",
                                class: "absolute inset-y-0 border-l border-line",
                                style: "left: {tick.position:.4}%",
                                span {
                                    class: "absolute left-1 top-1 whitespace-nowrap font-mono text-[10px] tabular-nums text-ink-faint",
                                    "{tick.label}"
                                }
                            }
                        }
                        input {
                            r#type: "range",
                            class: "absolute inset-0 z-20 h-full w-full cursor-ew-resize opacity-0",
                            min: "0",
                            max: "{range_max}",
                            step: "0.001",
                            value: "{shown_playhead}",
                            disabled: duration <= 0.0 || !capabilities.seek.allowed,
                            title: "{seek_title}",
                            "aria-label": "Episode playhead",
                            "aria-valuetext": "{format_clock(shown_playhead)} of {format_clock(duration)}",
                            oninput: move |event| {
                                if let Ok(seconds) = event.value().parse::<f64>() {
                                    let beginning_scrub = !scrubbing();
                                    scrubbing.set(true);
                                    if beginning_scrub && robot_sync_active
                                        && let Some(episode) = scrub_sync_episode.clone()
                                    {
                                        on_action.call(TimelineAction::RobotSyncTransport {
                                            episode,
                                            action: TimelineRobotSyncAction::Pause,
                                        });
                                    }
                                    let seconds = seconds.clamp(0.0, duration);
                                    draft_playhead.set(seconds);
                                    if let Some(target) = input_target.clone() {
                                        on_action.call(TimelineAction::Seek(TimelineSeekRequest {
                                            target,
                                            seconds,
                                            move_robot: false,
                                            committed: false,
                                        }));
                                    }
                                }
                            },
                            onchange: move |_| {
                                let seconds = draft_playhead().clamp(0.0, duration);
                                scrubbing.set(false);
                                if recording.is_some() && follow_live && seconds + 0.025 < duration {
                                    on_action.call(TimelineAction::SetFollowLive(false));
                                }
                                if let Some(target) = commit_target.clone() {
                                    on_action.call(TimelineAction::Seek(TimelineSeekRequest {
                                        target,
                                        seconds,
                                        move_robot: sync_robot,
                                        committed: true,
                                    }));
                                }
                            },
                            onpointercancel: move |_| {
                                scrubbing.set(false);
                                draft_playhead.set(external_playhead);
                            },
                        }
                    }

                    for lane in signal_lanes.iter() {
                        div { key: "lane-{lane.joint}", class: "contents",
                            div {
                                class: "flex min-h-9 items-center gap-1 border-b border-r border-line px-2 text-[11px]",
                                span { class: "h-px w-3 shrink-0", style: "background: {lane.color}" }
                                span { class: "truncate", "{lane.label}" }
                            }
                            SignalLaneTrack {
                                lane: lane.clone(),
                                duration,
                            }
                        }
                    }

                    div {
                        class: "pointer-events-none absolute bottom-0 right-0 top-0 z-10",
                        style: "left: 8.5rem",
                        div {
                            class: "absolute bottom-0 top-0 w-px bg-accent",
                            style: "left: {playhead_percent:.6}%",
                            span { class: "absolute -left-1 top-0 h-2 w-2 rounded-full bg-accent" }
                        }
                    }
                }
            }

        }
    }
}

#[component]
fn SignalLaneTrack(lane: TimelineSignalLane, duration: f64) -> Element {
    let bounds = signal_bounds(&lane.series);
    rsx! {
        div {
            class: "relative min-h-9 overflow-hidden border-b border-line bg-canvas",
            role: "img",
            "aria-label": if lane.series.is_empty() {
                format!("{} had no samples", lane.label)
            } else {
                format!("{} over time for {} arm(s)", lane.label, lane.series.len())
            },
            if lane.series.is_empty() {
                span { class: "absolute inset-0 flex items-center px-2 text-[10px] text-ink-faint", "No samples" }
            } else {
                svg {
                    class: "absolute inset-0 h-full w-full",
                    view_box: "0 0 1000 32",
                    preserve_aspect_ratio: "none",
                    "aria-hidden": "true",
                    for (series_index, series) in lane.series.iter().enumerate() {
                        for (segment_index, geometry) in trace_paths_with_bounds(&series.points, duration, bounds).into_iter().enumerate() {
                            path {
                                key: "{series.arm}-{segment_index}",
                                d: "{geometry}",
                                fill: "none",
                                stroke: "{lane.color}",
                                stroke_width: if series_index == 0 { "1.5" } else { "1.2" },
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                                vector_effect: "non-scaling-stroke",
                                style: if series_index == 0 { "opacity: 0.95" } else { "opacity: 0.52" },
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlaybackIcon(playing: bool) -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            if playing {
                path { d: "M5 3.5v9M11 3.5v9" }
            } else {
                path { d: "m5 3.5 7 4.5-7 4.5z" }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct TimelineTick {
    ordinal: u8,
    seconds: f64,
    position: f64,
    label: String,
}

fn timeline_ticks(duration: f64) -> Vec<TimelineTick> {
    let duration = duration.max(0.0);
    (0_u8..=4)
        .map(|index| {
            let fraction = f64::from(index) / 4.0;
            let seconds = duration * fraction;
            TimelineTick {
                ordinal: index,
                seconds,
                position: fraction * 100.0,
                label: format_clock(seconds),
            }
        })
        .collect()
}

fn percent(seconds: f64, duration: f64) -> f64 {
    if !seconds.is_finite() || !duration.is_finite() || duration <= 0.0 {
        0.0
    } else {
        (seconds / duration * 100.0).clamp(0.0, 100.0)
    }
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

/// Build one SVG path per contiguous run of valid samples. Each run is reduced
/// by retaining extrema in source order, never averaged, so a brief peak stays
/// visible when a long episode is fitted into the lane.
fn trace_paths_with_bounds(
    points: &[TimelineSignalPoint],
    duration: f64,
    bounds: Option<(f64, f64)>,
) -> Vec<String> {
    let mut runs = Vec::<Vec<(f64, f64)>>::new();
    let mut current = Vec::<(f64, f64)>::new();

    for point in points {
        match point.value.filter(|value| value.is_finite()) {
            Some(value) if point.seconds.is_finite() => current.push((point.seconds, value)),
            _ if !current.is_empty() => runs.push(std::mem::take(&mut current)),
            _ => {}
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }

    let Some((low, high)) = bounds else {
        return Vec::new();
    };
    let span = (high - low).max(1.0);

    runs.into_iter()
        .filter_map(|run| {
            let reduced = reduce_extremes(&run, TRACE_POINT_BUDGET);
            let mut geometry = String::new();
            for (index, (seconds, value)) in reduced.into_iter().enumerate() {
                let x = percent(seconds, duration) / 100.0 * TRACE_WIDTH;
                let y = TRACE_HEIGHT - ((value - low) / span * (TRACE_HEIGHT - 6.0) + 3.0);
                let command = if index == 0 { 'M' } else { 'L' };
                use std::fmt::Write as _;
                let _ = write!(geometry, "{command}{x:.2},{y:.2}");
            }
            (!geometry.is_empty()).then_some(geometry)
        })
        .collect()
}

fn signal_bounds(series: &[TimelineSignalSeries]) -> Option<(f64, f64)> {
    let mut low = f64::INFINITY;
    let mut high = f64::NEG_INFINITY;
    for point in series.iter().flat_map(|series| &series.points) {
        if let Some(value) = point.value.filter(|value| value.is_finite()) {
            low = low.min(value);
            high = high.max(value);
        }
    }
    (low <= high).then_some((low, high))
}

fn reduce_extremes(points: &[(f64, f64)], budget: usize) -> Vec<(f64, f64)> {
    if points.len() <= budget || budget < 4 {
        return points.to_vec();
    }

    let bucket_count = (budget / 2).max(1);
    let bucket_size = points.len().div_ceil(bucket_count);
    let mut reduced = Vec::with_capacity(budget + 2);

    for bucket in points.chunks(bucket_size) {
        let min = bucket
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| left.1.total_cmp(&right.1));
        let max = bucket
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.1.total_cmp(&right.1));
        match (min, max) {
            (Some((min_index, min)), Some((max_index, max))) if min_index <= max_index => {
                reduced.push(*min);
                if min_index != max_index {
                    reduced.push(*max);
                }
            }
            (Some((_, min)), Some((_, max))) => {
                reduced.push(*max);
                reduced.push(*min);
            }
            _ => {}
        }
    }

    reduced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_and_percent_are_stable_for_invalid_values() {
        assert_eq!(format_clock(f64::NAN), "0:00.0");
        assert_eq!(format_clock(61.25), "1:01.2");
        assert_eq!(percent(5.0, 10.0), 50.0);
        assert_eq!(percent(5.0, 0.0), 0.0);
    }

    #[test]
    fn one_transport_routes_through_the_robot_only_when_sync_is_enabled() {
        assert_eq!(
            transport_intent(TimelinePlayback::Stopped, false),
            TimelineTransportIntent::Preview(TimelinePreviewAction::Play)
        );
        assert_eq!(
            transport_intent(TimelinePlayback::Stopped, true),
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Start)
        );
        assert_eq!(
            transport_intent(TimelinePlayback::PreviewPlaying, false),
            TimelineTransportIntent::Preview(TimelinePreviewAction::Pause)
        );
        assert_eq!(
            transport_intent(TimelinePlayback::RobotSyncPlaying, true),
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Pause)
        );
        assert_eq!(
            transport_intent(TimelinePlayback::RobotSyncSeeking, true),
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Pause)
        );
        assert_eq!(
            transport_intent(TimelinePlayback::RobotSyncPaused, true),
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Resume)
        );
    }

    #[test]
    fn empty_timeline_ticks_keep_unique_render_identities() {
        let ticks = timeline_ticks(0.0);
        let identities = ticks
            .iter()
            .map(|tick| tick.ordinal)
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(ticks.len(), 5);
        assert_eq!(identities.len(), ticks.len());
    }

    #[test]
    fn traces_break_at_missing_samples_and_preserve_a_peak() {
        let mut points = (0..600)
            .map(|index| TimelineSignalPoint {
                seconds: index as f64 / 20.0,
                value: Some(0.0),
            })
            .collect::<Vec<_>>();
        points[250].value = Some(90.0);
        points[400].value = None;

        let series = TimelineSignalSeries {
            arm: "follower".to_owned(),
            points: points.clone(),
        };
        let paths =
            trace_paths_with_bounds(&points, 30.0, signal_bounds(std::slice::from_ref(&series)));
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|path| path.contains("3.00")));
    }

    #[test]
    fn canonical_lane_helper_always_returns_six_joint_slots() {
        let lanes = signal_lanes_from_store(&SignalStore::default());
        assert_eq!(lanes.len(), 6);
        assert_eq!(lanes[0].joint, "shoulder_pan");
        assert_eq!(lanes[5].joint, "gripper");
    }

    #[test]
    fn recorded_wire_lanes_are_reordered_into_the_canonical_six_slots() {
        let response = RecordedTimelineTraces {
            episode: "episode_000007".to_owned(),
            samples: 10,
            buckets: 2,
            start_seconds: Some(0.0),
            end_seconds: Some(1.0),
            lanes: vec![RecordedTimelineTraceLane {
                joint: "gripper".to_owned(),
                series: vec![RecordedTimelineTraceSeries {
                    arm: "follower".to_owned(),
                    points: vec![
                        RecordedTimelineTracePoint {
                            seconds: 0.0,
                            value: Some(-0.5),
                        },
                        RecordedTimelineTracePoint {
                            seconds: 0.5,
                            value: None,
                        },
                    ],
                }],
            }],
        };

        let lanes = response.signal_lanes();
        assert_eq!(lanes.len(), 6);
        assert!(lanes[0].series.is_empty());
        assert_eq!(lanes[5].joint, "gripper");
        assert_eq!(lanes[5].series[0].arm, "follower");
        assert_eq!(lanes[5].series[0].points[1].value, None);
    }
}
