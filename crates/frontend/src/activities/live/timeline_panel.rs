//! Live-specific recording controls and robot-sync policy around the shared timeline.

use crate::components::time_series::{TimelineSignalLane, TimelineSignalLanes};
use std::sync::Arc;

use crate::components::interaction::{
    Action, ActionPurpose, ActionSize, ActionVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, InlineConfirmation, Toggle,
};
use dioxus::prelude::*;

use crate::activities::live::timeline_subject::{TimelineSubject, TimelineTarget};
use crate::components::charts::CharmingChart;
use crate::format::ClockPresentation;

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
            playhead_seconds,
            playback,
            follow_live: false,
            sync_robot: false,
        }
    }

    pub(crate) const fn playhead_seconds(&self) -> f64 {
        self.playhead_seconds
    }

    pub(crate) const fn with_controls(mut self, follow_live: bool, sync_robot: bool) -> Self {
        self.follow_live = follow_live;
        self.sync_robot = sync_robot;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineCapabilities {
    record: Result<(), String>,
    seek: Result<(), String>,
    /// Whether full-episode playback may drive the compatible robot.
    synced_playback: Result<(), String>,
    /// Whether this Timeline subject may be armed to drive the robot.
    robot_sync: Result<(), String>,
}

impl TimelineCapabilities {
    pub(crate) fn new(record: Result<(), String>, seek: Result<(), String>) -> Self {
        Self {
            record,
            seek,
            synced_playback: Err("Robot playback is unavailable".into()),
            robot_sync: Err("Robot sync is unavailable".into()),
        }
    }

    pub(crate) fn with_robot_controls(
        mut self,
        synced_playback: Result<(), String>,
        robot_sync: Result<(), String>,
    ) -> Self {
        self.synced_playback = synced_playback;
        self.robot_sync = robot_sync;
        self
    }

    pub(crate) const fn robot_sync(&self) -> &Result<(), String> {
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

/// Signal content and capabilities projected by the Live activity.
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
pub(crate) fn LiveTimelinePanel(
    state: StudioTimelineState,
    visualization: TimelineVisualization,
    on_action: EventHandler<TimelineAction>,
) -> Element {
    use crate::components::time_series::TraceChartLayout;
    use crate::components::timeline::{
        PlaybackClock, SeekPhase, Timeline, TimelineAction as TimeAction, TimelineContent,
        TimelineEvent, TimelineInteraction, TimelineState, ViewWindow,
    };
    let TimelineVisualization {
        signal_lanes,
        signal_source,
        capabilities,
    } = visualization;
    let recording = state.subject.recording().cloned();
    let duration = state.subject.duration_seconds();
    let seconds = if recording.is_some() && state.follow_live {
        duration
    } else {
        state.playhead_seconds.clamp(0.0, duration)
    };
    let target = state.subject.target();
    let identity = format!("{target:?}");
    let mut window = use_signal(|| None::<ViewWindow>);
    let mut confirm_discard = use_signal(|| false);
    use_effect(use_reactive(&identity, move |_| {
        window.set(None);
        confirm_discard.set(false);
    }));
    let view = window().unwrap_or_else(|| ViewWindow::full(duration));
    let follow_live = state.follow_live;
    let sync_robot = capabilities.robot_sync.is_ok() && state.sync_robot;
    let playback = state.playback;
    let robot_sync_active = matches!(
        playback,
        TimelinePlayback::RobotSyncSeeking | TimelinePlayback::RobotSyncPlaying
    );
    let playing = robot_sync_active || playback == TimelinePlayback::PreviewPlaying;
    let transport_gate = match playback {
        TimelinePlayback::RobotSyncSeeking
        | TimelinePlayback::RobotSyncPlaying
        | TimelinePlayback::RobotSyncPaused => Ok(()),
        TimelinePlayback::Stopped if sync_robot => capabilities.synced_playback.clone(),
        _ => capabilities.seek.clone(),
    };
    let mut clock = PlaybackClock::new(duration);
    clock.set_playing(playing);
    clock.seek(seconds);
    let shared_state = TimelineState::new(identity.clone(), clock)
        .with_window(view)
        .with_external_playback()
        .with_blocked_seek(capabilities.seek.clone().err())
        .with_blocked_play(transport_gate.err());
    let preview_episode = state.subject.episode_id().map(str::to_owned);
    let seek_target = target.clone();
    let seek_episode = preview_episode.clone();
    let mut scrubbing = use_signal(|| false);
    let interaction = TimelineInteraction::new(
        shared_state,
        EventHandler::new(move |event: TimelineEvent| {
            if event.target() != identity {
                return;
            }
            match event.action() {
                TimeAction::SetPlaying(next) => {
                    if next == playing {
                        return;
                    }
                    if let Some(episode) = seek_episode.clone() {
                        match playback.transport_intent(sync_robot) {
                            TimelineTransportIntent::Preview(action) => {
                                on_action.call(TimelineAction::Preview { episode, action })
                            }
                            TimelineTransportIntent::RobotSync(action) => on_action
                                .call(TimelineAction::RobotSyncTransport { episode, action }),
                        }
                    }
                }
                TimeAction::Seek { seconds, phase } => {
                    if phase != SeekPhase::Cancel
                        && !scrubbing()
                        && robot_sync_active
                        && let Some(episode) = seek_episode.clone()
                    {
                        on_action.call(TimelineAction::RobotSyncTransport {
                            episode,
                            action: TimelineRobotSyncAction::Pause,
                        });
                    }
                    scrubbing.set(phase == SeekPhase::Preview);
                    if follow_live && seconds + 0.025 < duration {
                        on_action.call(TimelineAction::SetFollowLive(false));
                    }
                    if let Some(target) = seek_target.clone() {
                        on_action.call(TimelineAction::Seek(TimelineSeekRequest {
                            target,
                            seconds,
                            move_robot: sync_robot && phase == SeekPhase::Commit,
                            committed: phase == SeekPhase::Commit,
                        }));
                    }
                }
                TimeAction::SetWindow(next) => window.set(Some(next)),
                _ => {}
            }
        }),
    );
    let capture_episode = recording
        .as_ref()
        .map(|progress| progress.episode().to_owned());
    let capture_paused = recording.as_ref().is_some_and(|progress| progress.paused());
    let record_title = capabilities
        .record
        .clone()
        .err()
        .unwrap_or_else(|| "Start Teleop and record a new episode".into());
    let sync_title = capabilities.robot_sync.clone().err().unwrap_or_else(|| {
        "Keep the compatible robot synchronized while playing or seeking".into()
    });
    let sync_target = target;
    let toolbar = rsx! {
        if let Some(episode) = capture_episode.clone() {
            Action { purpose: ActionPurpose::Stop, variant: ActionVariant::Solid, size: ActionSize::Standard,
                class: "take-action take-action-live",
                onclick: { let episode = capture_episode.clone(); move |_| on_action.call(TimelineAction::Capture { episode: episode.clone(), action: TimelineCaptureAction::Stop }) }, "Stop" }
            Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Outline,
                onclick: move |_| on_action.call(TimelineAction::Capture { episode: Some(episode.clone()), action: if capture_paused { TimelineCaptureAction::Resume } else { TimelineCaptureAction::Pause } }),
                if capture_paused { "Resume recording" } else { "Pause recording" } }
            if confirm_discard() {
                InlineConfirmation {
                    id: "discard-take-confirmation", class: "timeline-discard-confirmation",
                    copy: ConfirmationCopy::new("Discard this take? The current recording will not be saved.", "Discard take", "Keep recording"),
                    response: ConfirmationResponse::new(ActionPurpose::Discard, {
                        let episode = capture_episode.clone();
                        move |_| { confirm_discard.set(false); on_action.call(TimelineAction::Capture { episode: episode.clone(), action: TimelineCaptureAction::Discard }); }
                    }, move |_| confirm_discard.set(false)),
                    dismissal: ConfirmationDismissal::new("discard-take"),
                }
            } else {
                Action { id: "discard-take", purpose: ActionPurpose::Discard, variant: ActionVariant::Link,
                    aria_label: "Discard this take without saving", onclick: move |_| confirm_discard.set(true), "Discard" }
            }
        } else {
            Action { purpose: ActionPurpose::Start, variant: ActionVariant::Solid, size: ActionSize::Standard,
                class: "take-action", disabled: capabilities.record.is_err(), title: "{record_title}",
                aria_label: if capabilities.record.is_ok() { "Record".to_owned() } else { format!("Record — {record_title}") },
                onclick: move |_| on_action.call(TimelineAction::Capture { episode: None, action: TimelineCaptureAction::Start }), "Record" }
        }
        Toggle { purpose: ActionPurpose::Sync, class: "timeline-sync-toggle", label: "Sync robot", title: "{sync_title}",
            checked: state.sync_robot, disabled: capabilities.robot_sync.is_err(),
            onchange: move |enabled| if let Some(target) = sync_target.clone() { on_action.call(TimelineAction::SetRobotSync { target, enabled }); } }
        if recording.is_some() {
            Action { purpose: ActionPurpose::Sync, variant: ActionVariant::Outline,
                class: if follow_live { "text-accent" } else { "" }, aria_pressed: follow_live,
                title: if follow_live { "Stop following the newest recorded sample" } else { "Return the playhead to the newest recorded sample" },
                onclick: move |_| on_action.call(TimelineAction::SetFollowLive(!follow_live)),
                if follow_live { "Following live" } else { "Follow live" } }
        }
    };
    let full_episode_id = state.subject.episode_id().map(str::to_owned);
    let identity_label = full_episode_id
        .as_deref()
        .map(|id| id.short_episode_id())
        .unwrap_or_else(|| "No episode".into());
    let frames = state.subject.frames();
    let identity = rsx! {
        strong { "{identity_label}" }
        if let Some(full) = full_episode_id { span { class: "truncate", title: "{frames} frames", "{full}" } }
    };
    let empty_reason = match &state.subject {
        TimelineSubject::Empty { reason, .. } => Some(reason.clone()),
        _ => None,
    };
    let status = if recording.is_some() {
        state.subject.problem().map(str::to_owned)
    } else {
        None
    };
    let notice = rsx! {
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


    };
    let signal_chart = signal_lanes
        .chart_with(&TraceChartLayout::sparkline(duration).with_window((view.start(), view.end())));
    let signal_chart_label = format!("Six robot joint signals over {}", duration.clock_label());
    let rows = rsx! {
        div { class: "timeline-signal-grid",
            div { class: "timeline-signal-labels",
                for lane in signal_lanes.iter() {
                    div { key: "{lane.id()}", class: "timeline-signal-label",
                        span { class: "timeline-signal-swatch", style: "background: {lane.color()}" }
                        span { class: "truncate", "{lane.label()}" }
                    }
                }
            }
            div { class: "timeline-signal-chart-stack",
                CharmingChart { id: TIMELINE_CHART_ID.to_owned(), chart: signal_chart,
                    presentation: crate::components::charts::ChartPresentation::new("timeline-signal-chart", signal_chart_label) }
                div { class: "timeline-signal-lane-overlays",
                    for lane in signal_lanes.iter() {
                        SignalLaneState { key: "{lane.id()}", lane: lane.clone(), source: signal_source.clone(), clocked: duration > 0.0 }
                    }
                }
            }
        }
    };
    let content = TimelineContent::new(rows)
        .with_identity(identity)
        .with_toolbar(toolbar)
        .with_notice(notice);
    rsx! { div { class: "studio-timeline", "aria-label": "Live Timeline", Timeline { interaction, content } } }
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

#[cfg(test)]
mod tests {
    use super::*;
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
mod episode_id_tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn generated_episode_ids_read_as_take_numbers() {
        assert_eq!("episode_000024".short_episode_id(), "#24");
        assert_eq!("imported-pass".short_episode_id(), "imported-pass");
    }
}
