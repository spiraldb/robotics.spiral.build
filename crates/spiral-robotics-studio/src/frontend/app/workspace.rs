//! Reactive assembly of the persisted frontend panel workspace.

use dioxus::prelude::*;
use spiral_robotics_contracts::api::{ModeSelection, ReplayPhase, ReplayPurpose};
use spiral_robotics_panels::{DockZone, Panel, PanelId, PanelLayout, PanelWorkspace};
#[cfg(test)]
use spiral_robotics_panels::{LayoutNode, SplitAxis};

use crate::frontend::api::{commands, historical};
#[cfg(test)]
use crate::frontend::app::activity::LoadState;
use crate::frontend::app::activity::{
    Activity, ActivityProjection, CameraIdentity, CameraRegistrySource, PanelRole,
    PrimaryVisualization, RecordedSource, TimelineSubject, TimelineTarget,
};
#[cfg(test)]
use crate::frontend::app::layout::default_panel_layout;
use crate::frontend::app::routes::studio_configuration;
use crate::frontend::app::state::Studio;
use crate::frontend::panels::camera::{
    CameraEditor, CameraEditorSource, recorded_camera_registry_placeholder,
    recorded_camera_source_for_live_assignment, recorded_camera_source_for_stream,
    recorded_camera_streams, recorded_scene_source,
};
#[cfg(test)]
use crate::frontend::panels::camera::{historical_camera, historical_camera_at};
use crate::frontend::panels::timeline::panel::{
    RecordedTimelineTraces, SignalSource, StudioTimeline, StudioTimelineState,
    TimelineCapabilities, TimelineGate, TimelinePlayback, signal_lanes_from_store,
};
use crate::frontend::panels::timeline::state::{TimelineActionContext, dispatch_timeline_action};
use crate::frontend::screens::data::workbench::{
    EpisodesPanel, QueryPanel, QueryResultView, short_id,
};
use crate::frontend::screens::live::actions::dispatch_live_action;
use crate::frontend::screens::live::workbench::LiveWorkbench;
use crate::frontend::screens::rollout::state::rollout_controller;
use crate::frontend::screens::rollout::workspace::{
    InferencePanel, RolloutWorkbench, follower as rollout_follower,
    selected_checkpoint_camera_roles,
};

use historical::{HistoricalPreview, historical_replay_slot, request_historical_preview};
#[cfg(test)]
use historical::{attach_historical_trace_failure, attach_historical_traces};

#[derive(Clone, PartialEq)]
pub(crate) struct ScreenWorkspaceDefinition {
    panels: Vec<(Panel, PanelRole)>,
    pub(crate) initial_layout: PanelLayout,
    layout_scope: String,
}

impl ScreenWorkspaceDefinition {
    pub(crate) fn new(
        panels: Vec<(Panel, PanelRole)>,
        initial_layout: PanelLayout,
        layout_scope: impl Into<String>,
    ) -> Self {
        Self {
            panels,
            initial_layout,
            layout_scope: layout_scope.into(),
        }
    }
}

