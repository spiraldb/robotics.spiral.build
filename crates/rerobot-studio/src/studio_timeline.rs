//! The shared episode timeline used by both the Live and Data workbenches.
//!
//! This component owns presentation and interaction intent only. Recording,
//! playback, seeking, and motor motion remain server-owned operations. In
//! particular, a scrub can request physical motion only when the caller passes
//! an allowed [`TimelineMotionGate`] and the operator has explicitly enabled
//! “Move arm on release”.

use dioxus::prelude::*;
use rerobot_core::api::{EpisodeSummary, RecordingProgress};
use rerobot_core::arm::SO101_JOINTS;
use serde::Deserialize;

use crate::signals::{SignalStore, signal_color};

const TRACE_WIDTH: f64 = 1_000.0;
const TRACE_HEIGHT: f64 = 32.0;
const TRACE_POINT_BUDGET: usize = 256;

/// The episode the timeline is presenting.
#[derive(Clone, Debug, PartialEq)]
pub enum TimelineSource {
    /// A take that is still growing at its live edge.
    Recording(RecordingProgress),
    /// A completed or recoverable episode selected for inspection.
    Review(EpisodeSummary),
}

impl TimelineSource {
    pub fn episode_id(&self) -> &str {
        match self {
            Self::Recording(progress) => &progress.episode,
            Self::Review(episode) => &episode.id,
        }
    }

    pub fn duration_seconds(&self) -> f64 {
        match self {
            Self::Recording(progress) => progress.elapsed_seconds,
            Self::Review(episode) => episode.duration_seconds,
        }
        .max(0.0)
    }

    pub fn frames(&self) -> u64 {
        match self {
            Self::Recording(progress) => progress.frames,
            Self::Review(episode) => episode.frames,
        }
    }

    pub fn is_recording(&self) -> bool {
        matches!(self, Self::Recording(_))
    }

    pub fn is_paused(&self) -> bool {
        matches!(self, Self::Recording(progress) if progress.paused)
    }

    pub fn available(&self) -> bool {
        match self {
            Self::Recording(progress) => progress.error.is_none(),
            Self::Review(episode) => episode.usable(),
        }
    }

    pub fn problem(&self) -> Option<&str> {
        match self {
            Self::Recording(progress) => progress.error.as_deref(),
            Self::Review(episode) => episode.problem.as_deref(),
        }
    }
}

/// Controlled interaction state shared by Live and Data timeline hosts.
#[derive(Clone, Debug, PartialEq)]
pub struct StudioTimelineState {
    pub source: TimelineSource,
    pub playhead_seconds: f64,
    /// In Live this pauses or resumes the view; in Data it pauses or resumes
    /// episode playback. It never starts or stops recording.
    pub playing: bool,
    pub follow_live: bool,
    pub move_arm_on_release: bool,
}

/// One contiguous recorded portion of the camera lane.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineVideoSpan {
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// Optional local label for a clip or camera change.
    pub label: Option<String>,
}

/// The recorded frame nearest the shared episode playhead.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineVideoPreview {
    pub url: String,
    pub video_seconds: f64,
    pub episode_seconds: f64,
    pub label: String,
    pub approximate: bool,
}

/// Camera media aligned to the episode clock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimelineVideoLane {
    pub label: String,
    pub spans: Vec<TimelineVideoSpan>,
    pub preview: Option<TimelineVideoPreview>,
    /// Printed when there are no spans, so an absent recording never looks
    /// like a loading failure.
    pub empty_reason: Option<String>,
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

/// Convenience for hosts that only need the renderable six-lane model.
pub async fn load_recorded_signal_lanes(
    episode: &str,
    buckets: usize,
) -> Result<[TimelineSignalLane; 6], String> {
    load_recorded_timeline_traces(episode, buckets)
        .await
        .map(|response| response.signal_lanes())
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

/// The backend's answer to whether a seek may command hardware.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineMotionGate {
    pub allowed: bool,
    /// A concrete recovery path when motion is blocked.
    pub reason: Option<String>,
}

impl TimelineMotionGate {
    pub fn allowed() -> Self {
        Self {
            allowed: true,
            reason: None,
        }
    }

    pub fn blocked(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: Some(reason.into()),
        }
    }
}

/// A committed seek. `move_arm` can only be true after both the backend gate
/// and the operator-controlled toggle have passed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineSeekRequest {
    pub seconds: f64,
    pub move_arm: bool,
    /// False for a live scrub preview, true once the range control commits.
    /// Hardware motion is only legal for committed requests.
    pub committed: bool,
}

