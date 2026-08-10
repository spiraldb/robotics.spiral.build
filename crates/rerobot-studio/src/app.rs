//! The studio shell: global session rails around a persisted tiled workbench.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{
    DataSettings, ModeSelection, SetRecordingPausedRequest, SetReplayPausedRequest,
    StartReplayRequest,
};
use rerobot_panels::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};

use crate::data_panel::{DataIcon, DataSettingsPanel};
use crate::data_workbench::{
    DEFAULT_QUERY_ROWS, DataVisualization, DataWorkbench, DataWorkbenchTab, QueryResultView,
    QueryViewModel, default_query_sql,
};
use crate::icons::RobotPanelIcon;
use crate::live_workbench::LiveWorkbench;
use crate::state::Studio;
use crate::studio_timeline::{
    RecordedTimelineTraces, StudioTimeline, StudioTimelineState, TimelineMotionGate,
    TimelineSeekRequest, TimelineSignalLane, TimelineSource, TimelineVideoLane,
    TimelineVideoPreview, TimelineVideoSpan, load_recorded_timeline_traces,
    signal_lanes_from_store,
};

#[cfg(feature = "web")]
use wasm_bindgen::{JsCast as _, JsValue};

const TAILWIND: Asset = asset!("/assets/tailwind.css");
const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='7' fill='%2337352f'/%3E%3Cpath d='M9 23V13l7-5 7 5v10' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E";
const HISTORICAL_REPLAY_PREVIEW_HZ: f64 = 10.0;