/// Shared episode-clock behavior for the routes that need robot, camera, and
/// timeline evidence. Each route owns its panel registry and layout; this host
/// owns only the cross-panel synchronization contract.
#[component]
pub(crate) fn EpisodeWorkspace(activity: Activity) -> Element {
    let studio = crate::frontend::app::state::studio();
    let configuration = studio_configuration();
    let dataset = configuration.dataset;
    let selected_dataset = studio.selected_dataset.read().clone();
    let spiral_query_available = configuration.spiral_query_available;
    let timeline = crate::frontend::panels::timeline::state::timeline_controller();
    let mut playhead = timeline.playhead;
    #[cfg(feature = "web")]
    let mut sampled_preview_slot = use_signal(|| None::<(String, u64)>);
    #[cfg(feature = "web")]
    let mut preview_tick_seconds = timeline.preview_tick_seconds;
    let follow_live = timeline.follow_live;
    let mut robot_sync_target = timeline.robot_sync_target;
    let mut historical_state = use_signal(|| None::<RecordedSource<HistoricalPreview>>);
    let mut historical_revision = use_signal(|| 0_u64);
    let mut sampled_replay_slot = use_signal(|| None::<(String, u64)>);
    let mut automatic_preview_target = use_signal(|| None::<(String, u64)>);
    let mut interactive_preview_target = use_signal(|| None::<(String, u64)>);
    let mut split_resize_revision = use_signal(|| 0_u64);

    // Visual preview is a controller-owned clock. It advances the one playhead
    // and samples every recorded renderer from the same episode/time identity.
    // Robot synchronization uses the same user-facing transport, while its
    // motion and progress remain a separate server-owned consequence.
    #[cfg(feature = "web")]
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(33).await;
            if !timeline.preview_playing() {
                preview_tick_seconds.set(None);
                continue;
            }
            let Some((episode, duration)) = timeline.preview_target() else {
                timeline.stop_preview();
                continue;
            };
            let now = js_sys::Date::now() / 1_000.0;
            let delta = preview_tick_seconds
                .replace(Some(now))
                .map(|previous| (now - previous).clamp(0.0, 0.25))
                .unwrap_or_default();
            let next = (playhead() + delta).clamp(0.0, duration.max(0.0));
            playhead.set(next);
            let slot = historical_replay_slot(next);
            let target = (episode.clone(), slot);
            if sampled_preview_slot.peek().as_ref() != Some(&target) {
                sampled_preview_slot.set(Some(target));
                request_historical_preview(
                    studio,
                    episode.clone(),
                    next,
                    historical_state,
                    historical_revision,
                );
            }
            if next >= duration {
                timeline.stop_preview();
                preview_tick_seconds.set(None);
            }
        }
    });

    // Replay telemetry may arrive at the controller rate. Historical scene
    // samples are bounded to a smooth visual cadence so the canvas receives
    // enough distinct poses without issuing duplicate requests for the same
    // display frame; browser-native video runs on its own media clock.
    // Physical scrub seeks deliberately stay owned by the explicit timeline
    // request instead of replacing it with replay telemetry. Replay purpose is
    // explicit because a valid timeline replay can contain only one frame.
    use_effect(move || {
        let Some(replay) = studio.replay().filter(replay_owns_timeline_transport) else {
            sampled_replay_slot.set(None);
            return;
        };
        let subject_episode = visible_timeline_episode(studio, activity);
        if !replay_matches_timeline_subject(subject_episode.as_deref(), &replay) {
            sampled_replay_slot.set(None);
            return;
        }
        let slot = historical_replay_slot(replay.elapsed_seconds);
        let next = (replay.episode.clone(), slot);
        if sampled_replay_slot.peek().as_ref() == Some(&next) {
            return;
        }
        sampled_replay_slot.set(Some(next));
        request_historical_preview(
            studio,
            replay.episode,
            replay.elapsed_seconds,
            historical_state,
            historical_revision,
        );
    });

    // Server progress is authoritative. The UI-owned playhead only takes over
    // after the operator leaves the live edge or reviews a completed episode.
    use_effect(move || {
        if let Some(recording) = studio.recording() {
            if follow_live() && !recording.paused {
                playhead.set(recording.elapsed_seconds);
            }
            if !recording.paused {
                timeline.stop_preview();
            }
        } else if let Some(replay) = studio.replay().filter(replay_owns_timeline_transport) {
            let subject_episode = visible_timeline_episode(studio, activity);
            if replay_matches_timeline_subject(subject_episode.as_deref(), &replay) {
                playhead.set(replay.elapsed_seconds);
                timeline.stop_preview();
            }
        }
    });

    // Resuming capture returns every visualization to the live edge. A stale
    // historical pose must never remain on screen while Teleop is active.
    use_effect(move || {
        if studio
            .recording()
            .is_some_and(|recording| !recording.paused)
            && historical_state.peek().is_some()
        {
            historical_state.set(None);
            let next_revision = historical_revision.peek().wrapping_add(1);
            historical_revision.set(next_revision);
        }
    });

    // With no active recording, Live always presents current hardware rather
    // than retaining a historical frame from a completed take.
    use_effect(move || {
        if activity == Activity::Live
            && studio.recording().is_none()
            && historical_state.peek().is_some()
        {
            historical_state.set(None);
            let next_revision = historical_revision.peek().wrapping_add(1);
            historical_revision.set(next_revision);
        }
    });

    // A paused take remains inspectable in Live. Completed recordings return
    // to the live source and stay available from Data instead.
    use_effect(move || {
        let (episode, seconds) = match studio.recording() {
            Some(recording) if recording.paused => (recording.episode, recording.elapsed_seconds),
            Some(_) => {
                if automatic_preview_target.peek().is_some() {
                    automatic_preview_target.set(None);
                }
                return;
            }
            None => return,
        };
        let target = (episode.clone(), (seconds.max(0.0) * 1_000.0).round() as u64);
        if automatic_preview_target.peek().as_ref() == Some(&target) {
            return;
        }
        automatic_preview_target.set(Some(target));
        if historical_state
            .peek()
            .as_ref()
            .is_some_and(|preview| preview.episode == episode)
        {
            return;
        }
        request_historical_preview(
            studio,
            episode,
            seconds,
            historical_state,
            historical_revision,
        );
    });

    let data_episode_context = studio.focused_episode();
    let projection = ActivityProjection::derive(activity, studio.recording(), data_episode_context);
    let projected_recorded_identity = match &projection.timeline {
        TimelineSubject::Recording(recording) if recording.paused => Some((
            recording.episode.clone(),
            recording.frames,
            recording.elapsed_seconds,
        )),
        TimelineSubject::Episode { episode, .. } => Some((episode.id.clone(), episode.frames, 0.0)),
        TimelineSubject::Empty { .. } | TimelineSubject::Recording(_) => None,
    };
    use_effect(use_reactive(
        (&projected_recorded_identity,),
        move |(identity,)| {
            let Some((episode, _source_frames, seconds)) = identity else {
                return;
            };
            // The stable episode ID survives checkpoints and finalization.
            // Treat a new persisted frame count as a new source even when the
            // selected episode itself did not change.
            let next_revision = historical_revision.peek().wrapping_add(1);
            historical_revision.set(next_revision);
            historical_state.set(None);
            playhead.set(seconds);
            request_historical_preview(
                studio,
                episode,
                seconds,
                historical_state,
                historical_revision,
            );
        },
    ));
    let subject = projection.timeline.clone();
    let live_signal_lanes = {
        let store = studio.recording_signals.read();
        signal_lanes_from_store(&store)
    };
    let assigned_cameras = assigned_cameras(projection.activity);
    let review_episode = subject.recorded_episode().map(str::to_owned);
    let historical_source = historical_state
        .read()
        .clone()
        .filter(|source| Some(&source.episode) == review_episode.as_ref());
    let historical = historical_source
        .as_ref()
        .and_then(RecordedSource::renderable)
        .cloned();
    let history_active = review_episode.is_some();
    let signal_lanes = match historical.as_ref() {
        Some(preview) => preview
            .signal_lanes
            .clone()
            .unwrap_or_else(|| RecordedTimelineTraces::default().signal_lanes()),
        None if history_active => RecordedTimelineTraces::default().signal_lanes(),
        None => live_signal_lanes,
    };
    // The lanes above cannot tell absence from delay, so the reason travels
    // beside them. Empty lanes only mean "this episode had no samples" once the
    // trace request has actually come back.
    let signal_source = match review_episode.as_ref() {
        None => SignalSource::Live,
        Some(episode) => {
            let failure = historical
                .as_ref()
                .and_then(|preview| preview.signal_lanes_error.clone())
                .or_else(|| {
                    historical_source
                        .as_ref()
                        .and_then(|source| source.state.reason().map(str::to_owned))
                });
            match failure {
                Some(reason) => SignalSource::RecordedUnavailable {
                    episode: episode.clone(),
                    reason,
                },
                None if historical
                    .as_ref()
                    .is_some_and(|preview| preview.signal_lanes.is_some()) =>
                {
                    SignalSource::RecordedReady
                }
                None => SignalSource::RecordedLoading,
            }
        }
    };
    let timeline_target = subject.target();
    // Two `None`s compare equal, so with no episode on the clock this read as
    // "armed" and the physical-motion toggle rendered checked at rest. Nothing
    // is armed until there is a target to arm it against.
    let requested_robot_sync =
        timeline_target.is_some() && robot_sync_target.read().as_ref() == timeline_target.as_ref();
    let capabilities = timeline_capabilities(studio, projection.activity, &subject);
    let seek_context = timeline_seek_context(studio, &subject);
    let playback = studio
        .replay()
        .filter(replay_owns_timeline_transport)
        .and_then(|replay| {
            (subject.episode_id() == Some(replay.episode.as_str())).then_some(if replay.paused {
                TimelinePlayback::RobotSyncPaused
            } else if replay.phase == ReplayPhase::Seeking {
                TimelinePlayback::RobotSyncSeeking
            } else {
                TimelinePlayback::RobotSyncPlaying
            })
        })
        .unwrap_or_else(|| {
            if timeline.preview_playing() {
                TimelinePlayback::PreviewPlaying
            } else {
                TimelinePlayback::Stopped
            }
        });
    let robot_sync = requested_robot_sync;
    let disarm_robot_sync = requested_robot_sync && !capabilities.robot_sync.allowed;
    use_effect(use_reactive(
        (&timeline_target, &disarm_robot_sync),
        move |(current_target, must_disarm)| {
            let armed = robot_sync_target.peek().clone();
            let target_changed = armed.is_some() && armed.as_ref() != current_target.as_ref();
            if !target_changed && !must_disarm {
                return;
            }
            robot_sync_target.set(None);
            let armed_episode = armed.as_ref().map(timeline_target_episode);
            if let Some(replay) = studio.replay()
                && armed_episode == Some(replay.episode.as_str())
            {
                commands::stop_replay(studio, &replay.episode);
            }
        },
    ));
    let mut selected = studio.selected;
    let scene_robot = (projection.activity == Activity::Rollout)
        .then(|| rollout_follower(&studio.snapshot.read().robots).map(|robot| robot.id))
        .flatten();

    let mut dock_panels = Vec::<(Panel, PanelRole)>::new();
    match &projection.primary {
        PrimaryVisualization::LiveScene | PrimaryVisualization::RecordedScene(_) => {
            let scene_source = recorded_scene_source(&subject, historical_source.as_ref());
            let title = match &projection.primary {
                PrimaryVisualization::RecordedScene(episode)
                    if projection.activity == Activity::Data =>
                {
                    format!("Episode · {}", short_id(&episode.id))
                }
                _ => "Scene".to_owned(),
            };
            let id = if projection.activity == Activity::Data {
                "episode-scene"
            } else {
                "scene"
            };
            dock_panels.push((
                Panel::new(
                    id,
                    title,
                    "primary",
                    rsx! {
                        crate::frontend::panels::scene::Viewport {
                            source: scene_source,
                            visible_robot: scene_robot.clone(),
                            resize_revision: split_resize_revision(),
                            on_select: move |next| selected.set(next),
                        }
                    },
                )
                .with_class("scene-editor"),
                PanelRole::PrimaryVisualization,
            ));
        }
        PrimaryVisualization::EmptyEpisodes => dock_panels.push((
            Panel::new(
                "episodes-empty",
                "Episodes",
                "primary",
                rsx! {
                    div { class: "editor-empty",
                        strong { "No episode selected" }
                        span { "Record in Live or import an episode into this dataset." }
                    }
                },
            )
            .with_class("episodes-editor"),
            PanelRole::PrimaryVisualization,
        )),
    }
    match &projection.cameras {
        CameraRegistrySource::LiveAssignments if !assigned_cameras.is_empty() => {
            for camera in &assigned_cameras {
                let id = camera_panel_id(camera);
                let source = if let Some(episode) = review_episode.as_deref() {
                    recorded_camera_source_for_live_assignment(
                        episode,
                        historical_source.as_ref(),
                        camera,
                        playhead(),
                    )
                } else {
                    CameraEditorSource::Live {
                        robot: camera.0.clone(),
                        role: camera.1.clone(),
                        device: camera.2.clone(),
                    }
                };
                dock_panels.push((
                    Panel::new(
                        id.clone(),
                        format!("Camera · {}", camera.1),
                        "cameras",
                        rsx! {
                            CameraEditor {
                                element_id: recorded_camera_element_id(&id),
                                source,
                                playing: matches!(playback, TimelinePlayback::PreviewPlaying | TimelinePlayback::RobotSyncPlaying),
                            }
                        },
                    )
                    .with_home_zone(DockZone::Bottom)
                    .with_class("camera-editor"),
                    PanelRole::Camera(CameraIdentity::Live {
                        robot: camera.0.clone(),
                        role: camera.1.clone(),
                        device: camera.2.clone(),
                    }),
                ));
            }
        }
        CameraRegistrySource::LiveAssignments => dock_panels.push((
            Panel::new(
                "camera-empty",
                "Camera",
                "cameras",
                rsx! {
                    CameraEditor {
                        element_id: "recorded-camera-empty".to_owned(),
                        source: CameraEditorSource::Empty {
                            title: "No camera assigned".to_owned(),
                            detail: "Assign a camera to this arm in Settings.".to_owned(),
                        },
                        playing: false,
                    }
                },
            )
            .with_class("camera-editor"),
            PanelRole::Placeholder,
        )),
        CameraRegistrySource::RecordedEpisode(episode) => {
            let streams = historical.as_ref().map(recorded_camera_streams).unwrap_or_default();
            for (stream_id, label) in streams {
                let id = format!("episode-camera:{stream_id}");
                let source = recorded_camera_source_for_stream(
                    &episode.id,
                    historical_source.as_ref(),
                    &stream_id,
                    &label,
                    playhead(),
                );
                dock_panels.push((
                    Panel::new(
                        id.clone(),
                        format!("Recorded · {label}"),
                        "cameras",
                        rsx! {
                            CameraEditor {
                                element_id: recorded_camera_element_id(&id),
                                source,
                                playing: matches!(playback, TimelinePlayback::PreviewPlaying | TimelinePlayback::RobotSyncPlaying),
                            }
                        },
                    )
                    .with_home_zone(DockZone::Bottom)
                    .with_class("camera-editor"),
                    PanelRole::Camera(CameraIdentity::Recorded {
                        episode: episode.id.clone(),
                        stream: stream_id,
                        label,
                    }),
                ));
            }
            if !dock_panels.iter().any(|(_, role)| matches!(role, PanelRole::Camera(CameraIdentity::Recorded { episode: panel_episode, .. }) if panel_episode == &episode.id)) {
                let source = recorded_camera_registry_placeholder(&episode.id, historical_source.as_ref());
                dock_panels.push((
                    Panel::new(
                        "episode-camera-empty",
                        "Camera",
                        "cameras",
                        rsx! {
                            CameraEditor {
                                element_id: "recorded-camera-empty".to_owned(),
                                source,
                                playing: false,
                            }
                        },
                    )
                    .with_class("camera-editor"),
                    PanelRole::Placeholder,
                ));
            }
        }
        CameraRegistrySource::None => dock_panels.push((
            Panel::new(
                "data-camera-context",
                "Camera",
                "cameras",
                rsx! {
                    CameraEditor {
                        element_id: "data-camera-context".to_owned(),
                        source: CameraEditorSource::Empty {
                            title: "No episode in view".to_owned(),
                            detail: "Select an episode to inspect its recorded camera frames.".to_owned(),
                        },
                        playing: false,
                    }
                },
            )
            .with_class("camera-editor"),
            PanelRole::Placeholder,
        )),
    }

    let action_subject = subject.clone();
    let action_review_episode = review_episode.clone();
    let timeline_panel = rsx! {
        StudioTimeline {
            state: StudioTimelineState {
                subject: subject.clone(),
                playhead_seconds: playhead(),
                playback,
                follow_live: follow_live(),
                sync_robot: robot_sync,
            },
            signal_lanes: signal_lanes.clone(),
            signal_source: signal_source.clone(),
            capabilities: capabilities.clone(),
            on_action: move |action| dispatch_timeline_action(
                TimelineActionContext {
                    studio,
                    controller: timeline,
                    subject: action_subject.clone(),
                    review_episode: action_review_episode.clone(),
                    seek_context: seek_context.clone(),
                    historical_state,
                    historical_revision,
                    automatic_preview_target,
                    interactive_preview_target,
                },
                action,
            ),
        }
    };
    let timeline_panel = Panel::new("timeline", "Timeline", "timeline", timeline_panel)
        .with_class("timeline-editor");
    let definition = match activity {
        Activity::Live => {
            let workbench = Panel::new(
                "workbench",
                "Live",
                "workbench",
                rsx! {
                    aside { class: "studio-v2-workbench",
                        LiveWorkbench {
                            on_action: move |action| dispatch_live_action(studio, action),
                        }
                    }
                },
            )
            .with_class("workbench-editor");
            crate::frontend::screens::live::screen::compose(workbench, dock_panels, timeline_panel)
        }
        Activity::Data => {
            let episodes = Panel::new(
                "episodes",
                "Episodes",
                "episodes",
                rsx! { EpisodesPanel { namespace: dataset.clone() } },
            )
            .with_class("episodes-workbench-editor");
            let query = Panel::new(
                "query",
                "Query",
                "query",
                rsx! { QueryPanel { namespace: dataset.clone(), dataset_id: selected_dataset.clone(), query_available: spiral_query_available } },
            )
            .with_class("query-workbench-editor");
            let query_result = Panel::new(
                "query-result",
                "Results",
                "query-result",
                rsx! { QueryResultView {} },
            )
            .with_class("query-editor");
            crate::frontend::screens::data::screen::compose(
                episodes,
                query,
                query_result,
                dock_panels,
                timeline_panel,
            )
        }
        Activity::Rollout => {
            let workbench = Panel::new(
                "workbench",
                "Rollout",
                "workbench",
                rsx! { aside { class: "studio-v2-workbench", RolloutWorkbench {} } },
            )
            .with_class("workbench-editor");
            let inference = Panel::new(
                "rollout-inference",
                "Inference",
                "timeline",
                rsx! { InferencePanel {} },
            )
            .with_class("rollout-inference-editor");
            crate::frontend::screens::rollout::screen::compose(workbench, dock_panels, inference)
        }
        Activity::Training => unreachable!("Training owns a separate route workspace"),
    };
    let registered_panels = definition.panels;
    let panel_roles = registered_panels
        .iter()
        .map(|(panel, role)| (panel.id.clone(), role.clone()))
        .collect::<Vec<_>>();
    let workspace_panels = registered_panels
        .into_iter()
        .map(|(panel, _)| panel)
        .collect::<Vec<_>>();
    let initial_panel_layout = definition.initial_layout;
    let panel_layout_scope = definition.layout_scope;

    rsx! {
        div {
            class: "min-h-0 min-w-0 overflow-hidden",
            tabindex: "-1",
            onkeydown: move |event| {
                if event.modifiers().ctrl() || event.modifiers().meta() || event.modifiers().alt() {
                    return;
                }
                if !commands::shortcuts_enabled() {
                    return;
                }
                let recording = studio.recording();
                match event.key() {
                    Key::Character(pressed) if pressed.eq_ignore_ascii_case("r") => {
                        event.prevent_default();
                        match recording {
                            Some(recording) => commands::stop_recording(
                                studio,
                                &recording.episode,
                                true,
                                EventHandler::new(move |_: spiral_robotics_contracts::api::EpisodeSummary| {
                                    let next_revision = historical_revision.peek().wrapping_add(1);
                                    historical_revision.set(next_revision);
                                    historical_state.set(None);
                                    automatic_preview_target.set(None);
                                    interactive_preview_target.set(None);
                                }),
                            ),
                            None if activity == Activity::Live => commands::start_recording(studio),
                            None => studio.report("Switch to Live to start recording."),
                        }
                    }
                    Key::Character(pressed)
                        if pressed.eq_ignore_ascii_case("x")
                            && recording.is_some()
                            && !event.modifiers().shift() =>
                    {
                        event.prevent_default();
                        studio.report_info_from(
                            crate::frontend::app::notifications::MessageSource::Live,
                            "Press Shift+X to discard this take.",
                        );
                    }
                    Key::Character(pressed)
                        if pressed.eq_ignore_ascii_case("x") && recording.is_some() =>
                    {
                        event.prevent_default();
                        if let Some(recording) = recording {
                            commands::stop_recording(
                                studio,
                                &recording.episode,
                                false,
                                EventHandler::new(move |_| {
                                    let next_revision = historical_revision.peek().wrapping_add(1);
                                    historical_revision.set(next_revision);
                                    historical_state.set(None);
                                    automatic_preview_target.set(None);
                                    interactive_preview_target.set(None);
                                }),
                            );
                        }
                    }
                    _ => {}
                }
            },
            PanelWorkspace {
                key: "{projection.activity.label()}",
                panels: workspace_panels,
                initial_layout: initial_panel_layout,
                layout_scope: Some(panel_layout_scope),
                on_layout_change: move |_layout: PanelLayout| {},
                on_resize: move |_| {
                    let next = split_resize_revision.peek().wrapping_add(1);
                    split_resize_revision.set(next);
                },
                on_panel_activate: move |panel_id: PanelId| {
                    if let Some((_, PanelRole::Camera(CameraIdentity::Live { robot, role, .. }))) =
                        panel_roles
                            .iter()
                            .find(|(registered, _)| registered == &panel_id)
                    {
                        selected.set(Some(crate::frontend::app::state::Selection::Camera {
                            robot: robot.clone(),
                            role: role.clone(),
                        }));
                    }
                },
            }
        }
    }
}

