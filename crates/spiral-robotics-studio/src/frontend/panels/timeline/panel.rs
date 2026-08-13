//! Dockable Timeline panel shared by the Live and Data screens.
//!
//! This component owns presentation and interaction intent only. Recording,
//! playback, seeking, and motor motion remain server-owned operations. In
//! particular, playback and a committed scrub can request physical motion only
//! when the caller passes an allowed robot-sync [`TimelineGate`] and the
//! operator has explicitly enabled “Sync robot”.

use charming::Chart;
use charming::component::{Axis, Grid};
use charming::datatype::{CompositeValue, DataFrame, DataPoint};
use charming::element::{
    AxisLabel, AxisLine, AxisTick, AxisType, Emphasis, LineStyle, Sampling, SplitLine, Symbol,
};
use charming::series::Line;
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_core::arm::SO101_JOINTS;

use crate::frontend::app::activity::{Activity, TimelineSubject, TimelineTarget};
use crate::frontend::components::charts::{
    CharmingChart, downsample_extrema, studio_numeric_tooltip,
};
use crate::frontend::panels::timeline::signals::{SignalStore, signal_color};

const TIMELINE_CHART_ID: &str = "studio-timeline-signal-chart";
const TRACE_POINT_BUDGET: usize = 512;
const TRACE_LANE_HEIGHT: f64 = 28.0;

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
pub(crate) struct TimelineSignalPoint {
    pub seconds: f64,
    pub value: Option<f64>,
}

/// One arm's trace within a joint lane.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TimelineSignalSeries {
    pub arm: String,
    pub points: Vec<TimelineSignalPoint>,
}

/// One of the six fixed SO-101 joint lanes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TimelineSignalLane {
    pub joint: String,
    pub label: String,
    pub color: String,
    pub series: Vec<TimelineSignalSeries>,
}