#[component]
pub fn App() -> Element {
    Studio::use_studio();
    crate::state::use_description();
    let studio = crate::state::studio();
    let mut workbench = use_signal(WorkbenchMode::default);
    let mut settings_open = use_signal(|| false);
    let mut about_open = use_signal(|| false);
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
    let mut data_visualization = use_signal(|| None::<DataVisualization>);
    let initial_query_dataset = dataset.clone();
    let data_tab = use_signal(DataWorkbenchTab::default);
    let data_sql = use_signal(move || default_query_sql(&initial_query_dataset));
    let data_max_rows = use_signal(|| DEFAULT_QUERY_ROWS);
    let data_query = use_signal(|| QueryViewModel::idle(data_sql(), data_max_rows()));
    let mut playhead = use_signal(|| 0.0_f64);
    let mut playing = use_signal(|| false);
    let mut follow_live = use_signal(|| true);
    let mut move_arm_on_release = use_signal(|| false);
    let mut historical_state = use_signal(|| None::<HistoricalPreview>);
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
        let preview = historical_state();
        let should_play = playing();
        let Some(preview) = preview else { return };

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
        if let Some(recorded) =
            historical_camera(&preview, selected_camera(&assigned_cameras()).as_ref())
                .or_else(|| historical_camera(&preview, None))
        {
            sync_recorded_camera(
                "timeline-camera-preview",
                recorded.video_seconds,
                should_play,
            );
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

    // Replay telemetry may arrive at the controller rate, but a local sample
    // request at that rate would add fetch churn without making the canvas
    // perceptibly smoother. Ten committed previews per second are enough for
    // motion continuity; the browser-native video clock runs between them.
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
            playing.set(!recording.paused);
        } else if let Some(replay) = studio.replay() {
            playhead.set(replay.elapsed_seconds);
            playing.set(!replay.paused);
        } else if playing() {
            playing.set(false);
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
                Some(episode) if workbench() == WorkbenchMode::Live => (episode, 0.0),
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
    let data_review = studio.focused_episode();
    let source = match workbench() {
        WorkbenchMode::Live => studio
            .recording()
            .map(TimelineSource::Recording)
            .or_else(|| live_review.map(TimelineSource::Review)),
        WorkbenchMode::Data => data_review.map(TimelineSource::Review),
    };
    let live_signal_lanes = {
        let store = studio.signals.read();
        signal_lanes_from_store(&store)
    };
    let assigned_cameras = assigned_cameras();
    let selected_camera = selected_camera(&assigned_cameras);
    let review_episode = source.as_ref().and_then(|source| match source {
        TimelineSource::Review(episode) => Some(episode.id.clone()),
        TimelineSource::Recording(recording) if recording.paused => Some(recording.episode.clone()),
        TimelineSource::Recording(_) => None,
    });
    let historical = historical_state
        .read()
        .clone()
        .filter(|preview| Some(&preview.episode) == review_episode.as_ref());
    let history_active = review_episode.is_some();
    let signal_lanes = match historical.as_ref() {
        Some(preview) => preview
            .signal_lanes
            .clone()
            .unwrap_or_else(|| RecordedTimelineTraces::default().signal_lanes()),
        None if history_active => RecordedTimelineTraces::default().signal_lanes(),
        None => live_signal_lanes,
    };
    let primary_historical_camera = historical.as_ref().and_then(|preview| {
        historical_camera(preview, selected_camera.as_ref())
            .or_else(|| historical_camera(preview, None))
    });
    let video = TimelineVideoLane {
        label: primary_historical_camera
            .as_ref()
            .map(|camera| format!("Camera · {}", camera.label))
            .or_else(|| {
                selected_camera
                    .as_ref()
                    .map(|(_, role, _)| format!("Camera · {role}"))
            })
            .unwrap_or_else(|| "Camera".to_owned()),
        spans: primary_historical_camera
            .as_ref()
            .map(|camera| camera.spans.clone())
            .or_else(|| {
                source
                    .as_ref()
                    .filter(|source| source.is_recording() && selected_camera.is_some())
                    .map(|source| {
                        vec![TimelineVideoSpan {
                            start_seconds: 0.0,
                            end_seconds: source.duration_seconds(),
                            label: selected_camera.as_ref().map(|(_, role, _)| role.clone()),
                        }]
                    })
            })
            .unwrap_or_default(),
        preview: primary_historical_camera
            .as_ref()
            .map(|camera| TimelineVideoPreview {
                url: camera.url.clone(),
                video_seconds: camera.video_seconds,
                episode_seconds: camera.episode_seconds,
                label: camera.label.clone(),
                approximate: camera.approximate,
            }),
        empty_reason: Some(if history_active {
            "This episode has no local video for the selected camera.".to_owned()
        } else if selected_camera.is_some() {
            "Recorded video appears here when episode media is available.".to_owned()
        } else {
            "Assign a camera in Settings to record video.".to_owned()
        }),
    };
    let motion_gate = timeline_motion_gate(studio, source.as_ref());
    let seek_context = timeline_seek_context(studio, source.as_ref());
    let mut selected = studio.selected;

    let (dock_scope, dock_panels) = match workbench() {
        WorkbenchMode::Live => {
            let mut panels =
                vec![Panel::new(
                "scene",
                "Scene",
                "primary",
                rsx! {
                    crate::viewport::Viewport {
                        historical: historical.as_ref().map(|preview| preview.sample.clone()),
                        resize_revision: split_resize_revision(),
                    }
                },
            )
            .with_class("scene-editor")];
            for camera in &assigned_cameras {
                let id = camera_panel_id(camera);
                panels.push(
                    Panel::new(
                        id.clone(),
                        format!("Camera · {}", camera.1),
                        "cameras",
                        rsx! {
                            CameraEditor {
                                element_id: recorded_camera_element_id(&id),
                                camera: Some(camera.clone()),
                                historical: historical
                                    .as_ref()
                                    .and_then(|preview| historical_camera(preview, Some(camera))),
                                history_active,
                                live_owner: true,
                                playing: playing(),
                            }
                        },
                    )
                    .with_class("camera-editor"),
                );
            }
            if assigned_cameras.is_empty() {
                panels.push(
                    Panel::new(
                        "camera-empty",
                        "Camera",
                        "cameras",
                        rsx! {
                            CameraEditor {
                                element_id: "recorded-camera-empty".to_owned(),
                                camera: None,
                                historical: None,
                                history_active,
                                live_owner: true,
                                playing: playing(),
                            }
                        },
                    )
                    .with_class("camera-editor"),
                );
            }
            ("live".to_owned(), panels)
        }
        WorkbenchMode::Data => match data_visualization.read().as_ref() {
            Some(DataVisualization::Query(model)) => (
                "data".to_owned(),
                vec![
                    Panel::new(
                        "query-result",
                        "Query result",
                        "primary",
                        rsx! { QueryResultView { model: model.clone() } },
                    )
                    .with_class("query-editor"),
                    Panel::new(
                        "data-camera-context",
                        "Camera",
                        "cameras",
                        rsx! {
                            div { class: "editor-empty",
                                strong { "No episode in view" }
                                span { "Select an episode to inspect its recorded camera frames." }
                            }
                        },
                    )
                    .with_class("camera-editor"),
                ],
            ),
            Some(DataVisualization::Episode(episode)) => {
                let mut panels = vec![
                    Panel::new(
                        "episode-scene",
                        format!("Episode · {}", crate::timeline::short_id(&episode.id)),
                        "primary",
                        rsx! {
                            crate::viewport::Viewport {
                                historical: historical
                                    .as_ref()
                                    .map(|preview| preview.sample.clone()),
                                resize_revision: split_resize_revision(),
                            }
                        },
                    )
                    .with_class("scene-editor"),
                ];
                let recorded_cameras = historical
                    .as_ref()
                    .map(recorded_camera_streams)
                    .unwrap_or_default();
                for (stream_id, label) in &recorded_cameras {
                    let id = format!("episode-camera:{stream_id}");
                    let recorded = historical.as_ref().and_then(|preview| {
                        historical_camera_for_stream(preview, stream_id, label)
                    });
                    panels.push(
                        Panel::new(
                            id.clone(),
                            format!("Recorded · {label}"),
                            "cameras",
                            rsx! {
                                CameraEditor {
                                    element_id: recorded_camera_element_id(&id),
                                    camera: None,
                                    historical: recorded,
                                    history_active: true,
                                    live_owner: false,
                                    playing: playing(),
                                }
                            },
                        )
                        .with_class("camera-editor"),
                    );
                }
                if recorded_cameras.is_empty() {
                    let loading = historical.is_none();
                    panels.push(
                            Panel::new(
                                "episode-camera-empty",
                                "Camera",
                                "cameras",
                                rsx! {
                                    div { class: "editor-empty",
                                        if loading {
                                            strong { "Loading recorded cameras…" }
                                            span { "The camera tabs will appear in this group." }
                                        } else {
                                            strong { "No recorded camera" }
                                            span { "This episode contains joint samples but no local video." }
                                        }
                                    }
                                },
                            )
                            .with_class("camera-editor"),
                        );
                }
                ("data".to_owned(), panels)
            }
            Some(DataVisualization::EmptyEpisodes) | None => (
                "data".to_owned(),
                vec![
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
                    Panel::new(
                        "data-camera-context",
                        "Camera",
                        "cameras",
                        rsx! {
                            div { class: "editor-empty",
                                strong { "No episode in view" }
                                span { "Select an episode to inspect its recorded camera frames." }
                            }
                        },
                    )
                    .with_class("camera-editor"),
                ],
            ),
        },
    };
    let activation_cameras = assigned_cameras.clone();
    let timeline_panel = if let Some(source) = source.clone() {
        let timeline_episode = source.episode_id().to_owned();
        rsx! {
            StudioTimeline {
                state: StudioTimelineState {
                    source,
                    playhead_seconds: playhead(),
                    playing: playing(),
                    follow_live: follow_live(),
                    move_arm_on_release: move_arm_on_release(),
                },
                video: video.clone(),
                signal_lanes: signal_lanes.clone(),
                motion_gate: motion_gate.clone(),
                on_play_pause: move |next| {
                    playing.set(next);
                    if let Some(recording) = studio.recording() {
                        follow_live.set(next);
                        set_timeline_recording_paused(studio, !next);
                        if next && recording.paused {
                            historical_state.set(None);
                            let next_revision = historical_revision.peek().wrapping_add(1);
                            historical_revision.set(next_revision);
                        }
                    } else if studio
                        .replay()
                        .is_some_and(|replay| replay.episode == timeline_episode)
                    {
                        set_timeline_replay_paused(studio, !next);
                    } else if next {
                        start_timeline_playback(studio);
                    }
                },
                on_seek: move |request: TimelineSeekRequest| {
                    playhead.set(request.seconds);
                    if let Some(episode) = review_episode.clone() {
                        let target = (
                            episode.clone(),
                            historical_replay_slot(request.seconds),
                        );
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
                    if request.committed && request.move_arm {
                        if let Some((episode, robot, source_arm)) = seek_context.clone() {
                            request_physical_seek(
                                studio,
                                episode,
                                robot,
                                source_arm,
                                request.seconds,
                            );
                        }
                    }
                },
                on_follow_live: move |next| {
                    follow_live.set(next);
                    if next {
                        if let Some(recording) = studio.recording() {
                            playhead.set(recording.elapsed_seconds);
                        }
                    }
                },
                on_move_arm_on_release: move |next| move_arm_on_release.set(next),
            }
        }
    } else {
        rsx! { EmptyTimeline {} }
    };
    let workbench_panel = Panel::new(
        "workbench",
        workbench().label(),
        "workbench",
        rsx! {
            aside { class: "studio-v2-workbench",
                div {
                    class: if workbench() == WorkbenchMode::Live { "h-full" } else { "hidden h-full" },
                    LiveWorkbench {}
                }
                div {
                    class: if workbench() == WorkbenchMode::Data { "h-full" } else { "hidden h-full" },
                    DataWorkbench {
                        dataset: dataset.clone(),
                        query_available: spiral_query_available,
                        tab: data_tab,
                        sql: data_sql,
                        max_rows: data_max_rows,
                        query: data_query,
                        on_visualization: move |next| {
                            if let DataVisualization::Episode(episode) = &next {
                                let changed = data_visualization
                                    .peek()
                                    .as_ref()
                                    .and_then(|current| match current {
                                        DataVisualization::Episode(current) => Some(current.id.as_str()),
                                        _ => None,
                                    }) != Some(episode.id.as_str());
                                if changed {
                                    playhead.set(0.0);
                                    playing.set(false);
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
                            data_visualization.set(Some(next));
                        },
                    }
                }
            }
        },
    )
    .with_class("workbench-editor");
    let mut workspace_panels = Vec::with_capacity(dock_panels.len() + 2);
    workspace_panels.push(workbench_panel);
    workspace_panels.extend(dock_panels);
    workspace_panels.push(
        Panel::new("timeline", "Timeline", "timeline", timeline_panel)
            .with_class("timeline-editor"),
    );
    let initial_panel_layout = default_panel_layout(&workspace_panels);
    // v2 establishes one durable ownership scope per workbench mode. Earlier
    // keys varied with the current Data visualization and could make panels
    // appear to migrate between registries.
    let panel_storage_key = format!("rerobot.panel-layout.v2.{dock_scope}");

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
                if event.key() == Key::Escape && about_open() {
                    about_open.set(false);
                    return;
                }
                if settings_open() || about_open() {
                    return;
                }
                if event.modifiers().ctrl() || event.modifiers().meta() || event.modifiers().alt() {
                    return;
                }
                if !crate::timeline::shortcuts_enabled() {
                    return;
                }
                let recording = studio.recording();
                match event.key() {
                    Key::Character(pressed) if pressed.eq_ignore_ascii_case("r") => {
                        event.prevent_default();
                        match recording {
                            Some(_) => crate::timeline::stop_recording(studio, true),
                            None => crate::timeline::start_recording(studio),
                        }
                    }
                    // Only meaningful mid-take: this is the "that one was bad"
                    // key, pressed while the arm is still moving.
                    Key::Character(pressed)
                        if pressed.eq_ignore_ascii_case("x") && recording.is_some() =>
                    {
                        event.prevent_default();
                        crate::timeline::stop_recording(studio, false);
                    }
                    _ => {}
                }
            },
            StudioMenuBar {
                on_settings: move |_| {
                    about_open.set(false);
                    settings_open.set(true);
                },
                on_about: move |_| {
                    settings_open.set(false);
                    about_open.toggle();
                }
            }
            if settings_open() {
                DataSettingsPanel {
                    on_close: move |_| settings_open.set(false),
                    on_saved: move |_| *settings_revision.write() += 1,
                }
            }
            if about_open() {
                AboutPopover { on_close: move |_| about_open.set(false) }
            }
            main { class: "studio-v2-body",
                ActivityRail {
                    mode: workbench(),
                    on_mode: move |next| workbench.set(next),
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
                        if let Some((robot, role, _)) = activation_cameras
                            .iter()
                            .find(|camera| panel_id.as_str().ends_with(&camera_panel_id(camera)))
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum WorkbenchMode {
    #[default]
    Live,
    Data,
}

impl WorkbenchMode {
    const fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Data => "Data",
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
    let editors = if cameras.is_empty() {
        primary
    } else {
        LayoutNode::split(
            "editor-columns",
            SplitAxis::Horizontal,
            2.0 / 3.0,
            primary,
            LayoutNode::tile("cameras", cameras),
        )
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

#[component]
fn StudioMenuBar(on_settings: EventHandler<()>, on_about: EventHandler<()>) -> Element {
    rsx! {
        header { class: "studio-menu-bar",
            h1 { class: "studio-v2-brand", "rerobot studio" }
            nav { "aria-label": "Application menu",
                button { r#type: "button", onclick: move |_| on_settings.call(()), "Settings" }
                button { r#type: "button", onclick: move |_| on_about.call(()), "About" }
            }
        }
    }
}

#[component]
fn ActivityRail(mode: WorkbenchMode, on_mode: EventHandler<WorkbenchMode>) -> Element {
    rsx! {
        nav { class: "studio-activity-rail", "aria-label": "Operating mode",
            button {
                r#type: "button",
                class: if mode == WorkbenchMode::Live { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                "aria-pressed": mode == WorkbenchMode::Live,
                onclick: move |_| on_mode.call(WorkbenchMode::Live),
                RobotPanelIcon {}
                span { "Live" }
            }
            button {
                r#type: "button",
                class: if mode == WorkbenchMode::Data { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                "aria-pressed": mode == WorkbenchMode::Data,
                onclick: move |_| on_mode.call(WorkbenchMode::Data),
                DataIcon {}
                span { "Data" }
            }
        }
    }
}

#[component]
fn CameraEditor(
    element_id: String,
    camera: Option<(String, String, String)>,
    historical: Option<HistoricalCamera>,
    history_active: bool,
    live_owner: bool,
    playing: bool,
) -> Element {
    rsx! {
        div { class: "camera-editor-content",
            if let Some(recorded) = historical {
                div { class: "camera-editor-meta",
                    span {
                        if recorded.approximate { "{recorded.label} · Recorded · approximate sync" }
                        else { "{recorded.label} · Recorded" }
                    }
                    span {
                        class: "status-dot",
                        style: if recorded.approximate { "background: var(--tone-caution)" } else { "background: var(--tone-good)" },
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
                }
            } else if history_active && live_owner && camera.is_some() {
                if let Some((robot, role, device)) = camera {
                    LiveCameraFrame {
                        key: "{device}",
                        robot,
                        role,
                        device,
                        context: Some("Live · no recorded clip for this episode".to_owned()),
                    }
                }
            } else if history_active {
                div { class: "editor-empty",
                    strong { "No recorded camera" }
                    span { "This episode has no local video for this camera." }
                }
            } else if let Some((robot, role, device)) = camera {
                LiveCameraFrame {
                    key: "{device}",
                    robot,
                    role,
                    device,
                    context: None,
                }
            } else {
                div { class: "editor-empty",
                    strong { "No camera assigned" }
                    span { "Assign a camera to this arm in Settings." }
                }
            }
        }
    }
}

#[component]
fn LiveCameraFrame(
    robot: String,
    role: String,
    device: String,
    context: Option<String>,
) -> Element {
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
    let (state_label, state_color, problem) = match status {
        None => (
            "Camera unavailable".to_owned(),
            "var(--tone-danger)",
            Some("This assigned camera is not currently detected.".to_owned()),
        ),
        Some(status) if status.error.is_some() => (
            "Camera unavailable".to_owned(),
            "var(--tone-danger)",
            status.error,
        ),
        Some(status) if status.frames > 0 => (
            context.unwrap_or_else(|| format!("{role} · Live")),
            "var(--tone-good)",
            None,
        ),
        Some(_) => (format!("{role} · Connecting…"), "var(--tone-caution)", None),
    };

    rsx! {
        div { class: "camera-editor-meta", role: "status",
            span { "{state_label}" }
            span { class: "status-dot", style: "background: {state_color}" }
        }
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
    spans: Vec<TimelineVideoSpan>,
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
    let (video_seconds, approximate, start_seconds, end_seconds) =
        if let Some(offset) = asset.timeline_offset_seconds {
            (
                (preview.sample.seconds - offset).clamp(0.0, asset.duration_seconds.max(0.0)),
                false,
                offset.max(0.0),
                (offset + asset.duration_seconds).max(offset),
            )
        } else {
            let ratio = if episode_duration > 0.0 {
                (preview.sample.seconds / episode_duration).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (
                ratio * asset.duration_seconds.max(0.0),
                true,
                0.0,
                episode_duration,
            )
        };
    let url =
        crate::api::episode_video_url(&preview.episode, &asset.stream_id, asset.segment_index);
    let spans = if exact_assets.is_empty() {
        vec![TimelineVideoSpan {
            start_seconds,
            end_seconds,
            label: Some(label.clone()),
        }]
    } else {
        exact_assets
            .iter()
            .map(|asset| {
                let start = asset.timeline_offset_seconds.unwrap_or_default().max(0.0);
                TimelineVideoSpan {
                    start_seconds: start,
                    end_seconds: (start + asset.duration_seconds.max(0.0)).max(start),
                    label: Some(label.clone()),
                }
            })
            .collect()
    };
    Some(HistoricalCamera {
        label: label.clone(),
        url,
        video_seconds,
        episode_seconds: preview.sample.seconds,
        approximate,
        spans,
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
    (seconds * HISTORICAL_REPLAY_PREVIEW_HZ).floor() as u64
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
fn EmptyTimeline() -> Element {
    rsx! {
        section { class: "empty-studio-timeline",
            header { "Timeline" }
            div {
                strong { "No episode selected" }
                span { "Record in Live or select an episode in Data." }
            }
        }
    }
}

#[component]
fn AboutPopover(on_close: EventHandler<()>) -> Element {
    rsx! {
        aside { class: "about-popover", role: "dialog", "aria-label": "About rerobot studio",
            header {
                strong { "rerobot studio" }
                button { r#type: "button", onclick: move |_| on_close.call(()), "Close" }
            }
            p { "A focused workbench for operating robot arms, recording datasets, reviewing episodes, and querying their captured data." }
        }
    }
}

#[component]
fn StatusBar(mode: WorkbenchMode) -> Element {
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

fn selected_camera(assigned: &[(String, String, String)]) -> Option<(String, String, String)> {
    let studio = crate::state::studio();
    let selected_role = studio.selected.read().as_ref().and_then(|selection| {
        if let crate::state::Selection::Camera { robot, role } = selection {
            Some((robot.clone(), role.clone()))
        } else {
            None
        }
    });
    assigned
        .iter()
        .find(|(robot, role, _)| {
            selected_role
                .as_ref()
                .is_some_and(|(selected_robot, selected_role)| {
                    selected_robot == robot && selected_role == role
                })
        })
        .or_else(|| assigned.first())
        .cloned()
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

fn timeline_motion_gate(studio: Studio, source: Option<&TimelineSource>) -> TimelineMotionGate {
    if source.is_none_or(|source| source.is_recording() && !source.is_paused()) {
        return TimelineMotionGate::blocked(
            "Pause or select a completed episode before moving hardware.",
        );
    }
    if !studio.controls_available() {
        return TimelineMotionGate::blocked("Reconnect telemetry before arming timeline motion.");
    }
    if studio.mode_active() && !matches!(studio.mode(), ModeSelection::Replay { .. }) {
        return TimelineMotionGate::blocked(
            "Stop the active control mode before arming timeline motion.",
        );
    }
    if timeline_seek_context(studio, source).is_none() {
        return TimelineMotionGate::blocked("Connect and calibrate a compatible follower first.");
    }
    TimelineMotionGate::allowed()
}

fn timeline_seek_context(
    studio: Studio,
    source: Option<&TimelineSource>,
) -> Option<(String, String, Option<String>)> {
    let source = source?;
    let follower = studio.connected_robots().into_iter().find(|robot| {
        robot.calibrated
            && robot
                .robot_type
                .as_deref()
                .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
    })?;
    let (episode, arms) = match source {
        TimelineSource::Recording(recording) if recording.paused => {
            (recording.episode.clone(), recording.arms.clone())
        }
        TimelineSource::Review(episode) => (
            episode.id.clone(),
            episode.arms.iter().map(|arm| arm.id.clone()).collect(),
        ),
        TimelineSource::Recording(_) => return None,
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

fn set_timeline_recording_paused(studio: Studio, paused: bool) {
    let request = SetRecordingPausedRequest { paused };
    let future = async move {
        crate::api::send(Method::POST, "/api/episodes/record/pause", Some(&request)).await
    };
    if paused {
        studio.run_safety("Recording paused. The episode remains open.", future);
    } else {
        studio.run("Recording resumed in the same episode.", future);
    }
}

fn request_historical_preview(
    studio: Studio,
    episode: String,
    seconds: f64,
    mut state: Signal<Option<HistoricalPreview>>,
    mut revision: Signal<u64>,
) {
    let ticket = revision.peek().wrapping_add(1);
    revision.set(ticket);
    let cached = state
        .peek()
        .as_ref()
        .filter(|preview| preview.episode == episode)
        .cloned();
    let changed_episode = state.peek().is_some() && cached.is_none();
    if changed_episode {
        state.set(None);
    }
    let existing_timeline = cached.as_ref().map(|preview| preview.timeline.clone());
    let existing_signal_lanes = cached.and_then(|preview| preview.signal_lanes);
    let should_load_traces = existing_timeline.is_none() && existing_signal_lanes.is_none();
    spawn(async move {
        let timeline = match existing_timeline {
            Some(timeline) => timeline,
            None => {
                let timeline_path = crate::api::episode_timeline_url(&episode);
                match crate::api::get::<crate::api::EpisodeTimelineResponse>(&timeline_path).await {
                    Ok(timeline) => timeline,
                    Err(error) => {
                        if revision.peek().eq(&ticket) {
                            state.set(None);
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
                state.set(Some(HistoricalPreview {
                    episode: episode.clone(),
                    timeline,
                    sample,
                    signal_lanes: existing_signal_lanes.clone(),
                }));
                if should_load_traces {
                    match load_recorded_timeline_traces(&episode, 240).await {
                        Ok(traces) => {
                            if let Some(preview) = state.write().as_mut()
                                && preview.episode == episode
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
                state.set(None);
                studio.report(error);
            }
            Err(_) => {}
        }
    });
}

fn replay_target(studio: Studio) -> Option<(String, Option<String>)> {
    let episode = studio.focused_episode()?;
    let robots = studio.connected_robots();
    let follower = robots.iter().find(|robot| {
        robot.calibrated
            && robot
                .robot_type
                .as_deref()
                .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
    })?;
    let source_arm = episode
        .arms
        .iter()
        .find(|arm| arm.id == follower.id)
        .or_else(|| {
            episode
                .arms
                .iter()
                .find(|arm| arm.id.to_ascii_lowercase().contains("follower"))
        })
        .or_else(|| episode.arms.first())
        .map(|arm| arm.id.clone());
    Some((follower.id.clone(), source_arm))
}

fn start_timeline_playback(studio: Studio) {
    let Some(episode) = studio.focused_episode() else {
        return;
    };
    let Some((robot, source_arm)) = replay_target(studio) else {
        studio.report("Connect and calibrate a compatible follower before replaying.");
        return;
    };
    let request = StartReplayRequest {
        episode: episode.id,
        robot,
        source_arm,
    };
    studio.run("", async move {
        crate::api::send(Method::POST, "/api/episodes/replay", Some(&request)).await
    });
}

fn set_timeline_replay_paused(studio: Studio, paused: bool) {
    let request = SetReplayPausedRequest { paused };
    let future = async move {
        crate::api::send(Method::POST, "/api/episodes/replay/pause", Some(&request)).await
    };
    if paused {
        studio.run_safety("Replay paused at the current frame.", future);
    } else {
        studio.run("Replay resumed from the paused frame.", future);
    }
}

#[derive(serde::Serialize)]
struct TimelineSeekBody {
    robot: String,
    source_arm: Option<String>,
    seconds: f64,
}

fn request_physical_seek(
    studio: Studio,
    episode: String,
    robot: String,
    source_arm: Option<String>,
    seconds: f64,
) {
    let path = format!("/api/episodes/{episode}/seek");
    let request = TimelineSeekBody {
        robot,
        source_arm,
        seconds,
    };
    studio.run("", async move {
        crate::api::send_json::<_, serde_json::Value>(Method::POST, &path, Some(&request))
            .await
            .map(|_| ())
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
        assert_eq!(camera.spans.len(), 2);
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
    fn recorded_camera_tabs_follow_episode_streams_not_live_assignments() {
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
    fn replay_preview_slots_bound_sample_fetches_to_ten_hz() {
        assert_eq!(historical_replay_slot(f64::NAN), 0);
        assert_eq!(historical_replay_slot(0.0), 0);
        assert_eq!(historical_replay_slot(0.099), 0);
        assert_eq!(historical_replay_slot(0.100), 1);
        assert_eq!(historical_replay_slot(1.249), 12);
    }
}