/// Physical seeks are consequences of the Timeline playhead, not transport
/// that may replace or reposition it while the arm is travelling to a pose.
fn replay_owns_timeline_transport(replay: &spiral_robotics_contracts::api::ReplayProgress) -> bool {
    replay.purpose == ReplayPurpose::Timeline
}

fn replay_matches_timeline_subject(
    subject_episode: Option<&str>,
    replay: &spiral_robotics_contracts::api::ReplayProgress,
) -> bool {
    subject_episode == Some(replay.episode.as_str())
}

fn visible_timeline_episode(studio: Studio, activity: Activity) -> Option<String> {
    match activity {
        Activity::Live => studio.recording().map(|recording| recording.episode),
        Activity::Data => studio.selected_episode.peek().clone(),
        Activity::Training | Activity::Rollout => None,
    }
}

#[cfg(test)]
fn replay_drives_recorded_media(replay: &spiral_robotics_contracts::api::ReplayProgress) -> bool {
    replay_owns_timeline_transport(replay) && replay.phase == ReplayPhase::Playing && !replay.paused
}

fn timeline_target_episode(target: &TimelineTarget) -> &str {
    match target {
        TimelineTarget::Recording(episode) | TimelineTarget::Episode(episode) => episode,
    }
}

fn assigned_cameras(activity: Activity) -> Vec<(String, String, String)> {
    let studio = crate::frontend::app::state::studio();
    let robot = if activity == Activity::Rollout {
        rollout_follower(&studio.snapshot.read().robots)
    } else {
        studio.focused()
    };
    let Some(focused) = robot else {
        return Vec::new();
    };
    if activity == Activity::Rollout {
        let selected_key = rollout_controller().checkpoint();
        let roles =
            selected_checkpoint_camera_roles(&studio.training.read(), selected_key.as_deref());
        if !roles.is_empty() {
            return roles
                .iter()
                .filter_map(|role| {
                    focused
                        .cameras
                        .iter()
                        .find(|binding| binding.name == role.as_str())
                        .map(|binding| {
                            (focused.id.clone(), binding.name.clone(), binding.id.clone())
                        })
                })
                .collect();
        }
    }
    focused
        .cameras
        .iter()
        .map(|binding| (focused.id.clone(), binding.name.clone(), binding.id.clone()))
        .collect()
}