/// Wire response from the bounded historical trace endpoint.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct RecordedTimelineTraces {
    pub episode: String,
    pub samples: u64,
    pub buckets: usize,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    pub lanes: Vec<RecordedTimelineTraceLane>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct RecordedTimelineTraceLane {
    pub joint: String,
    pub series: Vec<RecordedTimelineTraceSeries>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct RecordedTimelineTraceSeries {
    pub arm: String,
    pub points: Vec<RecordedTimelineTracePoint>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct RecordedTimelineTracePoint {
    pub seconds: f64,
    pub value: Option<f64>,
}

impl RecordedTimelineTraces {
    /// Adapt the wire response to the same fixed view model used by live
    /// [`SignalStore`] data. Missing or unknown lanes cannot disturb the
    /// canonical SO-101 order.
    pub(crate) fn signal_lanes(&self) -> [TimelineSignalLane; 6] {
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
pub(crate) async fn load_recorded_timeline_traces(
    episode: &str,
    buckets: usize,
) -> Result<RecordedTimelineTraces, String> {
    let path = format!(
        "{}/traces?buckets={}",
        crate::frontend::api::http::episode_timeline_url(episode),
        buckets.clamp(1, 512)
    );
    crate::frontend::api::http::get(&path).await
}

/// Convert the studio's existing take signal store into the timeline's fixed
/// six-lane view model. Each lane overlays the arms that reported that joint.
pub(crate) fn signal_lanes_from_store(store: &SignalStore) -> [TimelineSignalLane; 6] {
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
    /// Re-request the recorded traces for a named episode after a failure.
    ReloadRecorded {
        episode: String,
    },
}

/// Where the Timeline's signal lanes are getting their samples, and — when the
/// answer is "nowhere yet" — why.
///
/// Without this the lanes could only ask `series.is_empty()`, so a request still
/// in flight and a request that died rendered identically to an episode the
/// backend confirmed was empty: six lanes reading "No samples" with an
/// `aria-label` of "had no samples", asserted in the past tense, for a recording
/// that had 1082 frames. DESIGN.md's Explicit-Source Rule names these states;
/// this is the type that carries them as far as the thing that draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum SignalSource {
    /// Live telemetry from the recording signal store.
    #[default]
    Live,
    /// A recorded episode whose traces have arrived.
    RecordedReady,
    /// A recorded episode whose traces are still in flight.
    RecordedLoading,
    /// A recorded episode whose traces will not arrive, and why.
    RecordedUnavailable { episode: String, reason: String },
}

impl SignalSource {
    /// True while the lanes have nothing to draw and no verdict yet.
    fn loading(&self) -> bool {
        matches!(self, Self::RecordedLoading)
    }

    fn unavailable(&self) -> Option<(&str, &str)> {
        match self {
            Self::RecordedUnavailable { episode, reason } => {
                Some((episode.as_str(), reason.as_str()))
            }
            _ => None,
        }
    }
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
    signal_source: SignalSource,
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
    let mut confirm_discard = use_signal(|| false);

    use_effect(move || {
        if !scrubbing() {
            draft_playhead.set(external_playhead);
        }
    });

    // A pending confirmation belongs to the take that raised it. If that take
    // ends by any other route — Stop, a server-side fault, an activity switch —
    // the armed Discard must not survive to greet the next one.
    let capture_identity = recording.as_ref().map(|progress| progress.episode.clone());
    use_effect(use_reactive((&capture_identity,), move |(identity,)| {
        if identity.is_none() {
            confirm_discard.set(false);
        }
    }));

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
    let full_episode_id = state.subject.episode_id().map(str::to_owned);
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
    let identity_label = match full_episode_id.as_deref() {
        Some(id) => format!(
            "{subject_label} {}",
            crate::frontend::screens::data::workbench::short_id(id)
        ),
        None => "No episode".to_owned(),
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
        Activity::Training => "Training step timeline",
        Activity::Rollout => "Rollout live timeline",
    };
    let signal_chart = timeline_signal_chart(&signal_lanes, duration);
    let signal_chart_label = format!("Six robot joint signals over {}", format_clock(duration));

    rsx! {
        section {
            class: "studio-timeline flex min-h-0 flex-col border-t border-line bg-canvas text-ink",
            "aria-label": "{timeline_label} for {episode_id}",

            header { class: "flex min-h-9 flex-wrap items-center gap-2 border-b border-line px-2 py-1",
                if let Some(episode) = capture_episode.clone() {
                    // Stop is the one reached for with a hand still on the arm
                    // once a take is running, so it takes the 28px take-button
                    // and the live red. Pause stays an ordinary 24px control and
                    // Discard sits behind a confirm at the far end of the row —
                    // the three used to be the same size, 8px apart, with the
                    // only unconfirmed one being the only unrecoverable one.
                    button {
                        r#type: "button",
                        class: "take-button take-button-live",
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
                        class: "btn btn-outline",
                        onclick: move |_| on_action.call(TimelineAction::Capture {
                            episode: Some(episode.clone()),
                            action: if capture_paused { TimelineCaptureAction::Resume } else { TimelineCaptureAction::Pause },
                        }),
                        if capture_paused { "Resume recording" } else { "Pause recording" }
                    }
                    div { class: "ml-1 flex items-center gap-1 border-l border-line pl-2",
                        if confirm_discard() {
                            span { class: "text-[11px] text-danger", "Discard this take?" }
                            button {
                                r#type: "button",
                                class: "btn btn-danger",
                                onclick: {
                                    let episode = capture_episode.clone();
                                    move |_| {
                                        confirm_discard.set(false);
                                        on_action.call(TimelineAction::Capture {
                                            episode: episode.clone(),
                                            action: TimelineCaptureAction::Discard,
                                        });
                                    }
                                },
                                "Discard"
                            }
                            button {
                                r#type: "button",
                                class: "btn btn-outline",
                                onclick: move |_| confirm_discard.set(false),
                                "Keep"
                            }
                        } else {
                            button {
                                r#type: "button",
                                class: "btn btn-danger",
                                "aria-label": "Discard this take without saving",
                                onclick: move |_| confirm_discard.set(true),
                                "Discard"
                            }
                        }
                    }
                } else {
                    button {
                        r#type: "button",
                        class: "take-button",
                        disabled: !capabilities.record.allowed,
                        title: "{record_title}",
                        "aria-label": if capabilities.record.allowed { "Record".to_owned() } else { format!("Record — {record_title}") },
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

                // The list says "#21", the Scene tab says "Episode · #21", and
                // this used to say "episode_000021" — three spellings of one
                // take on one screen. The short form leads because it is what
                // the operator reads everywhere else; the durable key stays
                // beside it in micro, on the page, because it is the string
                // Query and any wire request actually need.
                div { class: "flex min-w-0 items-baseline gap-1",
                    strong { class: "truncate text-[12px]", "{identity_label}" }
                    if let Some(full) = full_episode_id.clone() {
                        span { class: "truncate font-mono text-[10px] tabular-nums text-ink-faint", "{full}" }
                    }
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
            // The reason belongs on the page once, not repeated into six lanes —
            // the lanes say "Unavailable" and this says why and offers the way
            // out. Without it a stalled trace request was indistinguishable from
            // an empty episode, and there was nothing to press.
            if let Some((failed_episode, reason)) = signal_source.unavailable() {
                div {
                    class: "m-0 flex flex-wrap items-center gap-2 border-b border-line bg-caution-soft px-2 py-1 text-[11px] text-caution",
                    role: "alert",
                    span { class: "min-w-0 flex-1", "Signals for {failed_episode} are unavailable. {reason}" }
                    button {
                        r#type: "button",
                        class: "btn btn-outline",
                        onclick: {
                            let episode = failed_episode.to_owned();
                            move |_| on_action.call(TimelineAction::ReloadRecorded {
                                episode: episode.clone(),
                            })
                        },
                        "Retry"
                    }
                }
            }

            div { class: "min-h-0 overflow-auto",
                div {
                    class: "timeline-signal-grid",

                    div { class: "timeline-signal-labels",
                        div { class: "timeline-time-label", "Time" }
                        for lane in signal_lanes.iter() {
                            div {
                                key: "label-{lane.joint}",
                                class: "timeline-signal-label",
                                span { class: "timeline-signal-swatch", style: "background: {lane.color}" }
                                span { class: "truncate", "{lane.label}" }
                            }
                        }
                    }

                    div { class: "timeline-signal-data",
                        div { class: "timeline-time-axis",
                            for tick in ticks.iter() {
                                div {
                                    key: "tick-{tick.ordinal}",
                                    class: "timeline-time-tick",
                                    style: "left: {tick.position:.4}%",
                                    span { "{tick.label}" }
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

                        div { class: "timeline-signal-chart-stack",
                            CharmingChart {
                                id: TIMELINE_CHART_ID.to_owned(),
                                chart: signal_chart,
                                class: "timeline-signal-chart".to_owned(),
                                aria_label: signal_chart_label,
                            }
                            div { class: "timeline-signal-lane-overlays",
                                for lane in signal_lanes.iter() {
                                    SignalLaneState {
                                        key: "state-{lane.joint}",
                                        lane: lane.clone(),
                                        source: signal_source.clone(),
                                    }
                                }
                            }
                        }

                        div {
                            class: "timeline-playhead",
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
fn SignalLaneState(lane: TimelineSignalLane, source: SignalSource) -> Element {
    let empty = lane.series.is_empty();
    let loading = empty && source.loading();
    let unavailable = empty && source.unavailable().is_some();
    let label = if loading {
        format!("{} samples are loading", lane.label)
    } else if unavailable {
        format!("{} samples are unavailable", lane.label)
    } else if empty {
        format!("{} had no samples", lane.label)
    } else {
        String::new()
    };
    rsx! {
        div {
            class: "timeline-signal-lane-state",
            role: empty.then_some("status"),
            "aria-busy": if loading { "true" } else { "false" },
            "aria-label": (!label.is_empty()).then_some(label),
            "aria-hidden": (!empty).then_some("true"),
            if loading {
                // A resting placeholder, not a shimmer: the reduced-motion block
                // strips animation anyway, and a lane that pulses while the arm
                // is live reads as telemetry rather than as absence.
                span { class: "timeline-signal-loading" }
            } else if unavailable {
                span { class: "timeline-signal-unavailable", "Unavailable" }
            } else if empty {
                span { class: "timeline-signal-empty", "No samples" }
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

fn timeline_signal_chart(lanes: &[TimelineSignalLane; 6], duration: f64) -> Chart {
    let mut chart = Chart::new()
        .animation(false)
        .tooltip(studio_numeric_tooltip(3));

    for (lane_index, lane) in lanes.iter().enumerate() {
        let grid_index = lane_index as f64;
        let (low, high) = signal_bounds(&lane.series).unwrap_or((0.0, 1.0));
        let padding = ((high - low) * 0.08).max(0.5);
        chart = chart
            .grid(
                Grid::new()
                    .id(format!("timeline-grid-{lane_index}"))
                    .left(0)
                    .right(0)
                    .top(lane_index as f64 * TRACE_LANE_HEIGHT)
                    .height(TRACE_LANE_HEIGHT),
            )
            .x_axis(
                hidden_timeline_axis(AxisType::Value, grid_index)
                    .min(0)
                    .max(duration.max(0.001)),
            )
            .y_axis(
                hidden_timeline_axis(AxisType::Value, grid_index)
                    .min(low - padding)
                    .max(high + padding)
                    .scale(true),
            );

        for (series_index, series) in lane.series.iter().enumerate() {
            chart = chart.series(
                Line::new()
                    .id(format!("{}-{}-{series_index}", lane.joint, series.arm))
                    .name(format!("{} · {}", lane.label, series.arm))
                    .x_axis_index(grid_index)
                    .y_axis_index(grid_index)
                    .show_symbol(false)
                    .symbol(Symbol::None)
                    .sampling(Sampling::Minmax)
                    .connect_nulls(false)
                    .line_style(
                        LineStyle::new()
                            .color(lane.color.clone())
                            .width(if series_index == 0 { 1.5 } else { 1.2 })
                            .opacity(if series_index == 0 { 0.95 } else { 0.52 }),
                    )
                    .emphasis(Emphasis::new().disabled(true))
                    .data(timeline_series_data(&series.points, TRACE_POINT_BUDGET)),
            );
        }
    }

    chart
}

fn hidden_timeline_axis(kind: AxisType, grid_index: f64) -> Axis {
    Axis::new()
        .type_(kind)
        .grid_index(grid_index)
        .show(false)
        .axis_line(AxisLine::new().show(false))
        .axis_tick(AxisTick::new().show(false))
        .axis_label(AxisLabel::new().show(false))
        .split_line(SplitLine::new().show(false))
}

/// Convert gaps to ECharts null points and retain extrema before crossing the
/// Wasm boundary. Pathological intermittent signals keep their longest valid
/// runs so the payload remains bounded instead of freezing the operator UI.
fn timeline_series_data(points: &[TimelineSignalPoint], budget: usize) -> DataFrame {
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
    if runs.is_empty() {
        return Vec::new();
    }

    let max_runs = (budget / 4).max(1);
    if runs.len() > max_runs {
        let mut selected = (0..runs.len()).collect::<Vec<_>>();
        selected.sort_by_key(|index| std::cmp::Reverse(runs[*index].len()));
        selected.truncate(max_runs);
        selected.sort_unstable();
        runs = selected
            .into_iter()
            .map(|index| std::mem::take(&mut runs[index]))
            .collect();
    }

    let total_valid = runs.iter().map(Vec::len).sum::<usize>().max(1);
    let point_budget = budget.saturating_sub(runs.len().saturating_sub(1));
    let mut data = DataFrame::new();
    let mut previous_end = None;

    for run in runs {
        let share =
            ((run.len() as f64 / total_valid as f64) * point_budget as f64).round() as usize;
        let share = share.max(4).min(run.len());
        let reduced = downsample_extrema(&run, share);
        if let (Some(end), Some((start, _))) = (previous_end, reduced.first()) {
            data.push(xy_point((end + start) / 2.0, None));
        }
        previous_end = reduced.last().map(|(seconds, _)| *seconds);
        data.extend(
            reduced
                .into_iter()
                .map(|(seconds, value)| xy_point(seconds, Some(value))),
        );
    }

    data
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

fn xy_point(x: f64, y: Option<f64>) -> DataPoint {
    DataPoint::from(vec![CompositeValue::from(x), CompositeValue::from(y)])
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

        let data = timeline_series_data(&points, TRACE_POINT_BUDGET);
        let data = serde_json::to_value(data).expect("trace data should serialize");
        let rows = data.as_array().expect("trace data should be an array");

        assert!(rows.iter().any(|row| row[1].is_null()));
        assert!(rows.iter().any(|row| row[1] == 90.0));
        assert!(rows.len() <= TRACE_POINT_BUDGET + 4);
    }

    #[test]
    fn timeline_chart_uses_one_grid_per_canonical_lane() {
        let chart = timeline_signal_chart(&signal_lanes_from_store(&SignalStore::default()), 12.0);
        let option = serde_json::to_value(chart).expect("timeline chart should serialize");

        assert_eq!(option["grid"].as_array().map(Vec::len), Some(6));
        assert_eq!(option["xAxis"].as_array().map(Vec::len), Some(6));
        assert_eq!(option["yAxis"].as_array().map(Vec::len), Some(6));
        assert_eq!(option["animation"], false);
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
