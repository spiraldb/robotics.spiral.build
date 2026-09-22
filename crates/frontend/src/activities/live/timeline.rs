//! Live's Timeline: what the shared timeline is showing, and the recording
//! controls that only Live has.
//!
//! The clock, ruler, scrubber, and keyboard transport are
//! [`crate::components::timeline`]'s. This module supplies its subject, its
//! identity and toolbar slots, and the six signal lanes drawn as its rows.

use std::sync::Arc;

use dioxus::prelude::*;
use spiral_robotics_common::recording::RecordingProgress;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, InlineConfirmation,
};

use super::transport::LiveTransport;
use crate::components::charts::{CharmingChart, ChartPresentation};
use crate::components::time_series::{TimelineSignalLane, TimelineSignalLanes, TraceChartLayout};
use crate::components::timeline::{
    PlaybackClock, Timeline, TimelineAction, TimelineContent, TimelineEvent, TimelineInteraction,
    TimelineState, ViewWindow,
};
use crate::format::ClockPresentation;

const TIMELINE_CHART_ID: &str = "studio-timeline-signal-chart";

/// The semantic subject of the canonical Timeline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TimelineSubject {
    Empty { reason: String },
    Recording(RecordingProgress),
}

impl TimelineSubject {
    pub(super) fn empty(reason: impl Into<String>) -> Self {
        Self::Empty {
            reason: reason.into(),
        }
    }

    /// The take on the clock; its identity is stable across pause and resume.
    pub(super) fn episode_id(&self) -> Option<&str> {
        self.recording().map(RecordingProgress::episode)
    }

    pub(super) fn duration_seconds(&self) -> f64 {
        self.recording()
            .map_or(0.0, RecordingProgress::elapsed_seconds)
            .max(0.0)
    }

    fn frames(&self) -> u64 {
        self.recording().map_or(0, RecordingProgress::frames)
    }

    pub(super) fn recording(&self) -> Option<&RecordingProgress> {
        match self {
            Self::Recording(progress) => Some(progress),
            Self::Empty { .. } => None,
        }
    }

    pub(super) fn problem(&self) -> Option<&str> {
        match self {
            Self::Empty { reason, .. } => Some(reason),
            Self::Recording(progress) => progress.error(),
        }
    }

    /// Why the recorded frames cannot be previewed or scrubbed, if they cannot.
    pub(super) fn preview_blocker(&self) -> Option<String> {
        match self.recording() {
            Some(progress) if progress.paused() => progress.error().map(str::to_owned),
            _ => Some("Pause the current recording to preview it.".to_owned()),
        }
    }

    /// The episode whose recorded data should be loaded for the current view.
    pub(super) fn recorded_episode(&self) -> Option<&str> {
        self.recording()
            .filter(|progress| progress.paused())
            .map(RecordingProgress::episode)
    }
}

/// Explicit loading and failure states for Live signal lanes.
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

/// The six joint traces drawn as the Timeline's rows, and where they came from.
#[derive(Clone, PartialEq)]
pub(super) struct TimelineSignals {
    lanes: Arc<[TimelineSignalLane; 6]>,
    source: SignalSource,
}

impl TimelineSignals {
    pub(super) const fn new(lanes: Arc<[TimelineSignalLane; 6]>, source: SignalSource) -> Self {
        Self { lanes, source }
    }
}

#[component]
pub(super) fn LiveTimelinePanel(transport: LiveTransport, signals: TimelineSignals) -> Element {
    let TimelineSignals { lanes, source } = signals;
    let subject = transport.subject();
    let recording = subject.recording().cloned();
    let duration = subject.duration_seconds();
    let seconds = if recording.is_some() && transport.follow_live() {
        duration
    } else {
        transport.seconds().clamp(0.0, duration)
    };
    let identity = format!("{:?}", subject.episode_id());
    let mut window = use_signal(|| None::<ViewWindow>);
    let mut confirm_discard = use_signal(|| false);
    use_effect(use_reactive(&identity, move |_| {
        window.set(None);
        confirm_discard.set(false);
    }));
    let view = window().unwrap_or_else(|| ViewWindow::full(duration));
    let mut clock = PlaybackClock::new(duration);
    clock.set_playing(transport.playing());
    clock.seek(seconds);
    let shared_state = TimelineState::new(identity.clone(), clock)
        .with_window(view)
        .with_external_playback()
        .with_blocked_seek(transport.seek_blocker())
        .with_blocked_play(transport.seek_blocker());
    let interaction = TimelineInteraction::new(shared_state, {
        let transport = transport.clone();
        EventHandler::new(move |event: TimelineEvent| {
            if event.target() != identity {
                return;
            }
            match event.action() {
                TimelineAction::SetPlaying(next) => {
                    if next != transport.playing() {
                        transport.press_play();
                    }
                }
                TimelineAction::Seek { seconds, phase } => {
                    if transport.follow_live() && seconds + 0.025 < duration {
                        transport.set_follow_live(false);
                    }
                    transport.seek(seconds, phase);
                }
                TimelineAction::SetWindow(next) => window.set(Some(next)),
                _ => {}
            }
        })
    });
    let content = TimelineContent::new(rsx! {
        SignalRows { lanes: lanes.clone(), source: source.clone(), duration, view }
    })
    .with_identity(rsx! { TimelineIdentity { subject: subject.clone() } })
    .with_toolbar(rsx! {
        CaptureControls { transport: transport.clone(), recording: recording.clone(), confirm_discard }
        if recording.is_some() {
            FollowLiveControl { transport: transport.clone() }
        }
    })
    .with_notice(rsx! { TimelineNotice { transport: transport.clone(), source } });
    rsx! { div { class: "studio-timeline", "aria-label": "Live Timeline", Timeline { interaction, content } } }
}