/// The canonical timeline visualization shared by Live and Data.
///
/// The component is controlled: the host owns playback, follow-live, and
/// motion-toggle state, and should feed each callback's requested value back
/// through [`StudioTimelineState`]. Scrubbing is held locally until the range
/// control commits, so physical motion is never requested for every pointer
/// move.
#[component]
pub fn StudioTimeline(
    state: StudioTimelineState,
    video: TimelineVideoLane,
    signal_lanes: [TimelineSignalLane; 6],
    motion_gate: TimelineMotionGate,
    on_play_pause: EventHandler<bool>,
    on_seek: EventHandler<TimelineSeekRequest>,
    on_follow_live: EventHandler<bool>,
    on_move_arm_on_release: EventHandler<bool>,
) -> Element {
    let recording = state.source.is_recording();
    let duration = state.source.duration_seconds();
    let external_playhead = if recording && state.follow_live {
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
    let episode_id = state.source.episode_id().to_owned();
    let frames = state.source.frames();
    let playing = state.playing;
    let follow_live = state.follow_live;
    let range_max = duration.max(0.001);
    let video_label = if video.label.is_empty() {
        "Video".to_owned()
    } else {
        video.label.clone()
    };
    let playhead_percent = percent(shown_playhead, duration);
    let move_arm = motion_gate.allowed && state.move_arm_on_release;
    let subject_label = if recording { "Recording" } else { "Episode" };
    let play_label = match (recording, state.playing) {
        (true, true) => "Pause recording",
        (true, false) => "Resume recording",
        (false, true) => "Pause episode",
        (false, false) => "Play episode",
    };
    let ticks = timeline_ticks(duration);
    let status = state.source.problem().map(str::to_owned);
    let source_available = state.source.available();

    rsx! {
        section {
            class: "studio-timeline flex min-h-0 flex-col border-t border-line bg-canvas text-ink",
            "aria-label": "Timeline for {episode_id}",

            header { class: "flex min-h-9 flex-wrap items-center gap-2 border-b border-line px-2 py-1",
                button {
                    r#type: "button",
                    class: "btn btn-outline inline-flex min-w-24 items-center gap-1",
                    disabled: !source_available,
                    "aria-label": play_label,
                    title: play_label,
                    onclick: move |_| on_play_pause.call(!playing),
                    PlaybackIcon { playing }
                    span { "{play_label}" }
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
                if recording {
                    button {
                        r#type: "button",
                        class: if follow_live { "btn btn-outline text-accent" } else { "btn btn-outline" },
                        "aria-pressed": follow_live,
                        title: if follow_live { "Stop following the newest recorded sample" } else { "Return the playhead to the newest recorded sample" },
                        onclick: move |_| on_follow_live.call(!follow_live),
                        if follow_live { "Following live" } else { "Follow live" }
                    }
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
                                key: "tick-{tick.seconds}",
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
                            disabled: duration <= 0.0,
                            "aria-label": "Episode playhead",
                            "aria-valuetext": "{format_clock(shown_playhead)} of {format_clock(duration)}",
                            oninput: move |event| {
                                if let Ok(seconds) = event.value().parse::<f64>() {
                                    scrubbing.set(true);
                                    let seconds = seconds.clamp(0.0, duration);
                                    draft_playhead.set(seconds);
                                    on_seek.call(TimelineSeekRequest {
                                        seconds,
                                        move_arm: false,
                                        committed: false,
                                    });
                                }
                            },
                            onchange: move |_| {
                                let seconds = draft_playhead().clamp(0.0, duration);
                                scrubbing.set(false);
                                if recording && follow_live && seconds + 0.025 < duration {
                                    on_follow_live.call(false);
                                }
                                on_seek.call(TimelineSeekRequest {
                                    seconds,
                                    move_arm,
                                    committed: true,
                                });
                            },
                            onpointercancel: move |_| {
                                scrubbing.set(false);
                                draft_playhead.set(external_playhead);
                            },
                        }
                    }

                    div { class: "flex items-center border-b border-r border-line px-2 text-[11px]",
                        span { class: "truncate", "{video_label}" }
                    }
                    VideoLaneTrack {
                        lane: video,
                        duration,
                        playhead_seconds: shown_playhead,
                        playing,
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

            footer { class: "flex min-h-8 flex-wrap items-center gap-x-2 gap-y-1 border-t border-line px-2 py-1",
                label {
                    class: if motion_gate.allowed { "flex items-center gap-1 text-[11px]" } else { "flex items-center gap-1 text-[11px] text-ink-faint" },
                    title: motion_gate.reason.as_deref().unwrap_or("Command the compatible replay arm after a committed seek"),
                    input {
                        r#type: "checkbox",
                        checked: move_arm,
                        disabled: !motion_gate.allowed,
                        onchange: move |event| on_move_arm_on_release.call(event.checked()),
                    }
                    span { "Move arm on release" }
                }
                if let Some(reason) = motion_gate.reason.as_deref() {
                    span { class: "text-[10px] text-ink-faint", "{reason}" }
                } else if move_arm {
                    span { class: "text-[10px] text-caution", "The compatible replay arm will move after the playhead is released." }
                } else {
                    span { class: "text-[10px] text-ink-faint", "Seeking changes the preview only." }
                }
            }
        }
    }
}

#[component]
fn VideoLaneTrack(
    lane: TimelineVideoLane,
    duration: f64,
    playhead_seconds: f64,
    playing: bool,
) -> Element {
    let preview_position = percent(playhead_seconds, duration);

    rsx! {
        div { class: "relative min-h-14 overflow-hidden border-b border-line bg-sunken/40",
            if lane.spans.is_empty() {
                span { class: "absolute inset-0 flex items-center px-2 text-[10px] text-ink-faint",
                    {lane.empty_reason.as_deref().unwrap_or("No recorded video")}
                }
            } else {
                for (index, span) in lane.spans.iter().enumerate() {
                    {
                        let start = percent(span.start_seconds, duration);
                        let end = percent(span.end_seconds, duration).max(start);
                        let width = (end - start).max(0.15);
                        rsx! {
                            div {
                                key: "video-{index}",
                                class: "absolute bottom-1 top-1 overflow-hidden rounded-sm border border-line bg-camera text-white",
                                style: "left: {start:.6}%; width: {width:.6}%",
                                title: span.label.as_deref().unwrap_or("Recorded camera video"),
                                div { class: "flex h-full items-start gap-1 px-1.5 pt-1",
                                    VideoIcon {}
                                    if let Some(label) = &span.label {
                                        span { class: "truncate text-[10px]", "{label}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(preview) = lane.preview {
                div {
                    class: "pointer-events-none absolute bottom-1 top-1 z-10 w-20 overflow-hidden rounded-sm border border-white/70 bg-camera shadow-sm",
                    style: "left: clamp(0px, calc({preview_position:.6}% - 2.5rem), calc(100% - 5rem));",
                    video {
                        key: "{preview.url}",
                        id: "timeline-camera-preview",
                        class: "h-full w-full object-cover",
                        src: "{preview.url}",
                        preload: "auto",
                        autoplay: playing,
                        muted: true,
                        playsinline: true,
                        onloadedmetadata: move |_| sync_timeline_video(
                            "timeline-camera-preview",
                            preview.video_seconds,
                            playing,
                        ),
                        "aria-label": if preview.approximate {
                            format!(
                                "Approximate recorded {} camera frame at {}",
                                preview.label,
                                format_clock(preview.episode_seconds),
                            )
                        } else {
                            format!(
                                "Recorded {} camera frame at {}",
                                preview.label,
                                format_clock(preview.episode_seconds),
                            )
                        },
                    }
                    span {
                        class: "absolute bottom-0 inset-x-0 truncate bg-black/60 px-1 py-0.5 text-[10px] text-white",
                        "{preview.label}"
                    }
                }
            }
        }
    }
}

fn sync_timeline_video(element_id: &str, video_seconds: f64, should_play: bool) {
    #[cfg(feature = "web")]
    {
        use wasm_bindgen::{JsCast as _, JsValue};

        let Some(element) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(element_id))
        else {
            return;
        };
        let target = video_seconds.max(0.0);
        let current = js_sys::Reflect::get(element.as_ref(), &JsValue::from_str("currentTime"))
            .ok()
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0);
        let threshold = if should_play { 0.25 } else { 0.005 };
        if (current - target).abs() > threshold {
            let _ = js_sys::Reflect::set(
                element.as_ref(),
                &JsValue::from_str("currentTime"),
                &JsValue::from_f64(target),
            );
        }
        let paused = js_sys::Reflect::get(element.as_ref(), &JsValue::from_str("paused"))
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(!should_play);
        let method = match (should_play, paused) {
            (true, true) => Some("play"),
            (false, false) => Some("pause"),
            _ => None,
        };
        if let Some(method) = method
            && let Ok(function) = js_sys::Reflect::get(element.as_ref(), &JsValue::from_str(method))
                .and_then(|value| value.dyn_into::<js_sys::Function>())
        {
            let _ = function.call0(element.as_ref());
        }
    }
    #[cfg(not(feature = "web"))]
    let _ = (element_id, video_seconds, should_play);
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

#[component]
fn VideoIcon() -> Element {
    rsx! {
        svg { class: "ui-icon shrink-0", view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "2.5", y: "3.5", width: "8", height: "9", rx: "1" }
            path { d: "m10.5 6 3-1.5v7l-3-1.5" }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct TimelineTick {
    seconds: f64,
    position: f64,
    label: String,
}

fn timeline_ticks(duration: f64) -> Vec<TimelineTick> {
    let duration = duration.max(0.0);
    (0..=4)
        .map(|index| {
            let fraction = f64::from(index) / 4.0;
            let seconds = duration * fraction;
            TimelineTick {
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
