//! The studio shell: global session rails around a persisted tiled workbench.

use dioxus::prelude::*;
use rerobot_core::api::{DataSettings, ModeSelection};
use rerobot_panels::{
    DockZone, LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis,
};

use crate::activity::{
    Activity, ActivityProjection, CameraIdentity, CameraRegistrySource, LoadState, PanelRole,
    PrimaryVisualization, RecordedSource, TimelineSubject, TimelineTarget,
};
use crate::data_workbench::{
    DEFAULT_QUERY_ROWS, DataIntent, DataWorkbench, DataWorkbenchTab, QueryResultView,
    QueryViewModel, default_query_sql, short_id,
};
use crate::icons::{DataIcon, RobotPanelIcon, SettingsIcon};
use crate::live_workbench::{LiveIntent, LiveWorkbench};
use crate::settings_panel::SettingsPanel;
use crate::state::Studio;
use crate::studio_timeline::{
    RecordedTimelineTraces, StudioTimeline, StudioTimelineState, TimelineAction,
    TimelineCapabilities, TimelineCaptureAction, TimelineGate, TimelinePlayback,
    TimelinePreviewAction, TimelineRobotSyncAction, TimelineSignalLane,
    load_recorded_timeline_traces, signal_lanes_from_store,
};

#[cfg(feature = "web")]
use wasm_bindgen::{JsCast as _, JsValue};

const TAILWIND: Asset = asset!("/assets/tailwind.css");
const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='7' fill='%2337352f'/%3E%3Cpath d='M9 23V13l7-5 7 5v10' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E";
const HISTORICAL_RENDER_HZ: f64 = 30.0;