/// The take being recorded, or the control that starts one.
#[component]
fn CaptureControls(
    transport: LiveTransport,
    recording: Option<RecordingProgress>,
    confirm_discard: Signal<bool>,
) -> Element {
    let mut confirm_discard = confirm_discard;
    let Some(recording) = recording else {
        let blocker = transport.record_blocker();
        let title = blocker
            .clone()
            .unwrap_or_else(|| "Start Teleop and record a new episode".into());
        return rsx! {
            Button {
                purpose: ActionPurpose::Start, variant: ButtonVariant::Solid, size: ButtonSize::Standard,
                class: "take-action", disabled: blocker.is_some(), title: "{title}",
                aria_label: match blocker { Some(reason) => format!("Record — {reason}"), None => "Record".to_owned() },
                onclick: move |_| transport.start_capture(),
                "Record"
            }
        };
    };
    let paused = recording.paused();
    rsx! {
        Button {
            purpose: ActionPurpose::Stop, variant: ButtonVariant::Solid, size: ButtonSize::Standard,
            class: "take-action take-action-live",
            onclick: { let transport = transport.clone(); move |_| transport.finish_capture(true) },
            "Stop"
        }
        Button {
            purpose: ActionPurpose::Toggle, variant: ButtonVariant::Outline,
            onclick: { let transport = transport.clone(); move |_| transport.set_capture_paused(!paused) },
            if paused { "Resume recording" } else { "Pause recording" }
        }
        if confirm_discard() {
            InlineConfirmation {
                id: "discard-take-confirmation", class: "timeline-discard-confirmation",
                copy: ConfirmationCopy::new("Discard this take? The current recording will not be saved.", "Discard take", "Keep recording"),
                response: ConfirmationResponse::new(ActionPurpose::Discard, move |_| {
                    confirm_discard.set(false);
                    transport.finish_capture(false);
                }, move |_| confirm_discard.set(false)),
                dismissal: ConfirmationDismissal::new("discard-take"),
            }
        } else {
            Button {
                id: "discard-take", purpose: ActionPurpose::Discard, variant: ButtonVariant::Link,
                aria_label: "Discard this take without saving", onclick: move |_| confirm_discard.set(true),
                "Discard"
            }
        }
    }
}

/// Returning the playhead to the newest recorded sample.
#[component]
fn FollowLiveControl(transport: LiveTransport) -> Element {
    let follow_live = transport.follow_live();
    rsx! {
        Button {
            purpose: ActionPurpose::Sync, variant: ButtonVariant::Outline,
            class: if follow_live { "text-accent" } else { "" }, aria_pressed: follow_live,
            title: if follow_live { "Stop following the newest recorded sample" } else { "Return the playhead to the newest recorded sample" },
            onclick: move |_| transport.set_follow_live(!follow_live),
            if follow_live { "Following live" } else { "Follow live" }
        }
    }
}

/// The take number, with the full episode id beside it.
#[component]
fn TimelineIdentity(subject: TimelineSubject) -> Element {
    let full = subject.episode_id().map(str::to_owned);
    let label = full
        .as_deref()
        .map(|id| id.short_episode_id())
        .unwrap_or_else(|| "No episode".into());
    let frames = subject.frames();
    rsx! {
        strong { "{label}" }
        if let Some(full) = full { span { class: "truncate", title: "{frames} frames", "{full}" } }
    }
}