fn camera_panel_id(camera: &(String, String, String)) -> String {
    format!("camera:{}:{}", camera.0, camera.1)
}

fn recorded_camera_element_id(panel_id: &str) -> String {
    let safe = panel_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("recorded-{safe}")
}

fn timeline_capabilities(
    studio: Studio,
    activity: Activity,
    subject: &TimelineSubject,
) -> TimelineCapabilities {
    let record = if activity != Activity::Live {
        TimelineGate::blocked("Switch to Live to start recording.")
    } else if let Some(reason) = commands::recording_blocker(studio) {
        TimelineGate::blocked(reason)
    } else {
        TimelineGate::allowed()
    };
    let preview = if subject.recorded_episode().is_none() {
        TimelineGate::blocked("Select a completed episode or pause the current recording first.")
    } else if !subject.available() {
        TimelineGate::blocked(
            subject
                .problem()
                .unwrap_or("This Timeline subject is unavailable."),
        )
    } else {
        TimelineGate::allowed()
    };
    let seek = preview.clone();
    let synced_playback = match subject.episode() {
        Some(episode) => commands::replay_blocker(studio, episode)
            .map(TimelineGate::blocked)
            .unwrap_or_else(TimelineGate::allowed),
        None => TimelineGate::blocked("Select a completed episode to play it with robot sync."),
    };
    let robot_sync = if !subject.is_paused_recording() && subject.episode().is_none() {
        TimelineGate::blocked("Pause or select a completed episode before synchronizing a robot.")
    } else if !studio.controls_available() {
        TimelineGate::blocked("Reconnect telemetry before synchronizing a robot.")
    } else if studio.mode_active() && !matches!(studio.mode(), ModeSelection::Replay { .. }) {
        TimelineGate::blocked("Stop the active control mode before synchronizing a robot.")
    } else if timeline_seek_context(studio, subject).is_none() {
        TimelineGate::blocked("Connect and calibrate a compatible follower to synchronize it.")
    } else {
        TimelineGate::allowed()
    };
    TimelineCapabilities {
        record,
        preview,
        seek,
        synced_playback,
        robot_sync,
    }
}