#[component]
pub fn App() -> Element {
    Studio::use_studio();
    crate::state::use_description();
    let mut studio = crate::state::studio();
    let mut workbench = use_signal(Activity::default);
    let mut settings_open = use_signal(|| false);
    let mut settings_revision = use_signal(|| 0_u64);
    let settings = use_resource(move || {
        let _revision = settings_revision();
        async { crate::api::get::<DataSettings>("/api/settings/data").await }
    });
    let dataset = settings
        .read_unchecked()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|settings| settings.spiral.dataset.clone())
        .filter(|dataset| !dataset.trim().is_empty())
        .unwrap_or_else(|| "rerobot".to_owned());
    let spiral_query_available = settings
        .read_unchecked()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .is_some_and(|settings| {
            settings.spiral.enabled
                && (settings.spiral.uri == "spiral:memory:"
                    || !settings.spiral.project_id.trim().is_empty())
        });
    let mut data_intent = use_signal(|| None::<DataIntent>);
    let initial_query_dataset = dataset.clone();
    let data_tab = use_signal(DataWorkbenchTab::default);
    let data_sql = use_signal(move || default_query_sql(&initial_query_dataset));
    let data_max_rows = use_signal(|| DEFAULT_QUERY_ROWS);
    let data_query = use_signal(|| QueryViewModel::idle(data_sql(), data_max_rows()));
    let mut playhead = use_signal(|| 0.0_f64);
    let mut preview_playing = use_signal(|| false);
    let mut preview_target = use_signal(|| None::<(String, f64)>);
    #[cfg(feature = "web")]
    let mut sampled_preview_slot = use_signal(|| None::<(String, u64)>);
    #[cfg(feature = "web")]
    let mut preview_tick_seconds = use_signal(|| None::<f64>);
    let mut follow_live = use_signal(|| true);
    let mut robot_sync_target = use_signal(|| None::<TimelineTarget>);
    let mut historical_state = use_signal(|| None::<RecordedSource<HistoricalPreview>>);
    let mut historical_revision = use_signal(|| 0_u64);
    let mut sampled_replay_slot = use_signal(|| None::<(String, u64)>);
    let mut live_review_episode = use_signal(|| None::<String>);
    let mut automatic_preview_target = use_signal(|| None::<(String, u64)>);
    let mut interactive_preview_target = use_signal(|| None::<(String, u64)>);
    let mut split_resize_revision = use_signal(|| 0_u64);

    // One reactive clock drives every recorded video element. Child component
    // effects only observed signals they owned, so a prop-only historical
    // sample update could change the printed time without seeking the media.
    // Reading these two App signals here makes frame and playback changes
    // explicit dependencies and keeps hidden mounted tabs synchronized too.
    #[cfg(feature = "web")]
    use_effect(move || {
        let source = historical_state();
        let should_play =
            preview_playing() || studio.replay().is_some_and(|progress| !progress.paused);
        let Some(preview) = source
            .as_ref()
            .and_then(RecordedSource::renderable)
            .cloned()
        else {
            return;
        };

        for camera in assigned_cameras() {
            if let Some(recorded) = historical_camera(&preview, Some(&camera)) {
                sync_recorded_camera(
                    &recorded_camera_element_id(&camera_panel_id(&camera)),
                    recorded.video_seconds,
                    should_play,
                );
            }
        }
        for (stream_id, label) in recorded_camera_streams(&preview) {
            if let Some(recorded) = historical_camera_for_stream(&preview, &stream_id, &label) {
                sync_recorded_camera(
                    &recorded_camera_element_id(&format!("episode-camera:{stream_id}")),
                    recorded.video_seconds,
                    should_play,
                );
            }
        }
    });

    // Visual preview is a controller-owned clock. It advances the one playhead
    // and samples every recorded renderer from the same episode/time identity.
    // Robot synchronization uses the same user-facing transport, while its
    // motion and progress remain a separate server-owned consequence.
    #[cfg(feature = "web")]
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(33).await;
            if !preview_playing() {
                preview_tick_seconds.set(None);
                continue;
            }
            let Some((episode, duration)) = preview_target() else {
                preview_playing.set(false);
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
                preview_playing.set(false);
                preview_tick_seconds.set(None);
            }
        }
    });

    // Live and Data deliberately own different episode selections. Recording
    // updates the Live review target, while DataWorkbench owns
    // `studio.selected_episode`. Sharing one signal here was the reason a Data
    // episode stayed on screen after the operator returned to Live.
    use_effect(move || {
        if let Some(recording) = studio.recording()
            && live_review_episode.peek().as_deref() != Some(recording.episode.as_str())
        {
            live_review_episode.set(Some(recording.episode));
        }
    });

    // Replay telemetry may arrive at the controller rate. Historical scene
    // samples are bounded to a smooth visual cadence so the canvas receives
    // enough distinct poses without issuing duplicate requests for the same
    // display frame; browser-native video runs on its own media clock.
    // One-frame Replay states belong to physical scrub seeks and deliberately
    // stay owned by the explicit timeline request instead of resetting to 0s.
    use_effect(move || {
        let Some(replay) = studio.replay().filter(|replay| replay.frames > 1) else {
            sampled_replay_slot.set(None);
            return;
        };
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
                preview_playing.set(false);
                preview_target.set(None);
            }
        } else if let Some(replay) = studio.replay().filter(replay_owns_timeline_transport) {
            playhead.set(replay.elapsed_seconds);
            preview_playing.set(false);
            preview_target.set(None);
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

    // A paused or just-stopped Live take remains inspectable in Live. Do not
    // treat a generic idle startup as a stopped recording: doing that loaded
    // Data's newest episode into Live and hid the healthy live camera behind a
    // historical "no video" state.
    use_effect(move || {
        let (episode, seconds) = match studio.recording() {
            Some(recording) if recording.paused => (recording.episode, recording.elapsed_seconds),
            Some(_) => {
                if automatic_preview_target.peek().is_some() {
                    automatic_preview_target.set(None);
                }
                return;
            }
            None => match live_review_episode() {
                Some(episode) if workbench() == Activity::Live => (episode, 0.0),
                _ => return,
            },
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

    let live_review = live_review_episode().and_then(|id| {
        studio
            .episodes()
            .into_iter()
            .find(|episode| episode.id == id && episode.usable() && !episode.discarded)
    });
    let data_episode_context = studio.focused_episode();
    let projection = ActivityProjection::derive(
        workbench(),
        studio.recording(),
        live_review,
        data_intent.read().as_ref(),
        data_episode_context,
    );
    let projected_recorded_episode = projection.timeline.recorded_episode().map(str::to_owned);
    use_effect(use_reactive(
        (&projected_recorded_episode,),
        move |(episode,)| {
            let Some(episode) = episode else { return };
            if historical_state
                .peek()
                .as_ref()
                .is_some_and(|source| source.episode == episode)
            {
                return;
            }
            request_historical_preview(studio, episode, 0.0, historical_state, historical_revision);
        },
    ));
    let subject = projection.timeline.clone();
    let live_signal_lanes = {
        let store = studio.recording_signals.read();
        signal_lanes_from_store(&store)
    };
    let assigned_cameras = assigned_cameras();
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
    let timeline_target = subject.target();
    let requested_robot_sync = robot_sync_target.read().as_ref() == timeline_target.as_ref();
    let capabilities = timeline_capabilities(studio, projection.activity, &subject);
    let seek_context = timeline_seek_context(studio, &subject);
    let playback = studio
        .replay()
        .filter(replay_owns_timeline_transport)
        .and_then(|replay| {
            (subject.episode_id() == Some(replay.episode.as_str())).then_some(if replay.paused {
                TimelinePlayback::RobotSyncPaused
            } else {
                TimelinePlayback::RobotSyncPlaying
            })
        })
        .unwrap_or_else(|| {
            if preview_playing() {
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
                crate::commands::stop_replay(studio, &replay.episode);
            }
        },
    ));
    let mut selected = studio.selected;

    let dock_scope = projection.activity.label().to_ascii_lowercase();
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
                        crate::viewport::Viewport {
                            source: scene_source,
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
    if let Some(model) = &projection.query {
        dock_panels.push((
            Panel::new(
                "query-result",
                "Query result",
                "primary",
                rsx! { QueryResultView { model: model.clone() } },
            )
            .with_class("query-editor"),
            PanelRole::PrimaryVisualization,
        ));
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
            capabilities: capabilities.clone(),
            on_action: move |action| match action {
                TimelineAction::Capture { episode, action } => match action {
                    TimelineCaptureAction::Start if action_subject.activity() == Activity::Live => {
                        crate::commands::start_recording(studio);
                    }
                    TimelineCaptureAction::Pause | TimelineCaptureAction::Resume => {
                        if let Some(episode) = episode {
                            let paused = action == TimelineCaptureAction::Pause;
                            crate::commands::set_recording_paused(studio, &episode, paused);
                            follow_live.set(!paused);
                            if !paused {
                                historical_state.set(None);
                            }
                        }
                    }
                    TimelineCaptureAction::Stop | TimelineCaptureAction::Discard => {
                        if let Some(episode) = episode {
                            let keep = action == TimelineCaptureAction::Stop;
                            crate::commands::stop_recording(
                                studio,
                                &episode,
                                keep,
                                EventHandler::new(move |summary: rerobot_core::api::EpisodeSummary| {
                                    if keep {
                                        live_review_episode.set(Some(summary.id));
                                    } else {
                                        live_review_episode.set(None);
                                    }
                                }),
                            );
                        }
                    }
                    TimelineCaptureAction::Start => {
                        studio.report("Recording is only available in Live.");
                    }
                },
                TimelineAction::Preview { episode, action }
                    if action_subject.episode_id() == Some(episode.as_str()) =>
                {
                    match action {
                        TimelinePreviewAction::Play => {
                            preview_target.set(Some((episode, action_subject.duration_seconds())));
                            preview_playing.set(true);
                            follow_live.set(false);
                        }
                        TimelinePreviewAction::Pause => preview_playing.set(false),
                    }
                }
                TimelineAction::RobotSyncTransport { episode, action }
                    if action_subject.episode_id() == Some(episode.as_str()) =>
                {
                    match action {
                        TimelineRobotSyncAction::Start => {
                            preview_playing.set(false);
                            preview_target.set(None);
                            follow_live.set(false);
                            if let Some(summary) = action_subject.episode() {
                                crate::commands::start_replay(studio, summary);
                            }
                        }
                        TimelineRobotSyncAction::Pause => {
                            crate::commands::set_replay_paused(studio, &episode, true)
                        }
                        TimelineRobotSyncAction::Resume => {
                            crate::commands::set_replay_paused(studio, &episode, false)
                        }
                    }
                }
                TimelineAction::Seek(request)
                    if action_subject.target().as_ref() == Some(&request.target) =>
                {
                    preview_playing.set(false);
                    playhead.set(request.seconds);
                    if let Some(episode) = action_review_episode.clone() {
                        let target = (episode.clone(), historical_replay_slot(request.seconds));
                        if request.committed
                            || interactive_preview_target.peek().as_ref() != Some(&target)
                        {
                            interactive_preview_target.set(Some(target));
                            request_historical_preview(
                                studio,
                                episode,
                                request.seconds,
                                historical_state,
                                historical_revision,
                            );
                        }
                    }
                    if request.committed
                        && request.move_robot
                        && let Some((episode, robot, source_arm)) = seek_context.clone()
                    {
                        crate::commands::physical_seek(
                            studio,
                            episode,
                            robot,
                            source_arm,
                            request.seconds,
                        );
                    }
                }
                TimelineAction::SetFollowLive(next) => {
                    follow_live.set(next);
                    if next {
                        preview_playing.set(false);
                        if let Some(recording) = studio.recording() {
                            playhead.set(recording.elapsed_seconds);
                        }
                    }
                }
                TimelineAction::SetRobotSync { target, enabled }
                    if action_subject.target().as_ref() == Some(&target) =>
                {
                    if enabled {
                        preview_playing.set(false);
                        preview_target.set(None);
                        robot_sync_target.set(Some(target));
                    } else {
                        robot_sync_target.set(None);
                        if let Some(replay) = studio.replay()
                            && action_subject.episode_id() == Some(replay.episode.as_str())
                        {
                            crate::commands::stop_replay(studio, &replay.episode);
                        }
                    }
                }
                _ => studio.report("The Timeline subject changed before the action was applied."),
            },
        }
    };
    let workbench_panel = Panel::new(
        "workbench",
        workbench().label(),
        "workbench",
        rsx! {
            aside { class: "studio-v2-workbench",
                div {
                    class: if workbench() == Activity::Live { "h-full" } else { "hidden h-full" },
                    LiveWorkbench {
                        on_intent: move |intent| match intent {
                            LiveIntent::Select(selection) => selected.set(selection),
                            LiveIntent::Point(selection) => studio.pointed.set(selection),
                        },
                    }
                }
                div {
                    class: if workbench() == Activity::Data { "h-full" } else { "hidden h-full" },
                    DataWorkbench {
                        dataset: dataset.clone(),
                        query_available: spiral_query_available,
                        tab: data_tab,
                        sql: data_sql,
                        max_rows: data_max_rows,
                        query: data_query,
                        on_intent: move |next| {
                            let current_episode = studio.selected_episode.peek().clone();
                            if let Some(selection) = data_episode_selection_change(
                                current_episode.as_deref(),
                                &next,
                            ) {
                                studio.selected_episode.set(selection);
                            }
                            if let DataIntent::Episode(episode) = &next {
                                let changed = data_intent
                                    .peek()
                                    .as_ref()
                                    .and_then(|current| match current {
                                        DataIntent::Episode(current) => Some(current.id.as_str()),
                                        _ => None,
                                }) != Some(episode.id.as_str());
                                if changed {
                                    playhead.set(0.0);
                                    preview_playing.set(false);
                                    preview_target.set(None);
                                    follow_live.set(false);
                                    interactive_preview_target.set(None);
                                    request_historical_preview(
                                        studio,
                                        episode.id.clone(),
                                        0.0,
                                        historical_state,
                                        historical_revision,
                                    );
                                }
                            }
                            data_intent.set(Some(next));
                        },
                    }
                }
            }
        },
    )
    .with_class("workbench-editor");
    let mut registered_panels = Vec::with_capacity(dock_panels.len() + 2);
    registered_panels.push((workbench_panel, PanelRole::Workbench));
    registered_panels.extend(dock_panels);
    registered_panels.push((
        Panel::new("timeline", "Timeline", "timeline", timeline_panel)
            .with_class("timeline-editor"),
        PanelRole::Timeline,
    ));
    let panel_roles = registered_panels
        .iter()
        .map(|(panel, role)| (panel.id.clone(), role.clone()))
        .collect::<Vec<_>>();
    let workspace_panels = registered_panels
        .into_iter()
        .map(|(panel, _)| panel)
        .collect::<Vec<_>>();
    let initial_panel_layout = default_panel_layout(&workspace_panels);
    // v5 establishes the independent camera-row default while retaining one
    // durable ownership scope per workbench mode. Later user docking persists
    // inside that scope instead of following the current Data selection.
    let panel_storage_key = format!("rerobot.panel-layout.v5.{dock_scope}");

    rsx! {
        document::Stylesheet { href: TAILWIND }
        document::Title { "rerobot studio" }
        document::Link { rel: "icon", href: FAVICON }
        document::Meta { name: "color-scheme", content: "light" }

        div {
            class: "studio-shell studio-shell-v2",
            tabindex: "-1",
            onkeydown: move |event| {
                if event.key() == Key::Escape && settings_open() {
                    settings_open.set(false);
                    return;
                }
                if settings_open() {
                    return;
                }
                if event.modifiers().ctrl() || event.modifiers().meta() || event.modifiers().alt() {
                    return;
                }
                if !crate::commands::shortcuts_enabled() {
                    return;
                }
                let recording = studio.recording();
                match event.key() {
                    Key::Character(pressed) if pressed.eq_ignore_ascii_case("r") => {
                        event.prevent_default();
                        match recording {
                            Some(recording) => crate::commands::stop_recording(
                                studio,
                                &recording.episode,
                                true,
                                EventHandler::new(move |summary: rerobot_core::api::EpisodeSummary| {
                                    live_review_episode.set(Some(summary.id));
                                }),
                            ),
                            None if workbench() == Activity::Live => {
                                crate::commands::start_recording(studio)
                            }
                            None => studio.report("Switch to Live to start recording."),
                        }
                    }
                    // Only meaningful mid-take: this is the "that one was bad"
                    // key, pressed while the arm is still moving.
                    Key::Character(pressed)
                        if pressed.eq_ignore_ascii_case("x") && recording.is_some() =>
                    {
                        event.prevent_default();
                        if let Some(recording) = recording {
                            crate::commands::stop_recording(
                                studio,
                                &recording.episode,
                                false,
                                EventHandler::new(move |_| live_review_episode.set(None)),
                            );
                        }
                    }
                    _ => {}
                }
            },
            if settings_open() {
                SettingsPanel {
                    on_close: move |_| settings_open.set(false),
                    on_saved: move |_| *settings_revision.write() += 1,
                }
            }
            main { class: "studio-v2-body",
                ActivityRail {
                    activity: workbench(),
                    on_activity: move |next| {
                        preview_playing.set(false);
                        preview_target.set(None);
                        workbench.set(next);
                    },
                    on_settings: move |_| settings_open.set(true),
                }
                PanelWorkspace {
                    key: "{dock_scope}",
                    panels: workspace_panels,
                    initial_layout: initial_panel_layout,
                    storage_key: Some(panel_storage_key),
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
                            selected.set(Some(crate::state::Selection::Camera {
                                robot: robot.clone(),
                                role: role.clone(),
                            }));
                        }
                    },
                }
            }
            StatusBar { mode: workbench() }
            NoticeStrip {}
        }
    }
}

