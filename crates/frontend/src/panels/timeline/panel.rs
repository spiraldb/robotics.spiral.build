//! Controlled timeline rendering with explicit content, capabilities, and action callbacks.
//!
//! This component owns presentation and interaction intent only. Recording,
//! playback, seeking, and motor motion remain server-owned operations. In
//! particular, playback and a committed scrub can request physical motion only
//! when the caller passes an allowed robot-sync [`TimelineGate`] and the
//! operator has explicitly enabled “Sync robot”.

pub(crate) use crate::components::time_series::{
    TimelineSignalLane, TimelineSignalLanes, TimelineSignalPoint, TimelineSignalSeries,
};
use std::sync::Arc;

use crate::components::interaction::{
    Action, ActionPurpose, ActionSize, ActionVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, InlineConfirmation, Toggle,
};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_viewport::SO101_JOINTS;

use crate::activities::data::workbench::EpisodeIdPresentation;
use crate::components::charts::CharmingChart;
use crate::format::ClockPresentation;
use crate::panels::timeline::signals::{SignalNamePresentation, SignalStore};
use crate::panels::timeline::subject::{TimelineSubject, TimelineTarget};

const TIMELINE_CHART_ID: &str = "studio-timeline-signal-chart";

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
    subject: TimelineSubject,
    label: String,
    playhead_seconds: f64,
    playback: TimelinePlayback,
    follow_live: bool,
    sync_robot: bool,
}

impl StudioTimelineState {
    pub(crate) fn new(
        subject: TimelineSubject,
        playhead_seconds: f64,
        playback: TimelinePlayback,
    ) -> Self {
        Self {
            subject,
            label: "Timeline".to_owned(),
            playhead_seconds,
            playback,
            follow_live: false,
            sync_robot: false,
        }
    }

    pub(crate) const fn playhead_seconds(&self) -> f64 {
        self.playhead_seconds
    }

    pub(crate) fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub(crate) const fn with_controls(mut self, follow_live: bool, sync_robot: bool) -> Self {
        self.follow_live = follow_live;
        self.sync_robot = sync_robot;
        self
    }
}

