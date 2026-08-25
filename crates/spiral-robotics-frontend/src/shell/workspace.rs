//! Reactive assembly of the persisted frontend panel workspace.

use std::sync::Arc;

use dioxus::prelude::*;
use dioxus_workbench::{DockZone, Panel, PanelId, PanelLayout, PanelWorkspace};
#[cfg(test)]
use dioxus_workbench::{LayoutNode, SplitAxis};
use spiral_robotics_api::recording::{ReplayPhase, ReplayPurpose};
use spiral_robotics_api::robots::ModeSelection;

use crate::activities::data::relations::RelationsPanel;
use crate::activities::data::workbench::{EpisodesPanel, QueryPanel, QueryResultView, short_id};
use crate::activities::live::workbench::LiveWorkbench;
use crate::activities::rollout::state::rollout_controller;
use crate::activities::rollout::workspace::{
    InferencePanel, RolloutWorkbench, follower as rollout_follower,
    selected_checkpoint_camera_roles,
};
use crate::client::{commands, historical};
use crate::panels::camera::{
    CameraEditor, CameraEditorSource, RecordedCameraSourceRequest,
    recorded_camera_registry_placeholder, recorded_scene_source,
};
#[cfg(test)]
use crate::panels::camera::{historical_camera, historical_camera_at};
use crate::panels::timeline::panel::{
    RecordedTimelineTraces, SignalSource, StudioTimeline, StudioTimelineState,
    TimelineCapabilities, TimelineGate, TimelinePlayback,
};
use crate::panels::timeline::state::{
    RecordedTimelineActionState, TimelineActionContext, TimelineActionSession,
    TimelinePreviewTargets,
};
#[cfg(test)]
use crate::shell::activity::LoadState;
use crate::shell::activity::{
    Activity, ActivityProjection, CameraIdentity, CameraRegistrySource, PanelRole,
    PrimaryVisualization, RecordedSource, TimelineSubject, TimelineTarget,
};
#[cfg(test)]
use crate::shell::layout::default_panel_layout;
use crate::shell::routes::studio_configuration;
use crate::shell::state::Studio;

use historical::{
    EpisodeInspectionCache, HistoricalPreview, HistoricalPreviewRequest, HistoricalPreviewSignals,
    historical_replay_slot, request_historical_preview,
};

#[derive(Clone, PartialEq)]
pub(crate) struct ScreenWorkspaceDefinition {
    panels: Vec<(Panel, PanelRole)>,
    initial_layout: PanelLayout,
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

    #[cfg(test)]
    pub(crate) const fn initial_layout(&self) -> &PanelLayout {
        &self.initial_layout
    }
}