/// The canonical first-open arrangement. Persistence takes over after this,
/// while panel `home` ids tell hot-plugged content where it belongs.
fn default_panel_layout(panels: &[Panel]) -> PanelLayout {
    let ids_in = |home: &str| {
        panels
            .iter()
            .filter(|panel| panel.home.as_str() == home)
            .map(|panel| panel.id.clone())
            .collect::<Vec<_>>()
    };
    let primary = LayoutNode::tile("primary", ids_in("primary"));
    let cameras = ids_in("cameras");
    let editors = if let Some(camera_stack) = default_camera_stack(&cameras) {
        LayoutNode::split(
            "editor-columns",
            SplitAxis::Horizontal,
            2.0 / 3.0,
            primary,
            camera_stack,
        )
    } else {
        primary
    };
    let visualization = LayoutNode::split(
        "visualization-rows",
        SplitAxis::Vertical,
        0.58,
        editors,
        LayoutNode::tile("timeline", ids_in("timeline")),
    );
    PanelLayout::new(LayoutNode::split(
        "studio-columns",
        SplitAxis::Horizontal,
        1.0 / 3.0,
        LayoutNode::tile("workbench", ids_in("workbench")),
        visualization,
    ))
}

/// Give every camera its own visible row on first open. The horizontal
/// dividers preserve a useful 16:9 width in the right-hand camera column; the
/// panel workspace still owns every later tab, dock, and resize mutation.
fn default_camera_stack(cameras: &[PanelId]) -> Option<LayoutNode> {
    fn build(cameras: &[PanelId], offset: usize) -> LayoutNode {
        let tile_id = if offset == 0 {
            "cameras".to_owned()
        } else {
            format!("cameras-{}", offset + 1)
        };
        let first = LayoutNode::tile(tile_id, [cameras[0].clone()]);
        if cameras.len() == 1 {
            return first;
        }

        LayoutNode::split(
            format!("camera-rows-{}", offset + 1),
            SplitAxis::Vertical,
            1.0 / cameras.len() as f64,
            first,
            build(&cameras[1..], offset + 1),
        )
    }

    (!cameras.is_empty()).then(|| build(cameras, 0))
}