/// Wire response from the bounded recorded trace endpoint.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct RecordedTimelineTraces {
    episode: String,
    samples: u64,
    buckets: usize,
    start_seconds: Option<f64>,
    end_seconds: Option<f64>,
    lanes: Vec<RecordedTimelineTraceLane>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct RecordedTimelineTraceLane {
    joint: String,
    series: Vec<RecordedTimelineTraceSeries>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct RecordedTimelineTraceSeries {
    arm: String,
    points: Vec<RecordedTimelineTracePoint>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct RecordedTimelineTracePoint {
    seconds: f64,
    value: Option<f64>,
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
                .find(|lane| lane.joint == definition.name())
                .map(|lane| {
                    lane.series
                        .iter()
                        .map(|series| {
                            TimelineSignalSeries::new(
                                series.arm.clone(),
                                series
                                    .points
                                    .iter()
                                    .map(|point| {
                                        TimelineSignalPoint::new(point.seconds, point.value)
                                    })
                                    .collect(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            TimelineSignalLane::new(
                definition.name(),
                definition.label(),
                definition.name().signal_color(),
            )
            .with_series(series)
        })
    }
}

/// Convert the studio's existing take signal store into the timeline's fixed
/// six-lane view model. Each lane overlays the arms that reported that joint.
pub(crate) trait RecordingTimeline {
    fn timeline_lanes(&self) -> [TimelineSignalLane; 6];
}

impl RecordingTimeline for SignalStore {
    fn timeline_lanes(&self) -> [TimelineSignalLane; 6] {
        std::array::from_fn(|index| {
            let definition = SO101_JOINTS[index];
            let series = self
                .arms()
                .iter()
                .filter_map(|arm| {
                    let joint = arm
                        .joints()
                        .iter()
                        .find(|joint| joint.name() == definition.name())?;
                    let points = self
                        .times()
                        .iter()
                        .zip(joint.values())
                        .map(|(seconds, value)| {
                            TimelineSignalPoint::new(f64::from(*seconds), value.map(f64::from))
                        })
                        .collect();
                    Some(TimelineSignalSeries::new(arm.id(), points))
                })
                .collect();

            TimelineSignalLane::new(
                definition.name(),
                definition.label(),
                definition.name().signal_color(),
            )
            .with_series(series)
        })
    }
}

/// Whether a Timeline action is currently available, with a concrete recovery
/// path when it is not.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineGate {
    allowed: bool,
    reason: Option<String>,
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

    pub(crate) const fn is_allowed(&self) -> bool {
        self.allowed
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineCapabilities {
    record: TimelineGate,
    preview: TimelineGate,
    seek: TimelineGate,
    /// Whether full-episode playback may drive the compatible robot.
    synced_playback: TimelineGate,
    /// Whether this Timeline subject may be armed to drive the robot.
    robot_sync: TimelineGate,
}

impl TimelineCapabilities {
    pub(crate) const fn new(
        record: TimelineGate,
        preview: TimelineGate,
        seek: TimelineGate,
    ) -> Self {
        Self {
            record,
            preview,
            seek,
            synced_playback: TimelineGate {
                allowed: false,
                reason: None,
            },
            robot_sync: TimelineGate {
                allowed: false,
                reason: None,
            },
        }
    }

    pub(crate) fn with_robot_controls(
        mut self,
        synced_playback: TimelineGate,
        robot_sync: TimelineGate,
    ) -> Self {
        self.synced_playback = synced_playback;
        self.robot_sync = robot_sync;
        self
    }

    pub(crate) const fn robot_sync(&self) -> &TimelineGate {
        &self.robot_sync
    }
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
    target: TimelineTarget,
    seconds: f64,
    move_robot: bool,
    committed: bool,
}

impl TimelineSeekRequest {
    pub(crate) const fn target(&self) -> &TimelineTarget {
        &self.target
    }

    pub(crate) const fn seconds(&self) -> f64 {
        self.seconds
    }

    pub(crate) const fn moves_robot(&self) -> bool {
        self.move_robot
    }

    pub(crate) const fn committed(&self) -> bool {
        self.committed
    }
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

impl TimelinePlayback {
    fn transport_intent(self, sync_robot: bool) -> TimelineTransportIntent {
        match self {
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
            TimelinePlayback::Stopped => {
                TimelineTransportIntent::Preview(TimelinePreviewAction::Play)
            }
        }
    }
}

/// Reusable timeline visualization configured by its owning activity.
///
/// The component is controlled: the activity controller owns the projection and
/// feeds requested actions back through [`StudioTimelineState`]. Scrubbing is
/// held locally until the range control commits, so physical motion is never
/// requested for every pointer move.
#[derive(Clone, PartialEq)]
pub(crate) struct TimelineVisualization {
    signal_lanes: Arc<[TimelineSignalLane; 6]>,
    signal_source: SignalSource,
    capabilities: TimelineCapabilities,
}

impl TimelineVisualization {
    pub(crate) const fn new(
        signal_lanes: Arc<[TimelineSignalLane; 6]>,
        signal_source: SignalSource,
        capabilities: TimelineCapabilities,
    ) -> Self {
        Self {
            signal_lanes,
            signal_source,
            capabilities,
        }
    }
}

#[component]
pub(crate) fn StudioTimeline(
    state: StudioTimelineState,
    visualization: TimelineVisualization,
    on_action: EventHandler<TimelineAction>,
) -> Element {
    let TimelineVisualization {
        signal_lanes,
        signal_source,
        capabilities,
    } = visualization;
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
    let capture_identity = recording
        .as_ref()
        .map(|progress| progress.episode().to_owned());
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
    let playhead_percent = shown_playhead.percent_of(duration);
    let sync_robot = capabilities.robot_sync.allowed && state.sync_robot;
    let subject_label = match &state.subject {
        TimelineSubject::Empty { label, .. } => label.as_str(),
        TimelineSubject::Recording(_) => "Recording",
        TimelineSubject::Episode { .. } => "Episode",
    };
    let identity_label = match full_episode_id.as_deref() {
        Some(id) => format!("{subject_label} {}", id.short_episode_id()),
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
    let ticks = duration.timeline_ticks();

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
    let next_transport = playback.transport_intent(sync_robot);
    let capture_episode = recording
        .as_ref()
        .map(|progress| progress.episode().to_owned());
    let capture_paused = recording.as_ref().is_some_and(|progress| progress.paused());
    let timeline_label = &state.label;
    let signal_chart = signal_lanes.chart(duration);
    let signal_chart_label = format!("Six robot joint signals over {}", duration.clock_label());

    rsx! {
        section {
            class: "studio-timeline flex min-h-0 flex-col border-t border-line bg-canvas text-ink",
            "aria-label": "{timeline_label} for {episode_id}",

            header { class: "flex min-h-9 flex-wrap items-center gap-2 border-b border-line px-2 py-1",
                if let Some(episode) = capture_episode.clone() {
                    // Stop is the one reached for with a hand still on the arm
                    // once a take is running, so it takes the 28px take action
                    // and the live red. Pause stays an ordinary 24px control and
                    // Discard sits behind a confirm at the far end of the row —
                    // the three used to be the same size, 8px apart, with the
                    // only unconfirmed one being the only unrecoverable one.
                    Action {
                        purpose: ActionPurpose::Stop,
                        variant: ActionVariant::Solid,
                        size: ActionSize::Standard,
                        class: "take-action take-action-live",
                        onclick: {
                            let episode = capture_episode.clone();
                            move |_| on_action.call(TimelineAction::Capture {
                                episode: episode.clone(),
                                action: TimelineCaptureAction::Stop,
                            })
                        },
                        "Stop"
                    }
                    Action {
                        purpose: ActionPurpose::Toggle,
                        variant: ActionVariant::Outline,
                        onclick: move |_| on_action.call(TimelineAction::Capture {
                            episode: Some(episode.clone()),
                            action: if capture_paused { TimelineCaptureAction::Resume } else { TimelineCaptureAction::Pause },
                        }),
                        if capture_paused { "Resume recording" } else { "Pause recording" }
                    }
                    if confirm_discard() {
                        InlineConfirmation {
                            id: "discard-take-confirmation",
                            class: "timeline-discard-confirmation",
                            copy: ConfirmationCopy::new(
                                "Discard this take? The current recording will not be saved.",
                                "Discard take",
                                "Keep recording",
                            ),
                            response: ConfirmationResponse::new(
                                ActionPurpose::Discard,
                                {
                                    let episode = capture_episode.clone();
                                    move |_| {
                                        confirm_discard.set(false);
                                        on_action.call(TimelineAction::Capture {
                                            episode: episode.clone(),
                                            action: TimelineCaptureAction::Discard,
                                        });
                                    }
                                },
                                move |_| confirm_discard.set(false),
                            ),
                            dismissal: ConfirmationDismissal::new("discard-take"),
                        }
                    } else {
                        div { class: "ml-1 flex items-center gap-1 border-l border-line pl-2",
                            Action {
                                id: "discard-take",
                                purpose: ActionPurpose::Discard,
                                variant: ActionVariant::Link,
                                aria_label: "Discard this take without saving",
                                onclick: move |_| confirm_discard.set(true),
                                "Discard"
                            }
                        }
                    }
                } else {
                    Action {
                        purpose: ActionPurpose::Start,
                        variant: ActionVariant::Solid,
                        size: ActionSize::Standard,
                        class: "take-action",
                        disabled: !capabilities.record.allowed,
                        title: "{record_title}",
                        aria_label: if capabilities.record.allowed { "Record".to_owned() } else { format!("Record — {record_title}") },
                        onclick: move |_| on_action.call(TimelineAction::Capture {
                            episode: None,
                            action: TimelineCaptureAction::Start,
                        }),
                        "Record"
                    }
                }

                Action {
                    purpose: ActionPurpose::Toggle,
                    variant: ActionVariant::Icon,
                    class: "timeline-transport-action",
                    disabled: !transport_allowed,
                    title: "{transport_title}",
                    aria_label: "{transport_label}",
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

                Toggle {
                    purpose: ActionPurpose::Sync,
                    class: "timeline-sync-toggle",
                    label: "Sync robot",
                    title: "{sync_title}",
                    checked: state.sync_robot,
                    disabled: !capabilities.robot_sync.allowed,
                    onchange: {
                        let target = sync_target.clone();
                        move |checked| if let Some(target) = target.clone() {
                                on_action.call(TimelineAction::SetRobotSync {
                                    target,
                                    enabled: checked,
                                });
                        }
                    },
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
                    span { class: "text-[12px] font-medium", "{shown_playhead.clock_label()}" }
                    span { class: "text-[10px] text-ink-faint", "/ {duration.clock_label()}" }
                    span { class: "ml-1 text-[10px] text-ink-faint", "{frames} frames" }
                }
                if recording.is_some() {
                    Action {
                        purpose: ActionPurpose::Sync,
                        variant: ActionVariant::Outline,
                        class: if follow_live { "text-accent" } else { "" },
                        aria_pressed: follow_live,
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
                    class: "timeline-source-note",
                    role: "alert",
                    span {
                        title: "Signals for {failed_episode} are unavailable. {reason}",
                        "Signals for {failed_episode} are unavailable. {reason}"
                    }
                    Action {
                        purpose: ActionPurpose::Retry,
                        variant: ActionVariant::Outline,
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
                                key: "label-{lane.id()}",
                                class: "timeline-signal-label",
                                span { class: "timeline-signal-swatch", style: "background: {lane.color()}" }
                                span { class: "truncate", "{lane.label()}" }
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
                                    if !tick.label.is_empty() {
                                        span { "{tick.label}" }
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
                                "aria-valuetext": "{shown_playhead.clock_label()} of {duration.clock_label()}",
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
                                presentation: crate::components::charts::ChartPresentation::new(
                                    "timeline-signal-chart",
                                    signal_chart_label,
                                ),
                            }
                            div { class: "timeline-signal-lane-overlays",
                                for lane in signal_lanes.iter() {
                                    SignalLaneState {
                                        key: "state-{lane.id()}",
                                        lane: lane.clone(),
                                        source: signal_source.clone(),
                                        clocked: duration > 0.0,
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

/// The per-lane verdict, drawn only once there is a clock for it to be about.
///
/// `clocked` is false when the panel has no recording and no selected episode.
/// Six lanes each reporting "No samples" for an episode that does not exist
/// restated the panel's own empty line six times, in the six places the eye
/// goes looking for data — absence stated that emphatically reads as failure.
/// Once an episode is on the clock, a lane that is genuinely missing its trace
/// is a real fact about that episode and says so.
#[component]
fn SignalLaneState(lane: TimelineSignalLane, source: SignalSource, clocked: bool) -> Element {
    let blank = lane.series().is_empty();
    let loading = blank && source.loading();
    let unavailable = blank && source.unavailable().is_some();
    // A request that is still in flight or has already failed is a verdict about
    // a real episode, so it is reported whether or not the clock has a span yet.
    // "No samples" is the only one of the three that needs an episode to be
    // about, which is why it alone waits for the clock.
    let missing = blank && clocked && !loading && !unavailable;
    let reported = loading || unavailable || missing;
    let label = if loading {
        format!("{} samples are loading", lane.label())
    } else if unavailable {
        format!("{} samples are unavailable", lane.label())
    } else if missing {
        format!("{} had no samples", lane.label())
    } else {
        String::new()
    };
    rsx! {
        div {
            class: "timeline-signal-lane-state",
            role: reported.then_some("status"),
            "aria-busy": if loading { "true" } else { "false" },
            "aria-label": (!label.is_empty()).then_some(label),
            "aria-hidden": (!reported).then_some("true"),
            if loading {
                // A resting placeholder, not a shimmer: the reduced-motion block
                // strips animation anyway, and a lane that pulses while the arm
                // is live reads as telemetry rather than as absence.
                span { class: "timeline-signal-loading" }
            } else if unavailable {
                span { class: "timeline-signal-unavailable", "Unavailable" }
            } else if missing {
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

/// The five divisions of the axis, labelled only once there is a span to label.
///
/// A clock with no episode on it has no scale, and stamping `0.0.clock_label()`
/// at every division printed `0:00.0` five times across the head of the panel —
/// a row that looks like a readout stuck at zero rather than an axis waiting for
/// an episode. The divisions themselves stay, so the lanes keep their grid and
/// every tick keeps the distinct identity its render key depends on.
trait TimelineSeconds {
    fn timeline_ticks(self) -> Vec<TimelineTick>;
    fn percent_of(self, duration: f64) -> f64;
}

impl TimelineSeconds for f64 {
    fn timeline_ticks(self) -> Vec<TimelineTick> {
        let duration = self.max(0.0);
        let scaled = self > 0.0;
        (0_u8..=4)
            .map(|index| {
                let fraction = f64::from(index) / 4.0;
                let seconds = duration * fraction;
                TimelineTick {
                    ordinal: index,
                    seconds,
                    position: fraction * 100.0,
                    label: if scaled {
                        seconds.clock_label()
                    } else {
                        String::new()
                    },
                }
            })
            .collect()
    }

    fn percent_of(self, duration: f64) -> f64 {
        if !self.is_finite() || !duration.is_finite() || duration <= 0.0 {
            0.0
        } else {
            (self / duration * 100.0).clamp(0.0, 100.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn percent_is_stable_for_invalid_duration() {
        assert_eq!(5.0_f64.percent_of(10.0), 50.0);
        assert_eq!(5.0_f64.percent_of(0.0), 0.0);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn one_transport_routes_through_the_robot_only_when_sync_is_enabled() {
        assert_eq!(
            TimelinePlayback::Stopped.transport_intent(false),
            TimelineTransportIntent::Preview(TimelinePreviewAction::Play)
        );
        assert_eq!(
            TimelinePlayback::Stopped.transport_intent(true),
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Start)
        );
        assert_eq!(
            TimelinePlayback::PreviewPlaying.transport_intent(false),
            TimelineTransportIntent::Preview(TimelinePreviewAction::Pause)
        );
        assert_eq!(
            TimelinePlayback::RobotSyncPlaying.transport_intent(true),
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Pause)
        );
        assert_eq!(
            TimelinePlayback::RobotSyncSeeking.transport_intent(true),
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Pause)
        );
        assert_eq!(
            TimelinePlayback::RobotSyncPaused.transport_intent(true),
            TimelineTransportIntent::RobotSync(TimelineRobotSyncAction::Resume)
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn empty_timeline_ticks_keep_unique_render_identities() {
        let ticks = 0.0_f64.timeline_ticks();
        let identities = ticks
            .iter()
            .map(|tick| tick.ordinal)
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(ticks.len(), 5);
        assert_eq!(identities.len(), ticks.len());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn an_unscaled_axis_is_unlabelled_and_a_scaled_one_is_not() {
        assert!(
            0.0_f64
                .timeline_ticks()
                .iter()
                .all(|tick| tick.label.is_empty()),
            "an axis with no episode on it has no times to print"
        );

        let ticks = 8.0_f64.timeline_ticks();
        assert!(ticks.iter().all(|tick| !tick.label.is_empty()));
        assert_eq!(ticks.last().map(|tick| tick.seconds), Some(8.0));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn timeline_chart_uses_one_grid_per_canonical_lane() {
        let chart = SignalStore::default().timeline_lanes().chart(12.0);
        let option = serde_json::to_value(chart).expect("timeline chart should serialize");

        assert_eq!(option["grid"].as_array().map(Vec::len), Some(6));
        assert_eq!(option["xAxis"].as_array().map(Vec::len), Some(6));
        assert_eq!(option["yAxis"].as_array().map(Vec::len), Some(6));
        assert_eq!(option["animation"], false);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn canonical_lane_helper_always_returns_six_joint_slots() {
        let lanes = SignalStore::default().timeline_lanes();
        assert_eq!(lanes.len(), 6);
        assert_eq!(lanes[0].id(), "shoulder_pan");
        assert_eq!(lanes[5].id(), "gripper");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
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
        assert!(lanes[0].series().is_empty());
        assert_eq!(lanes[5].id(), "gripper");
        assert_eq!(lanes[5].series()[0].name(), "follower");
        assert_eq!(lanes[5].series()[0].points()[1].value(), None);
    }
}