/// Shared episode-clock behavior for the routes that need robot, camera, and
/// timeline evidence. Each route owns its panel registry and layout; this host
/// owns only the cross-panel synchronization contract.
#[component]
pub(crate) fn EpisodeWorkspace(activity: Activity) -> Element {
    let studio = crate::shell::state::studio();
    let configuration = studio_configuration();
    let dataset = configuration.dataset();
    let selected_dataset = studio.selected_dataset().read().to_owned();
    let spiral_query_available = configuration.spiral_query_available();
    let timeline = crate::panels::timeline::state::timeline_controller();
    let mut playhead = timeline.playhead_signal();
    #[cfg(feature = "web")]
    let mut sampled_preview_slot = use_signal(|| None::<(String, u64)>);
    #[cfg(feature = "web")]
    let mut preview_tick_seconds = timeline.preview_tick_signal();
    let follow_live = timeline.follow_live_signal();
    let mut robot_sync_target = timeline.robot_sync_target_signal();
    let mut historical_state = use_signal(|| None::<RecordedSource<HistoricalPreview>>);
    let mut historical_revision = use_signal(|| 0_u64);
    let historical_cache = use_signal(EpisodeInspectionCache::default);
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
                    HistoricalPreviewRequest::new(episode.clone(), None, next),
                    HistoricalPreviewSignals::new(
                        historical_state,
                        historical_revision,
                        historical_cache,
                    ),
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
        let subject_episode = studio.visible_timeline_episode(activity);
        if !replay_matches_timeline_subject(subject_episode.as_deref(), &replay) {
            sampled_replay_slot.set(None);
            return;
        }
        let slot = historical_replay_slot(replay.elapsed_seconds());
        let next = (replay.episode().to_owned(), slot);
        if sampled_replay_slot.peek().as_ref() == Some(&next) {
            return;
        }
        sampled_replay_slot.set(Some(next));
        request_historical_preview(
            studio,
            HistoricalPreviewRequest::new(
                replay.episode().to_owned(),
                None,
                replay.elapsed_seconds(),
            ),
            HistoricalPreviewSignals::new(historical_state, historical_revision, historical_cache),
        );
    });

    // Server progress is authoritative. The UI-owned playhead only takes over
    // after the operator leaves the live edge or reviews a completed episode.
    use_effect(move || {
        if let Some(recording) = studio.recording() {
            if follow_live() && !recording.paused() {
                playhead.set(recording.elapsed_seconds());
            }
            if !recording.paused() {
                timeline.stop_preview();
            }
        } else if let Some(replay) = studio.replay().filter(replay_owns_timeline_transport) {
            let subject_episode = studio.visible_timeline_episode(activity);
            if replay_matches_timeline_subject(subject_episode.as_deref(), &replay) {
                playhead.set(replay.elapsed_seconds());
                timeline.stop_preview();
            }
        }
    });

    // Resuming capture returns every visualization to the live edge. A stale
    // historical pose must never remain on screen while Teleop is active.
    use_effect(move || {
        if studio
            .recording()
            .is_some_and(|recording| !recording.paused())
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
        let (episode, frames, seconds) = match studio.recording() {
            Some(recording) if recording.paused() => (
                recording.episode().to_owned(),
                recording.frames(),
                recording.elapsed_seconds(),
            ),
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
            .is_some_and(|preview| preview.episode() == episode)
        {
            return;
        }
        request_historical_preview(
            studio,
            HistoricalPreviewRequest::new(episode, Some(frames), seconds),
            HistoricalPreviewSignals::new(historical_state, historical_revision, historical_cache),
        );
    });

    let data_episode_context = studio.focused_episode();
    let projection = ActivityProjection::derive(activity, studio.recording(), data_episode_context);
    let projected_recorded_identity = match &projection.timeline() {
        TimelineSubject::Recording(recording) if recording.paused() => Some((
            recording.episode().to_owned(),
            recording.frames(),
            recording.elapsed_seconds(),
        )),
        TimelineSubject::Episode { episode, .. } => {
            Some((episode.id().to_owned(), episode.frames(), 0.0))
        }
        TimelineSubject::Empty { .. } | TimelineSubject::Recording(_) => None,
    };
    use_effect(use_reactive(
        (&projected_recorded_identity,),
        move |(identity,)| {
            let Some((episode, source_frames, seconds)) = identity else {
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
                HistoricalPreviewRequest::new(episode, Some(source_frames), seconds),
                HistoricalPreviewSignals::new(
                    historical_state,
                    historical_revision,
                    historical_cache,
                ),
            );
        },
    ));
    let subject = projection.timeline().clone();
    let live_signal_lanes = Arc::new({
        let recording_signals = studio.recording_signals();
        let store = recording_signals.read();
        store.timeline_lanes()
    });
    let assigned_cameras = assigned_cameras(projection.activity());
    let review_episode = subject.recorded_episode().map(str::to_owned);
    let historical_source = historical_state
        .read()
        .clone()
        .filter(|source| Some(source.episode()) == review_episode.as_deref());
    let historical = historical_source
        .as_ref()
        .and_then(RecordedSource::renderable)
        .cloned();
    let history_active = review_episode.is_some();
    let signal_lanes = match historical.as_ref() {
        Some(preview) => Arc::clone(preview.signal_lanes()),
        None if history_active => Arc::new(RecordedTimelineTraces::default().signal_lanes()),
        None => live_signal_lanes,
    };
    let signal_source = match review_episode.as_ref() {
        None => SignalSource::Live,
        Some(episode) => {
            let failure = historical_source
                .as_ref()
                .and_then(|source| source.state().reason().map(str::to_owned));
            match failure {
                Some(reason) => SignalSource::RecordedUnavailable {
                    episode: episode.clone(),
                    reason,
                },
                None if historical.is_some() => SignalSource::RecordedReady,
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
    let capabilities = studio.timeline_capabilities(projection.activity(), &subject);
    let seek_context = studio.timeline_seek_context(&subject);
    let playback = studio
        .replay()
        .filter(replay_owns_timeline_transport)
        .and_then(|replay| {
            (subject.episode_id() == Some(replay.episode())).then_some(if replay.paused() {
                TimelinePlayback::RobotSyncPaused
            } else if replay.phase() == ReplayPhase::Seeking {
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
    let disarm_robot_sync = requested_robot_sync && !capabilities.robot_sync().is_allowed();
    use_effect(use_reactive(
        (&timeline_target, &disarm_robot_sync),
        move |(current_target, must_disarm)| {
            let armed = robot_sync_target.peek().to_owned();
            let target_changed = armed.is_some() && armed.as_ref() != current_target.as_ref();
            if !target_changed && !must_disarm {
                return;
            }
            robot_sync_target.set(None);
            let armed_episode = armed.as_ref().map(timeline_target_episode);
            if let Some(replay) = studio.replay()
                && armed_episode == Some(replay.episode())
            {
                studio.stop_replay(replay.episode());
            }
        },
    ));
    let mut selected = studio.selection();
    let scene_robot = (projection.activity() == Activity::Rollout)
        .then(|| {
            rollout_follower(studio.snapshot().read().robots()).map(|robot| robot.id().to_owned())
        })
        .flatten();

    let mut dock_panels = Vec::<(Panel, PanelRole)>::new();
    match &projection.primary() {
        PrimaryVisualization::LiveScene | PrimaryVisualization::RecordedScene(_) => {
            let scene_source = recorded_scene_source(&subject, historical_source.as_ref());
            let title = match &projection.primary() {
                PrimaryVisualization::RecordedScene(episode)
                    if projection.activity() == Activity::Data =>
                {
                    format!("Episode · {}", short_id(episode.id()))
                }
                _ => "Scene".to_owned(),
            };
            let id = if projection.activity() == Activity::Data {
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
                        crate::panels::scene::Viewport {
                            source: scene_source,
                            presentation: crate::panels::scene::ViewportPresentation::new(
                                scene_robot.clone(),
                                split_resize_revision(),
                            ),
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
    match &projection.cameras() {
        CameraRegistrySource::LiveAssignments if !assigned_cameras.is_empty() => {
            for camera in &assigned_cameras {
                let id = camera_panel_id(camera);
                let source = if let Some(episode) = review_episode.as_deref() {
                    RecordedCameraSourceRequest::new(
                        episode,
                        historical_source.as_ref(),
                        playhead(),
                    )
                    .for_live_assignment(camera)
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
            let streams = historical
                .as_ref()
                .map(HistoricalPreview::recorded_camera_streams)
                .unwrap_or_default();
            for (stream_id, label) in streams {
                let id = format!("episode-camera:{stream_id}");
                let source = RecordedCameraSourceRequest::new(
                    episode.id(),
                    historical_source.as_ref(),
                    playhead(),
                )
                .for_stream(&stream_id, &label);
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
                        episode: episode.id().to_owned(),
                        stream: stream_id,
                        label,
                    }),
                ));
            }
            if !dock_panels.iter().any(|(_, role)| matches!(role, PanelRole::Camera(CameraIdentity::Recorded { episode: panel_episode, .. }) if panel_episode == episode.id())) {
                let source = recorded_camera_registry_placeholder(episode.id(), historical_source.as_ref());
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
            state: StudioTimelineState::new(subject.clone(), playhead(), playback)
                .with_controls(follow_live(), robot_sync),
            visualization: crate::panels::timeline::panel::TimelineVisualization::new(
                signal_lanes.clone(),
                signal_source.clone(),
                capabilities.clone(),
            ),
            on_action: move |action| TimelineActionContext::new(
                    TimelineActionSession::new(studio, timeline, action_subject.clone()),
                    RecordedTimelineActionState::new(
                        action_review_episode.clone(),
                        seek_context.clone(),
                        HistoricalPreviewSignals::new(
                            historical_state,
                            historical_revision,
                            historical_cache,
                        ),
                    ),
                    TimelinePreviewTargets::new(
                        automatic_preview_target,
                        interactive_preview_target,
                    ),
                )
                .dispatch(action),
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
                            on_action: move |action| studio.dispatch_live_action(action),
                        }
                    }
                },
            )
            .with_class("workbench-editor");
            ScreenWorkspaceDefinition::for_live(workbench, dock_panels, timeline_panel)
        }
        Activity::Data => {
            let episodes = Panel::new(
                "episodes",
                "Episodes",
                "episodes",
                rsx! { EpisodesPanel { namespace: dataset } },
            )
            .with_class("episodes-workbench-editor");
            let query = Panel::new(
                "query",
                "Query",
                "query",
                rsx! { QueryPanel { namespace: dataset, dataset_id: selected_dataset.clone(), query_available: spiral_query_available } },
            )
            .with_class("query-workbench-editor");
            let query_result = Panel::new(
                "query-result",
                "Results",
                "query-result",
                rsx! { QueryResultView {} },
            )
            .with_class("query-editor");
            let relations = Panel::new(
                "relations",
                "Relations",
                "primary",
                rsx! { RelationsPanel { namespace: dataset, query_available: spiral_query_available } },
            )
            .with_class("relations-editor");
            ScreenWorkspaceDefinition::for_data(
                crate::activities::data::screen::DataWorkbenchPanels::new(episodes, query),
                crate::activities::data::screen::DataEvidencePanels::new(
                    query_result,
                    relations,
                    crate::activities::data::screen::DataContextPanels::new(
                        dock_panels,
                        timeline_panel,
                    ),
                ),
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
            ScreenWorkspaceDefinition::for_rollout(workbench, dock_panels, inference)
        }
        Activity::Training | Activity::Workflow => {
            unreachable!("Training and Workflow own separate route workspaces")
        }
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
                            Some(recording) => studio.stop_recording(
                                recording.episode(),
                                commands::RecordingCompletion::new(true, EventHandler::new(move |_: spiral_robotics_api::recording::EpisodeSummary| {
                                    let next_revision = historical_revision.peek().wrapping_add(1);
                                    historical_revision.set(next_revision);
                                    historical_state.set(None);
                                    automatic_preview_target.set(None);
                                    interactive_preview_target.set(None);
                                })),
                            ),
                            None if activity == Activity::Live => studio.start_recording(),
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
                            crate::shell::notifications::MessageSource::Live,
                            "Press Shift+X to discard this take.",
                        );
                    }
                    Key::Character(pressed)
                        if pressed.eq_ignore_ascii_case("x") && recording.is_some() =>
                    {
                        event.prevent_default();
                        if let Some(recording) = recording {
                            studio.stop_recording(
                                recording.episode(),
                                commands::RecordingCompletion::new(false, EventHandler::new(move |_| {
                                    let next_revision = historical_revision.peek().wrapping_add(1);
                                    historical_revision.set(next_revision);
                                    historical_state.set(None);
                                    automatic_preview_target.set(None);
                                    interactive_preview_target.set(None);
                                })),
                            );
                        }
                    }
                    _ => {}
                }
            },
            PanelWorkspace {
                key: "{projection.activity().label()}",
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
                        selected.set(Some(crate::shell::state::Selection::Camera {
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
fn replay_owns_timeline_transport(replay: &spiral_robotics_api::recording::ReplayProgress) -> bool {
    replay.purpose() == ReplayPurpose::Timeline
}

fn replay_matches_timeline_subject(
    subject_episode: Option<&str>,
    replay: &spiral_robotics_api::recording::ReplayProgress,
) -> bool {
    subject_episode == Some(replay.episode())
}

impl Studio {
    fn visible_timeline_episode(self, activity: Activity) -> Option<String> {
        let studio = self;
        match activity {
            Activity::Live => studio
                .recording()
                .map(|recording| recording.episode().to_owned()),
            Activity::Data => studio.selected_episode().peek().to_owned(),
            Activity::Workflow | Activity::Training | Activity::Rollout => None,
        }
    }
}

#[cfg(test)]
fn replay_drives_recorded_media(replay: &spiral_robotics_api::recording::ReplayProgress) -> bool {
    replay_owns_timeline_transport(replay)
        && replay.phase() == ReplayPhase::Playing
        && !replay.paused()
}

fn timeline_target_episode(target: &TimelineTarget) -> &str {
    match target {
        TimelineTarget::Recording(episode) | TimelineTarget::Episode(episode) => episode,
    }
}

fn assigned_cameras(activity: Activity) -> Vec<(String, String, String)> {
    let studio = crate::shell::state::studio();
    let robot = if activity == Activity::Rollout {
        rollout_follower(studio.snapshot().read().robots())
    } else {
        studio.focused()
    };
    let Some(focused) = robot else {
        return Vec::new();
    };
    if activity == Activity::Rollout {
        let selected_key = rollout_controller().checkpoint();
        let roles =
            selected_checkpoint_camera_roles(&studio.training().read(), selected_key.as_deref());
        if !roles.is_empty() {
            return roles
                .iter()
                .filter_map(|role| {
                    focused
                        .cameras()
                        .iter()
                        .find(|binding| binding.name() == role.as_str())
                        .map(|binding| {
                            (
                                focused.id().to_owned(),
                                binding.name().to_owned(),
                                binding.id().to_owned(),
                            )
                        })
                })
                .collect();
        }
    }
    focused
        .cameras()
        .iter()
        .map(|binding| {
            (
                focused.id().to_owned(),
                binding.name().to_owned(),
                binding.id().to_owned(),
            )
        })
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

impl Studio {
    fn timeline_capabilities(
        self,
        activity: Activity,
        subject: &TimelineSubject,
    ) -> TimelineCapabilities {
        let studio = self;
        let record = if activity != Activity::Live {
            TimelineGate::blocked("Switch to Live to start recording.")
        } else if let Some(reason) = studio.recording_blocker() {
            TimelineGate::blocked(reason)
        } else {
            TimelineGate::allowed()
        };
        let preview = if subject.recorded_episode().is_none() {
            TimelineGate::blocked(
                "Select a completed episode or pause the current recording first.",
            )
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
            Some(episode) => studio
                .replay_blocker(episode)
                .map(TimelineGate::blocked)
                .unwrap_or_else(TimelineGate::allowed),
            None => TimelineGate::blocked("Select a completed episode to play it with robot sync."),
        };
        let robot_sync = if !subject.is_paused_recording() && subject.episode().is_none() {
            TimelineGate::blocked(
                "Pause or select a completed episode before synchronizing a robot.",
            )
        } else if !studio.controls_available() {
            TimelineGate::blocked("Reconnect telemetry before synchronizing a robot.")
        } else if studio.mode_active() && !matches!(studio.mode(), ModeSelection::Replay { .. }) {
            TimelineGate::blocked("Stop the active control mode before synchronizing a robot.")
        } else if studio.timeline_seek_context(subject).is_none() {
            TimelineGate::blocked("Connect and calibrate a compatible follower to synchronize it.")
        } else {
            TimelineGate::allowed()
        };
        TimelineCapabilities::new(record, preview, seek)
            .with_robot_controls(synced_playback, robot_sync)
    }

    fn timeline_seek_context(
        self,
        subject: &TimelineSubject,
    ) -> Option<(String, String, Option<String>)> {
        let studio = self;
        let follower = studio.connected_robots().into_iter().find(|robot| {
            robot.calibrated()
                && robot
                    .robot_type()
                    .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
        })?;
        let (episode, arms) = match subject {
            TimelineSubject::Recording(recording) if recording.paused() => {
                (recording.episode().to_owned(), recording.arms().to_owned())
            }
            TimelineSubject::Episode { episode, .. } => (
                episode.id().to_owned(),
                episode
                    .arms()
                    .iter()
                    .map(|arm| arm.id().to_owned())
                    .collect(),
            ),
            TimelineSubject::Empty { .. } | TimelineSubject::Recording(_) => return None,
        };
        let source_arm = arms
            .iter()
            .find(|arm| **arm == follower.id())
            .or_else(|| {
                arms.iter()
                    .find(|arm| arm.to_ascii_lowercase().contains("follower"))
            })
            .or_else(|| arms.first())
            .cloned();
        Some((episode, follower.id().to_owned(), source_arm))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::http::{
        EpisodeTimelineResponse, TimelineMetadata, TimelineSample, VideoAssetMetadata,
    };
    use spiral_robotics_api::recording::{
        EpisodeAssessment, EpisodeIdentity, EpisodeSummary, EpisodeTimeline, EpisodeVideoFormat,
        EpisodeVideoIdentity, EpisodeVideoTimeline, ReplayIdentity, ReplayTimeline,
        ReplayTransport,
    };

    fn episode_summary() -> EpisodeSummary {
        EpisodeSummary::new(
            EpisodeIdentity::new("take-1", "default", Vec::new()).with_source("Teleoperation"),
            EpisodeTimeline::new(600, 10.0, 60.0),
            EpisodeAssessment::new(false, None, None),
        )
    }

    fn preview(at: f64, videos: Vec<VideoAssetMetadata>) -> HistoricalPreview {
        HistoricalPreview::fixture(
            "take-1".to_owned(),
            Arc::new(EpisodeTimelineResponse::new(
                episode_summary(),
                TimelineMetadata::default(),
                videos,
            )),
            (
                TimelineSample::new(0, at),
                Arc::new(RecordedTimelineTraces::default().signal_lanes()),
            ),
        )
    }

    fn video(segment_index: u32, offset: Option<f64>, duration_seconds: f64) -> VideoAssetMetadata {
        VideoAssetMetadata::new(
            EpisodeVideoIdentity::new("follower.camera.wrist", "camera-1", segment_index),
            EpisodeVideoFormat::new(640, 480, 30),
            EpisodeVideoTimeline::new((duration_seconds * 30.0) as u64, duration_seconds, offset),
        )
    }

    #[test]
    fn historical_camera_selects_the_exact_pause_segment() {
        let preview = preview(
            4.25,
            vec![video(0, Some(0.0), 2.0), video(1, Some(4.0), 3.0)],
        );

        let camera = historical_camera(&preview, None).expect("recorded camera");

        assert!(!camera.approximate());
        assert_eq!(camera.video_seconds(), 0.25);
        assert_eq!(
            camera.url(),
            "/api/episodes/take-1/videos/follower.camera.wrist?segment=1"
        );
    }

    #[test]
    fn historical_camera_keeps_legacy_sync_explicitly_approximate() {
        let preview = preview(5.0, vec![video(0, None, 8.0)]);

        let camera = historical_camera(&preview, None).expect("recorded camera");

        assert!(camera.approximate());
        assert_eq!(camera.video_seconds(), 4.0);
        assert_eq!(
            camera.url(),
            "/api/episodes/take-1/videos/follower.camera.wrist?segment=0"
        );
    }

    #[test]
    fn recorded_video_sync_uses_the_controller_clock_not_a_late_sample_response() {
        let preview = preview(1.0, vec![video(0, Some(0.0), 8.0)]);

        let camera = historical_camera_at(&preview, None, 5.0).expect("recorded camera");

        assert_eq!(camera.video_seconds(), 5.0);
        assert_eq!(camera.episode_seconds(), 5.0);
    }

    #[test]
    fn recorded_camera_panels_follow_episode_streams_not_live_assignments() {
        let top = video(0, Some(0.0), 4.0);
        let top = top.with_stream_id("follower.camera.top");
        let timeline = preview(
            1.0,
            vec![video(0, Some(0.0), 2.0), video(1, Some(2.0), 2.0), top],
        );

        assert_eq!(
            timeline.recorded_camera_streams(),
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
            dioxus_workbench::PanelPlacement::new("workbench", "workbench"),
            dioxus_workbench::PanelPlacement::new("episode-scene", "primary"),
            dioxus_workbench::PanelPlacement::new("episode-camera:wrist", "cameras"),
            dioxus_workbench::PanelPlacement::new("timeline", "timeline"),
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
            dioxus_workbench::PanelPlacement::new("workbench", "workbench"),
            dioxus_workbench::PanelPlacement::new("episode-scene", "primary"),
            dioxus_workbench::PanelPlacement::new("query-result", "primary"),
            dioxus_workbench::PanelPlacement::new("episode-camera:wrist", "cameras"),
            dioxus_workbench::PanelPlacement::new("timeline", "timeline"),
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
        let mut replay = spiral_robotics_api::recording::ReplayProgress::new(
            ReplayIdentity::new("take-1", "follower"),
            ReplayTimeline::new(0, 1, 0.0).with_duration_seconds(1.0 / 60.0),
            ReplayTransport::new(ReplayPurpose::PhysicalSeek, ReplayPhase::Seeking, false),
        );

        assert!(!replay_owns_timeline_transport(&replay));
        assert!(!replay_drives_recorded_media(&replay));
        replay.set_transport(ReplayTransport::new(
            ReplayPurpose::Timeline,
            ReplayPhase::Seeking,
            false,
        ));
        assert!(replay_owns_timeline_transport(&replay));
        assert!(!replay_drives_recorded_media(&replay));
        replay.set_transport(ReplayTransport::new(
            ReplayPurpose::Timeline,
            ReplayPhase::Playing,
            false,
        ));
        assert!(replay_drives_recorded_media(&replay));
        assert!(replay_matches_timeline_subject(Some("take-1"), &replay));
        assert!(!replay_matches_timeline_subject(Some("take-2"), &replay));
        assert!(!replay_matches_timeline_subject(None, &replay));
        replay.set_transport(ReplayTransport::new(
            ReplayPurpose::Timeline,
            ReplayPhase::Playing,
            true,
        ));
        assert!(!replay_drives_recorded_media(&replay));
    }

    #[test]
    fn a_missing_recorded_scene_source_never_falls_back_to_live() {
        let episode = preview(0.0, Vec::new()).timeline().episode().clone();
        let subject = TimelineSubject::Episode {
            activity: Activity::Data,
            episode,
        };

        let source = recorded_scene_source(&subject, None);

        assert!(matches!(
            source,
            crate::panels::scene::SceneSource::Recorded(source)
                if matches!(source.state(), LoadState::Loading { previous: None })
        ));
    }

    #[test]
    fn a_missing_recorded_camera_source_never_falls_back_to_the_live_device() {
        let assigned = (
            "follower".to_owned(),
            "wrist".to_owned(),
            "camera-1".to_owned(),
        );

        let source =
            RecordedCameraSourceRequest::new("take-1", None, 0.0).for_live_assignment(&assigned);

        assert!(matches!(
            source,
            CameraEditorSource::Recorded(source)
                if matches!(source.state(), LoadState::Loading { previous: None })
        ));
    }
}