/// Return a selected-episode mutation only when a Data intent actually changes
/// semantic selection. Writing the same value would wake the workbench effect
/// that produced the intent and create a client-side render loop.
fn data_episode_selection_change(
    current: Option<&str>,
    intent: &DataIntent,
) -> Option<Option<String>> {
    match intent {
        DataIntent::Episode(episode) if current != Some(episode.id.as_str()) => {
            Some(Some(episode.id.clone()))
        }
        DataIntent::EmptyEpisodes if current.is_some() => Some(None),
        DataIntent::Episode(_) | DataIntent::Query(_) | DataIntent::EmptyEpisodes => None,
    }
}

#[component]
fn ActivityRail(
    activity: Activity,
    on_activity: EventHandler<Activity>,
    on_settings: EventHandler<()>,
) -> Element {
    rsx! {
        nav { class: "studio-activity-rail", "aria-label": "Application",
            button {
                r#type: "button",
                class: if activity == Activity::Live { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                "aria-pressed": activity == Activity::Live,
                onclick: move |_| on_activity.call(Activity::Live),
                RobotPanelIcon {}
                span { "Live" }
            }
            button {
                r#type: "button",
                class: if activity == Activity::Data { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                "aria-pressed": activity == Activity::Data,
                onclick: move |_| on_activity.call(Activity::Data),
                DataIcon {}
                span { "Data" }
            }
            button {
                r#type: "button",
                class: "activity-rail-item activity-rail-settings",
                "aria-label": "Settings",
                title: "Settings",
                onclick: move |_| on_settings.call(()),
                SettingsIcon {}
                span { "Settings" }
            }
        }
    }
}

/// Camera panels render exactly one declared source. A recorded camera never
/// falls back to the live device when its clip is missing or still loading.
#[derive(Clone, Debug, PartialEq)]
enum CameraEditorSource {
    Live {
        robot: String,
        role: String,
        device: String,
    },
    Recorded(RecordedSource<HistoricalCamera>),
    Empty {
        title: String,
        detail: String,
    },
}

#[component]
fn CameraEditor(element_id: String, source: CameraEditorSource, playing: bool) -> Element {
    let content = match source {
        CameraEditorSource::Live {
            robot,
            role,
            device,
        } => rsx! {
            LiveCameraFrame { key: "{device}", robot, role, device }
        },
        CameraEditorSource::Recorded(source) => {
            if let Some(recorded) = source.renderable().cloned() {
                let updating = source.state.is_loading();
                let warning = source.state.reason().map(str::to_owned);
                rsx! {
                    div { class: "camera-editor-meta",
                        span {
                            if recorded.approximate { "{recorded.label} · Recorded · approximate sync" }
                            else if updating { "{recorded.label} · Recorded · updating…" }
                            else { "{recorded.label} · Recorded" }
                        }
                        span {
                            class: "status-dot",
                            style: if recorded.approximate || warning.is_some() { "background: var(--tone-caution)" } else { "background: var(--tone-good)" },
                        }
                    }
                    div { class: "camera-editor-frame",
                        video {
                            key: "{recorded.url}",
                            id: "{element_id}",
                            class: "h-full w-full object-contain",
                            src: "{recorded.url}",
                            preload: "auto",
                            autoplay: playing,
                            muted: true,
                            playsinline: true,
                            onloadedmetadata: move |_| sync_recorded_camera(&element_id, recorded.video_seconds, playing),
                            "aria-label": "Recorded {recorded.label} camera at {format_elapsed_status(recorded.episode_seconds)}",
                            "Your browser cannot preview this recorded camera stream."
                        }
                        if let Some(ref warning) = warning {
                            div { class: "camera-live-error", role: "alert", span { "{warning}" } }
                        }
                    }
                }
            } else {
                let (title, detail) = match &source.state {
                    LoadState::Loading { .. } => (
                        "Loading recorded camera",
                        "The recorded frame will appear here when the episode source is ready."
                            .to_owned(),
                    ),
                    LoadState::Unavailable { reason } | LoadState::Failed { reason, .. } => {
                        ("Recorded camera unavailable", reason.clone())
                    }
                    LoadState::Ready(_) => (
                        "No recorded camera",
                        "This episode has no local video for this camera.".to_owned(),
                    ),
                };
                rsx! {
                    div { class: "editor-empty",
                        strong { "{title}" }
                        span { "{detail}" }
                    }
                }
            }
        }
        CameraEditorSource::Empty { title, detail } => rsx! {
            div { class: "editor-empty",
                strong { "{title}" }
                span { "{detail}" }
            }
        },
    };

    rsx! { div { class: "camera-editor-content", {content} } }
}

#[component]
fn LiveCameraFrame(robot: String, role: String, device: String) -> Element {
    let studio = crate::state::studio();
    let status = studio
        .snapshot
        .read()
        .cameras
        .iter()
        .find(|camera| camera.id == device)
        .cloned();
    // Safari can fire image lifecycle events repeatedly for an MJPEG resource.
    // Mutating component state from those events made Dioxus patch the same
    // live image element while WebKit was painting it, which presented as a flash. The
    // backend camera status is already authoritative and lets the image node
    // remain completely stable for the life of this panel.
    let problem = match status {
        None => Some("This assigned camera is not currently detected.".to_owned()),
        Some(status) => status.error,
    };

    rsx! {
        div { class: "camera-editor-frame camera-live-frame",
            img {
                class: "camera-stream-image",
                src: crate::api::camera_stream_url(&device),
                alt: "Live view from the {role} camera on {robot}",
                loading: "eager",
                decoding: "sync",
                draggable: "false",
            }
            if let Some(problem) = problem {
                div { class: "camera-live-error", role: "alert",
                    strong { "Camera stream unavailable" }
                    span { "{problem} Open Settings to rescan or assign another camera." }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct HistoricalPreview {
    episode: String,
    timeline: crate::api::EpisodeTimelineResponse,
    sample: crate::api::TimelineSample,
    signal_lanes: Option<[TimelineSignalLane; 6]>,
}

#[derive(Clone, Debug, PartialEq)]
struct HistoricalCamera {
    label: String,
    url: String,
    video_seconds: f64,
    episode_seconds: f64,
    approximate: bool,
}

fn historical_camera(
    preview: &HistoricalPreview,
    selected: Option<&(String, String, String)>,
) -> Option<HistoricalCamera> {
    let expected = selected.map(|(robot, role, _)| format!("{robot}.camera.{role}"));
    let stream_id = if let Some(expected) = expected.as_deref() {
        preview
            .timeline
            .videos
            .iter()
            .find(|asset| asset.stream_id == expected)
            .map(|asset| asset.stream_id.as_str())?
    } else {
        preview
            .timeline
            .videos
            .first()
            .map(|asset| asset.stream_id.as_str())?
    };
    let label = selected
        .filter(|(robot, role, _)| stream_id == format!("{robot}.camera.{role}"))
        .map(|(_, role, _)| role.clone())
        .or_else(|| stream_id.rsplit('.').next().map(str::to_owned))
        .unwrap_or_else(|| "recorded".to_owned());
    historical_camera_for_stream(preview, stream_id, &label)
}

fn recorded_scene_source(
    subject: &TimelineSubject,
    source: Option<&RecordedSource<HistoricalPreview>>,
) -> crate::viewport::SceneSource {
    let Some(episode) = subject.recorded_episode() else {
        return crate::viewport::SceneSource::Live;
    };
    let source = source
        .filter(|source| source.episode == episode)
        .cloned()
        .map(|source| source.map(|preview| preview.sample))
        .unwrap_or_else(|| RecordedSource {
            episode: episode.to_owned(),
            seconds: 0.0,
            state: LoadState::Loading { previous: None },
        });
    crate::viewport::SceneSource::Recorded(source)
}

fn map_recorded_camera(
    episode: &str,
    source: Option<&RecordedSource<HistoricalPreview>>,
    select: impl Fn(&HistoricalPreview) -> Option<HistoricalCamera>,
) -> RecordedSource<HistoricalCamera> {
    let Some(source) = source.filter(|source| source.episode == episode) else {
        return RecordedSource {
            episode: episode.to_owned(),
            seconds: 0.0,
            state: LoadState::Loading { previous: None },
        };
    };
    let state = match &source.state {
        LoadState::Loading { previous } => LoadState::Loading {
            previous: previous.as_ref().and_then(&select),
        },
        LoadState::Ready(preview) => select(preview).map_or_else(
            || LoadState::Unavailable {
                reason: "This episode has no local video for this camera.".to_owned(),
            },
            LoadState::Ready,
        ),
        LoadState::Unavailable { reason } => LoadState::Unavailable {
            reason: reason.clone(),
        },
        LoadState::Failed { reason, previous } => LoadState::Failed {
            reason: reason.clone(),
            previous: previous.as_ref().and_then(select),
        },
    };
    RecordedSource {
        episode: source.episode.clone(),
        seconds: source.seconds,
        state,
    }
}

fn recorded_camera_source_for_live_assignment(
    episode: &str,
    source: Option<&RecordedSource<HistoricalPreview>>,
    camera: &(String, String, String),
) -> CameraEditorSource {
    CameraEditorSource::Recorded(map_recorded_camera(episode, source, |preview| {
        historical_camera(preview, Some(camera))
    }))
}

fn recorded_camera_source_for_stream(
    episode: &str,
    source: Option<&RecordedSource<HistoricalPreview>>,
    stream_id: &str,
    label: &str,
) -> CameraEditorSource {
    CameraEditorSource::Recorded(map_recorded_camera(episode, source, |preview| {
        historical_camera_for_stream(preview, stream_id, label)
    }))
}

fn recorded_camera_registry_placeholder(
    episode: &str,
    source: Option<&RecordedSource<HistoricalPreview>>,
) -> CameraEditorSource {
    let (title, detail) = match source
        .filter(|source| source.episode == episode)
        .map(|source| &source.state)
    {
        None | Some(LoadState::Loading { .. }) => (
            "Loading recorded cameras",
            "The camera tabs will appear when the episode source is ready.".to_owned(),
        ),
        Some(LoadState::Ready(_)) | Some(LoadState::Unavailable { .. }) => (
            "No recorded camera",
            "This episode contains no local camera video.".to_owned(),
        ),
        Some(LoadState::Failed { reason, .. }) => ("Recorded cameras unavailable", reason.clone()),
    };
    CameraEditorSource::Empty {
        title: title.to_owned(),
        detail,
    }
}

fn recorded_camera_streams(preview: &HistoricalPreview) -> Vec<(String, String)> {
    let mut streams = Vec::new();
    for asset in &preview.timeline.videos {
        if streams
            .iter()
            .any(|(stream_id, _)| stream_id == &asset.stream_id)
        {
            continue;
        }
        let label = asset
            .stream_id
            .rsplit('.')
            .next()
            .filter(|label| !label.is_empty())
            .unwrap_or("camera")
            .to_owned();
        streams.push((asset.stream_id.clone(), label));
    }
    streams
}

fn historical_camera_for_stream(
    preview: &HistoricalPreview,
    stream_id: &str,
    label: &str,
) -> Option<HistoricalCamera> {
    let assets = preview
        .timeline
        .videos
        .iter()
        .filter(|asset| asset.stream_id == stream_id)
        .collect::<Vec<_>>();
    let mut exact_assets = assets
        .iter()
        .copied()
        .filter(|asset| asset.timeline_offset_seconds.is_some())
        .collect::<Vec<_>>();
    exact_assets.sort_by(|left, right| {
        left.timeline_offset_seconds
            .unwrap_or_default()
            .total_cmp(&right.timeline_offset_seconds.unwrap_or_default())
    });
    let at = preview.sample.seconds.max(0.0);
    let asset = if exact_assets.is_empty() {
        assets
            .iter()
            .copied()
            .min_by_key(|asset| asset.segment_index)?
    } else {
        exact_assets
            .iter()
            .copied()
            .filter(|asset| {
                let start = asset.timeline_offset_seconds.unwrap_or_default();
                let end = start + asset.duration_seconds.max(0.0);
                start <= at && at < end
            })
            .max_by(|left, right| {
                left.timeline_offset_seconds
                    .unwrap_or_default()
                    .total_cmp(&right.timeline_offset_seconds.unwrap_or_default())
            })
            .or_else(|| {
                exact_assets.iter().copied().min_by(|left, right| {
                    video_segment_distance(left, at).total_cmp(&video_segment_distance(right, at))
                })
            })?
    };
    let label = label.to_owned();
    let episode_duration = preview.timeline.episode.duration_seconds.max(0.0);
    let (video_seconds, approximate) = if let Some(offset) = asset.timeline_offset_seconds {
        (
            (preview.sample.seconds - offset).clamp(0.0, asset.duration_seconds.max(0.0)),
            false,
        )
    } else {
        let ratio = if episode_duration > 0.0 {
            (preview.sample.seconds / episode_duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (ratio * asset.duration_seconds.max(0.0), true)
    };
    let url =
        crate::api::episode_video_url(&preview.episode, &asset.stream_id, asset.segment_index);
    Some(HistoricalCamera {
        label,
        url,
        video_seconds,
        episode_seconds: preview.sample.seconds,
        approximate,
    })
}

fn video_segment_distance(asset: &crate::api::VideoAssetMetadata, at: f64) -> f64 {
    let start = asset.timeline_offset_seconds.unwrap_or_default();
    let end = start + asset.duration_seconds.max(0.0);
    if at < start {
        start - at
    } else if at > end {
        at - end
    } else {
        0.0
    }
}

fn historical_replay_slot(seconds: f64) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * HISTORICAL_RENDER_HZ).floor() as u64
}

/// A one-frame replay is the controller's bounded implementation of a
/// committed physical seek. It is a consequence of the Timeline playhead, not
/// a transport that may replace or reposition that playhead while the arm is
/// still travelling to the requested pose.
fn replay_owns_timeline_transport(replay: &rerobot_core::api::ReplayProgress) -> bool {
    replay.frames > 1
}

fn timeline_target_episode(target: &TimelineTarget) -> &str {
    match target {
        TimelineTarget::Recording(episode) | TimelineTarget::Episode(episode) => episode,
    }
}

fn sync_recorded_camera(element_id: &str, video_seconds: f64, should_play: bool) {
    #[cfg(feature = "web")]
    {
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
        let seek_threshold = if should_play { 0.25 } else { 0.005 };
        if (current - target).abs() > seek_threshold {
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
fn StatusBar(mode: Activity) -> Element {
    let studio = crate::state::studio();
    let snapshot = studio.snapshot.read().clone();
    let telemetry = studio.telemetry.read().clone();
    let connected = studio.controls_available();
    let connected_arms = snapshot
        .robots
        .iter()
        .filter(|robot| robot.connection.connected)
        .count();
    let holding = telemetry.robots.iter().filter(|robot| robot.torque).count();
    let mode_label = snapshot.mode.selection.label();
    let status_mode = format!("{} · {mode_label}", mode.label());
    let arms_label = format!("{connected_arms}/{} arms", snapshot.robots.len());
    let problems_label = format!("{} problems", snapshot.issues.len());
    let rate_label = format!("{:.0}/{:.0} Hz", telemetry.loop_hz, telemetry.target_hz);
    let recording_label = telemetry.recording.as_ref().map(|recording| {
        format!(
            "Recording {}",
            format_elapsed_status(recording.elapsed_seconds)
        )
    });

    rsx! {
        footer { class: "studio-status-bar",
            div { class: "status-bar-group",
                span { class: "status-bar-item",
                    span { class: "status-dot", style: if connected { "background: var(--tone-good)" } else { "background: var(--tone-danger)" } }
                    if connected { "Connected" } else { "Reconnecting" }
                }
                span { class: "status-bar-item", if snapshot.simulated { "Motors simulated" } else { "Physical hardware" } }
                span { class: "status-bar-item", "{arms_label}" }
            }
            div { class: "status-bar-group status-bar-group-right",
                span { class: "status-bar-item status-bar-accent", "{status_mode}" }
                if let Some(recording_label) = recording_label {
                    span { class: "status-bar-item status-bar-danger", "{recording_label}" }
                }
                if holding > 0 {
                    span { class: "status-bar-item status-bar-danger", "{holding} holding" }
                }
                if telemetry.target_hz > 0.0 {
                    span { class: "status-bar-item", "{rate_label}" }
                }
                span { class: "status-bar-item", "{problems_label}" }
            }
        }
    }
}

fn assigned_cameras() -> Vec<(String, String, String)> {
    let studio = crate::state::studio();
    let Some(focused) = studio.focused() else {
        return Vec::new();
    };
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
    let record = if activity == Activity::Data {
        TimelineGate::blocked("Switch to Live to start recording.")
    } else if let Some(reason) = crate::commands::recording_blocker(studio) {
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
        Some(episode) => crate::commands::replay_blocker(studio, episode)
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

fn request_historical_preview(
    studio: Studio,
    episode: String,
    seconds: f64,
    mut state: Signal<Option<RecordedSource<HistoricalPreview>>>,
    mut revision: Signal<u64>,
) {
    let ticket = revision.peek().wrapping_add(1);
    revision.set(ticket);
    let previous = state
        .peek()
        .as_ref()
        .filter(|source| source.episode == episode)
        .and_then(RecordedSource::renderable)
        .cloned();
    let existing_timeline = previous.as_ref().map(|preview| preview.timeline.clone());
    let existing_signal_lanes = previous
        .as_ref()
        .and_then(|preview| preview.signal_lanes.clone());
    let should_load_traces = existing_timeline.is_none() && existing_signal_lanes.is_none();
    state.set(Some(RecordedSource {
        episode: episode.clone(),
        seconds,
        state: LoadState::Loading {
            previous: previous.clone(),
        },
    }));
    spawn(async move {
        let timeline = match existing_timeline {
            Some(timeline) => timeline,
            None => {
                let timeline_path = crate::api::episode_timeline_url(&episode);
                match crate::api::get::<crate::api::EpisodeTimelineResponse>(&timeline_path).await {
                    Ok(timeline) => timeline,
                    Err(error) => {
                        if revision.peek().eq(&ticket) {
                            state.set(Some(RecordedSource {
                                episode: episode.clone(),
                                seconds,
                                state: LoadState::Failed {
                                    reason: error.clone(),
                                    previous: previous.clone(),
                                },
                            }));
                            studio.report(error);
                        }
                        return;
                    }
                }
            }
        };
        if !revision.peek().eq(&ticket) {
            return;
        }
        let sample_path = crate::api::timeline_sample_url(&episode, seconds);
        match crate::api::get::<crate::api::TimelineSample>(&sample_path).await {
            Ok(sample) if revision.peek().eq(&ticket) => {
                state.set(Some(RecordedSource {
                    episode: episode.clone(),
                    seconds,
                    state: LoadState::Ready(HistoricalPreview {
                        episode: episode.clone(),
                        timeline,
                        sample,
                        signal_lanes: existing_signal_lanes.clone(),
                    }),
                }));
                if should_load_traces {
                    match load_recorded_timeline_traces(&episode, 240).await {
                        Ok(traces) => {
                            if let Some(source) = state.write().as_mut()
                                && source.episode == episode
                                && let LoadState::Ready(preview) = &mut source.state
                            {
                                preview.signal_lanes = Some(traces.signal_lanes());
                            }
                        }
                        Err(error) if revision.peek().eq(&ticket) => studio.report(error),
                        Err(_) => {}
                    }
                }
            }
            Ok(_) => {}
            Err(error) if revision.peek().eq(&ticket) => {
                state.set(Some(RecordedSource {
                    episode: episode.clone(),
                    seconds,
                    state: LoadState::Failed {
                        reason: error.clone(),
                        previous,
                    },
                }));
                studio.report(error);
            }
            Err(_) => {}
        }
    });
}

fn format_elapsed_status(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor() as u64;
    let remainder = seconds - minutes as f64 * 60.0;
    format!("{minutes}:{remainder:04.1}")
}

/// A notice floats over the shell until it ages out — see [`crate::state::Notice`],
/// which carries its own deadline, and the studio clock, which enforces it.
#[component]
fn NoticeStrip() -> Element {
    let studio = crate::state::studio();
    let mut notice = studio.notice;
    let Some(current) = notice.read().clone() else {
        return rsx! {};
    };

    rsx! {
        div { class: "notice-positioner",
            div {
                class: "notice-card", role: if current.failed { "alert" } else { "status" },
                "aria-live": if current.failed { "assertive" } else { "polite" },
                span { class: "status-dot", style: if current.failed { "background: var(--tone-danger)" } else { "background: var(--tone-good)" } }
                span { class: "flex-1 text-[12px]", "{current.text}" }
                button { r#type: "button", class: "link", onclick: move |_| notice.set(None), "Dismiss" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{
        EpisodeTimelineResponse, TimelineMetadata, TimelineSample, VideoAssetMetadata,
    };
    use rerobot_core::api::EpisodeSummary;

    fn preview(at: f64, videos: Vec<VideoAssetMetadata>) -> HistoricalPreview {
        HistoricalPreview {
            episode: "take-1".to_owned(),
            timeline: EpisodeTimelineResponse {
                episode: EpisodeSummary {
                    id: "take-1".to_owned(),
                    file: "take-1.jsonl".to_owned(),
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
            rerobot_panels::PanelPlacement::new("workbench", "workbench"),
            rerobot_panels::PanelPlacement::new("episode-scene", "primary"),
            rerobot_panels::PanelPlacement::new("episode-camera:wrist", "cameras"),
            rerobot_panels::PanelPlacement::new("timeline", "timeline"),
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
            rerobot_panels::PanelPlacement::new("workbench", "workbench"),
            rerobot_panels::PanelPlacement::new("episode-scene", "primary"),
            rerobot_panels::PanelPlacement::new("query-result", "primary"),
            rerobot_panels::PanelPlacement::new("episode-camera:wrist", "cameras"),
            rerobot_panels::PanelPlacement::new("timeline", "timeline"),
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
        let mut replay = rerobot_core::api::ReplayProgress {
            episode: "take-1".to_owned(),
            robot: "follower".to_owned(),
            frame: 0,
            frames: 1,
            elapsed_seconds: 0.0,
            duration_seconds: 1.0 / 60.0,
            paused: false,
        };

        assert!(!replay_owns_timeline_transport(&replay));
        replay.frames = 2;
        assert!(replay_owns_timeline_transport(&replay));
    }

    #[test]
    fn repeated_data_intents_do_not_rewrite_their_own_selection_dependency() {
        let episode = preview(0.0, Vec::new()).timeline.episode;

        assert_eq!(
            data_episode_selection_change(
                Some(episode.id.as_str()),
                &DataIntent::Episode(episode.clone()),
            ),
            None
        );
        assert_eq!(
            data_episode_selection_change(None, &DataIntent::EmptyEpisodes),
            None
        );
        assert_eq!(
            data_episode_selection_change(None, &DataIntent::Episode(episode.clone())),
            Some(Some(episode.id))
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
            crate::viewport::SceneSource::Recorded(RecordedSource {
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

        let source = recorded_camera_source_for_live_assignment("take-1", None, &assigned);

        assert!(matches!(
            source,
            CameraEditorSource::Recorded(RecordedSource {
                state: LoadState::Loading { previous: None },
                ..
            })
        ));
    }
}