fn timeline_seek_context(
    studio: Studio,
    subject: &TimelineSubject,
) -> Option<(String, String, Option<String>)> {
    let follower = studio.connected_robots().into_iter().find(|robot| {
        robot.calibrated
            && robot
                .robot_type
                .as_deref()
                .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
    })?;
    let (episode, arms) = match subject {
        TimelineSubject::Recording(recording) if recording.paused => {
            (recording.episode.clone(), recording.arms.clone())
        }
        TimelineSubject::Episode { episode, .. } => (
            episode.id.clone(),
            episode.arms.iter().map(|arm| arm.id.clone()).collect(),
        ),
        TimelineSubject::Empty { .. } | TimelineSubject::Recording(_) => return None,
    };
    let source_arm = arms
        .iter()
        .find(|arm| **arm == follower.id)
        .or_else(|| {
            arms.iter()
                .find(|arm| arm.to_ascii_lowercase().contains("follower"))
        })
        .or_else(|| arms.first())
        .cloned();
    Some((episode, follower.id, source_arm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::api::http::{
        EpisodeTimelineResponse, TimelineMetadata, TimelineSample, VideoAssetMetadata,
    };
    use spiral_robotics_contracts::api::EpisodeSummary;

    fn preview(at: f64, videos: Vec<VideoAssetMetadata>) -> HistoricalPreview {
        HistoricalPreview {
            episode: "take-1".to_owned(),
            timeline: EpisodeTimelineResponse {
                episode: EpisodeSummary {
                    dataset_id: "default".to_owned(),
                    id: "take-1".to_owned(),
                    arms: Vec::new(),
                    frames: 600,
                    duration_seconds: 10.0,
                    recording_gaps: Vec::new(),
                    sample_hz: 60.0,
                    started_ms: 0,
                    source: "Teleoperation".to_owned(),
                    max_follow_error_degrees: None,
                    discarded: false,
                    problem: None,
                },
                samples: TimelineMetadata::default(),
                videos,
            },
            sample: TimelineSample {
                seconds: at,
                ..TimelineSample::default()
            },
            signal_lanes: None,
            signal_lanes_error: None,
        }
    }

    fn video(segment_index: u32, offset: Option<f64>, duration_seconds: f64) -> VideoAssetMetadata {
        VideoAssetMetadata {
            stream_id: "follower.camera.wrist".to_owned(),
            camera_id: "camera-1".to_owned(),
            width: 640,
            height: 480,
            fps: 30,
            frames: (duration_seconds * 30.0) as u64,
            duration_seconds,
            segment_index,
            timeline_offset_seconds: offset,
        }
    }

    #[test]
    fn historical_camera_selects_the_exact_pause_segment() {
        let preview = preview(
            4.25,
            vec![video(0, Some(0.0), 2.0), video(1, Some(4.0), 3.0)],
        );

        let camera = historical_camera(&preview, None).expect("recorded camera");

        assert!(!camera.approximate);
        assert_eq!(camera.video_seconds, 0.25);
        assert_eq!(
            camera.url,
            "/api/episodes/take-1/videos/follower.camera.wrist?segment=1"
        );
    }

    #[test]
    fn historical_camera_keeps_legacy_sync_explicitly_approximate() {
        let preview = preview(5.0, vec![video(0, None, 8.0)]);

        let camera = historical_camera(&preview, None).expect("recorded camera");

        assert!(camera.approximate);
        assert_eq!(camera.video_seconds, 4.0);
        assert_eq!(
            camera.url,
            "/api/episodes/take-1/videos/follower.camera.wrist?segment=0"
        );
    }

    #[test]
    fn recorded_video_sync_uses_the_controller_clock_not_a_late_sample_response() {
        let preview = preview(1.0, vec![video(0, Some(0.0), 8.0)]);

        let camera = historical_camera_at(&preview, None, 5.0).expect("recorded camera");

        assert_eq!(camera.video_seconds, 5.0);
        assert_eq!(camera.episode_seconds, 5.0);
    }

    #[test]
    fn recorded_camera_panels_follow_episode_streams_not_live_assignments() {
        let mut top = video(0, Some(0.0), 4.0);
        top.stream_id = "follower.camera.top".to_owned();
        let timeline = preview(
            1.0,
            vec![video(0, Some(0.0), 2.0), video(1, Some(2.0), 2.0), top],
        );

        assert_eq!(
            recorded_camera_streams(&timeline),
            vec![
                ("follower.camera.wrist".to_owned(), "wrist".to_owned()),
                ("follower.camera.top".to_owned(), "top".to_owned()),
            ]
        );
    }

    #[test]
    fn default_camera_feeds_start_in_independent_horizontal_rows() {
        fn split_axis(node: &LayoutNode, split: &str) -> Option<SplitAxis> {
            match node {
                LayoutNode::Tile(_) => None,
                LayoutNode::Split {
                    id,
                    axis,
                    first,
                    second,
                    ..
                } => (id.as_str() == split)
                    .then_some(*axis)
                    .or_else(|| split_axis(first, split))
                    .or_else(|| split_axis(second, split)),
            }
        }

        let panels = vec![
            Panel::new("workbench", "Live", "workbench", rsx! { div {} }),
            Panel::new("scene", "Scene", "primary", rsx! { div {} }),
            Panel::new(
                "camera:follower:gripper",
                "Camera · gripper",
                "cameras",
                rsx! { div {} },
            ),
            Panel::new(
                "camera:follower:top",
                "Camera · top",
                "cameras",
                rsx! { div {} },
            ),
            Panel::new("timeline", "Timeline", "timeline", rsx! { div {} }),
        ];
        let layout = default_panel_layout(&panels);
        let gripper = layout.tile_for_panel(&PanelId::from("camera:follower:gripper"));
        let top = layout.tile_for_panel(&PanelId::from("camera:follower:top"));

        assert_eq!(gripper.as_ref().map(|tile| tile.as_str()), Some("cameras"));
        assert_eq!(top.as_ref().map(|tile| tile.as_str()), Some("cameras-2"));
        assert_ne!(gripper, top);
        assert_eq!(
            split_axis(&layout.root, "camera-rows-1"),
            Some(SplitAxis::Vertical)
        );
    }

    #[test]
    fn asynchronous_data_cameras_keep_their_named_right_hand_home() {
        let initial = vec![
            Panel::new("workbench", "Data", "workbench", rsx! { div {} }),
            Panel::new("episodes-empty", "Episodes", "primary", rsx! { div {} }),
            Panel::new("data-camera-context", "Camera", "cameras", rsx! { div {} }),
            Panel::new("timeline", "Timeline", "timeline", rsx! { div {} }),
        ];
        let mut layout = default_panel_layout(&initial);
        layout.reconcile(&[
            spiral_robotics_panels::PanelPlacement::new("workbench", "workbench"),
            spiral_robotics_panels::PanelPlacement::new("episode-scene", "primary"),
            spiral_robotics_panels::PanelPlacement::new("episode-camera:wrist", "cameras"),
            spiral_robotics_panels::PanelPlacement::new("timeline", "timeline"),
        ]);

        assert_eq!(
            layout
                .tile_for_panel(&PanelId::from("episode-camera:wrist"))
                .as_ref()
                .map(|tile| tile.as_str()),
            Some("cameras")
        );
    }

    #[test]
    fn query_result_joins_the_scene_without_unregistering_it() {
        let initial = vec![
            Panel::new("workbench", "Data", "workbench", rsx! { div {} }),
            Panel::new("episode-scene", "Episode", "primary", rsx! { div {} }),
            Panel::new("episode-camera:wrist", "Camera", "cameras", rsx! { div {} }),
            Panel::new("timeline", "Timeline", "timeline", rsx! { div {} }),
        ];
        let mut layout = default_panel_layout(&initial);
        layout.reconcile(&[
            spiral_robotics_panels::PanelPlacement::new("workbench", "workbench"),
            spiral_robotics_panels::PanelPlacement::new("episode-scene", "primary"),
            spiral_robotics_panels::PanelPlacement::new("query-result", "primary"),
            spiral_robotics_panels::PanelPlacement::new("episode-camera:wrist", "cameras"),
            spiral_robotics_panels::PanelPlacement::new("timeline", "timeline"),
        ]);

        let scene_tile = layout
            .tile_for_panel(&PanelId::from("episode-scene"))
            .expect("scene tile");
        let query_tile = layout
            .tile_for_panel(&PanelId::from("query-result"))
            .expect("query tile");
        assert_eq!(scene_tile, query_tile);
        let primary = layout.tile(&scene_tile).expect("primary tile");
        assert!(primary.panels.contains(&PanelId::from("episode-scene")));
        assert_eq!(primary.active, Some(PanelId::from("query-result")));
    }

    #[test]
    fn recorded_render_slots_bound_sample_fetches_to_thirty_hz() {
        assert_eq!(historical_replay_slot(f64::NAN), 0);
        assert_eq!(historical_replay_slot(0.0), 0);
        assert_eq!(historical_replay_slot(0.032), 0);
        assert_eq!(historical_replay_slot(0.034), 1);
        assert_eq!(historical_replay_slot(1.249), 37);
    }

    #[test]
    fn physical_seek_never_takes_ownership_of_timeline_transport() {
        let mut replay = spiral_robotics_contracts::api::ReplayProgress {
            episode: "take-1".to_owned(),
            robot: "follower".to_owned(),
            frame: 0,
            frames: 1,
            elapsed_seconds: 0.0,
            duration_seconds: 1.0 / 60.0,
            purpose: ReplayPurpose::PhysicalSeek,
            phase: ReplayPhase::Seeking,
            paused: false,
        };

        assert!(!replay_owns_timeline_transport(&replay));
        assert!(!replay_drives_recorded_media(&replay));
        replay.purpose = ReplayPurpose::Timeline;
        assert!(replay_owns_timeline_transport(&replay));
        assert!(!replay_drives_recorded_media(&replay));
        replay.phase = ReplayPhase::Playing;
        assert!(replay_drives_recorded_media(&replay));
        assert!(replay_matches_timeline_subject(Some("take-1"), &replay));
        assert!(!replay_matches_timeline_subject(Some("take-2"), &replay));
        assert!(!replay_matches_timeline_subject(None, &replay));
        replay.paused = true;
        assert!(!replay_drives_recorded_media(&replay));
    }

    #[test]
    fn stale_trace_results_cannot_cross_episode_revisions() {
        let mut source = Some(RecordedSource {
            episode: "take-1".to_owned(),
            seconds: 0.0,
            state: LoadState::Ready(preview(0.0, Vec::new())),
        });
        let traces = RecordedTimelineTraces::default();

        assert!(!attach_historical_traces(
            &mut source,
            "take-1",
            599,
            &traces,
        ));
        assert!(
            source
                .as_ref()
                .and_then(RecordedSource::renderable)
                .is_some_and(|preview| preview.signal_lanes.is_none())
        );

        assert!(attach_historical_traces(
            &mut source,
            "take-1",
            600,
            &traces,
        ));
        assert!(
            source
                .as_ref()
                .and_then(RecordedSource::renderable)
                .is_some_and(|preview| preview.signal_lanes.is_some())
        );

        let mut in_flight_sample = Some(RecordedSource {
            episode: "take-1".to_owned(),
            seconds: 2.0,
            state: LoadState::Loading {
                previous: Some(preview(1.0, Vec::new())),
            },
        });
        assert!(attach_historical_traces(
            &mut in_flight_sample,
            "take-1",
            600,
            &traces,
        ));
        assert!(
            in_flight_sample
                .as_ref()
                .and_then(RecordedSource::renderable)
                .is_some_and(|preview| preview.signal_lanes.is_some())
        );
    }

    #[test]
    fn a_dead_trace_request_is_recorded_as_unavailable_not_as_an_empty_episode() {
        let mut source = Some(RecordedSource {
            episode: "take-1".to_owned(),
            seconds: 0.0,
            state: LoadState::Ready(preview(0.0, Vec::new())),
        });

        // A failure from a different persisted revision belongs to a take that is
        // no longer on the clock.
        assert!(!attach_historical_trace_failure(
            &mut source,
            "take-1",
            599,
            "The signal traces did not answer within 15s.",
        ));

        assert!(attach_historical_trace_failure(
            &mut source,
            "take-1",
            600,
            "The signal traces did not answer within 15s.",
        ));
        let failed = source
            .as_ref()
            .and_then(RecordedSource::renderable)
            .expect("preview");
        assert!(failed.signal_lanes.is_none());
        assert_eq!(
            failed.signal_lanes_error.as_deref(),
            Some("The signal traces did not answer within 15s.")
        );
    }

    #[test]
    fn traces_arriving_after_a_timeout_clear_the_failure() {
        let mut source = Some(RecordedSource {
            episode: "take-1".to_owned(),
            seconds: 0.0,
            state: LoadState::Ready(preview(0.0, Vec::new())),
        });
        attach_historical_trace_failure(&mut source, "take-1", 600, "timed out");

        assert!(attach_historical_traces(
            &mut source,
            "take-1",
            600,
            &RecordedTimelineTraces::default(),
        ));
        let recovered = source
            .as_ref()
            .and_then(RecordedSource::renderable)
            .expect("preview");
        assert!(recovered.signal_lanes.is_some());
        assert_eq!(recovered.signal_lanes_error, None);

        // A late timeout from the same generation must not un-say a success the
        // operator can already see.
        assert!(!attach_historical_trace_failure(
            &mut source,
            "take-1",
            600,
            "timed out",
        ));
        assert_eq!(
            source
                .as_ref()
                .and_then(RecordedSource::renderable)
                .and_then(|preview| preview.signal_lanes_error.as_deref()),
            None
        );
    }

    #[test]
    fn a_missing_recorded_scene_source_never_falls_back_to_live() {
        let episode = preview(0.0, Vec::new()).timeline.episode;
        let subject = TimelineSubject::Episode {
            activity: Activity::Data,
            episode,
        };

        let source = recorded_scene_source(&subject, None);

        assert!(matches!(
            source,
            crate::frontend::panels::scene::SceneSource::Recorded(RecordedSource {
                state: LoadState::Loading { previous: None },
                ..
            })
        ));
    }

    #[test]
    fn a_missing_recorded_camera_source_never_falls_back_to_the_live_device() {
        let assigned = (
            "follower".to_owned(),
            "wrist".to_owned(),
            "camera-1".to_owned(),
        );

        let source = recorded_camera_source_for_live_assignment("take-1", None, &assigned, 0.0);

        assert!(matches!(
            source,
            CameraEditorSource::Recorded(RecordedSource {
                state: LoadState::Loading { previous: None },
                ..
            })
        ));
    }
}