/// Why this Timeline is empty, in trouble, or drawing nothing.
#[component]
fn TimelineNotice(transport: LiveTransport, source: SignalSource) -> Element {
    let subject = transport.subject();
    let empty_reason = match subject {
        TimelineSubject::Empty { reason, .. } => Some(reason.clone()),
        _ => None,
    };
    let problem = subject
        .recording()
        .and_then(|_| subject.problem())
        .map(str::to_owned);
    rsx! {
        if let Some(reason) = empty_reason {
            p { class: "m-0 border-b border-line bg-sunken px-2 py-1 text-[11px] text-ink-soft", role: "status",
                "{reason}"
            }
        }
        if let Some(problem) = problem {
            p { class: "m-0 border-b border-line bg-danger-soft px-2 py-1 text-[11px] text-danger", role: "alert",
                "{problem}"
            }
        }
        if let Some((failed_episode, reason)) = source.unavailable() {
            div { class: "timeline-source-note", role: "alert",
                span {
                    title: "Signals for {failed_episode} are unavailable. {reason}",
                    "Signals for {failed_episode} are unavailable. {reason}"
                }
                Button {
                    purpose: ActionPurpose::Retry,
                    variant: ButtonVariant::Outline,
                    onclick: move |_| transport.reload_recorded(),
                    "Retry"
                }
            }
        }
    }
}

/// Six labelled joint traces sharing the Timeline's time window.
#[component]
fn SignalRows(
    lanes: Arc<[TimelineSignalLane; 6]>,
    source: SignalSource,
    duration: f64,
    view: ViewWindow,
) -> Element {
    let chart = lanes
        .chart_with(&TraceChartLayout::sparkline(duration).with_window((view.start(), view.end())));
    let label = format!("Six robot joint signals over {}", duration.clock_label());
    rsx! {
        div { class: "timeline-signal-grid",
            div { class: "timeline-signal-labels",
                for lane in lanes.iter() {
                    div { key: "{lane.id()}", class: "timeline-signal-label",
                        span { class: "timeline-signal-swatch", style: "background: {lane.color()}" }
                        span { class: "truncate", "{lane.label()}" }
                    }
                }
            }
            div { class: "timeline-signal-chart-stack",
                CharmingChart { id: TIMELINE_CHART_ID.to_owned(), chart,
                    presentation: ChartPresentation::new("timeline-signal-chart", label) }
                div { class: "timeline-signal-lane-overlays",
                    for lane in lanes.iter() {
                        SignalLaneState { key: "{lane.id()}", lane: lane.clone(), source: source.clone(), clocked: duration > 0.0 }
                    }
                }
            }
        }
    }
}

/// Per-lane availability; an empty workspace has no missing-sample verdict.
#[component]
fn SignalLaneState(lane: TimelineSignalLane, source: SignalSource, clocked: bool) -> Element {
    let blank = lane.series().is_empty();
    let loading = blank && source.loading();
    let unavailable = blank && source.unavailable().is_some();
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
                span { class: "timeline-signal-loading" }
            } else if unavailable {
                span { class: "timeline-signal-unavailable", "Unavailable" }
            } else if missing {
                span { class: "timeline-signal-empty", "No samples" }
            }
        }
    }
}

trait EpisodeIdPresentation {
    fn short_episode_id(&self) -> String;
}

impl EpisodeIdPresentation for str {
    fn short_episode_id(&self) -> String {
        self.strip_prefix("episode_")
            .and_then(|number| number.parse::<u64>().ok())
            .map(|number| format!("#{number}"))
            .unwrap_or_else(|| self.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::recording::{RecordingClock, RecordingIdentity, RecordingState};

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn capture_pause_and_resume_change_media_source_without_changing_transport_identity() {
        let recording = |paused| {
            TimelineSubject::Recording(RecordingProgress::new(
                RecordingIdentity::new("take-1", "default", vec!["follower".to_owned()]),
                RecordingClock::new(60, 1.0, 2.0),
                RecordingState::new(paused, None, None),
            ))
        };
        let active = recording(false);
        let paused = recording(true);
        assert_eq!(active.episode_id(), paused.episode_id());
        assert_eq!(active.recorded_episode(), None);
        assert_eq!(paused.recorded_episode(), Some("take-1"));
        assert_eq!(paused.duration_seconds(), 1.0);
        assert_eq!(paused.frames(), 60);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn an_empty_timeline_has_nothing_to_preview() {
        let subject = TimelineSubject::empty("Select a recording.");
        assert!(subject.episode_id().is_none());
        assert!(subject.recorded_episode().is_none());
        assert!(subject.preview_blocker().is_some());
        assert_eq!(subject.problem(), Some("Select a recording."));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn generated_episode_ids_read_as_take_numbers() {
        assert_eq!("episode_000024".short_episode_id(), "#24");
        assert_eq!("imported-pass".short_episode_id(), "imported-pass");
    }
}
