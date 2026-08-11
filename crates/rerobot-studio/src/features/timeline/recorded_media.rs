//! Camera panel rendering and recorded-media mapping.

use dioxus::prelude::*;

use crate::app::activity::{LoadState, RecordedSource, TimelineSubject};
use crate::app::shell::format_elapsed_status;
use crate::features::timeline::historical_source::HistoricalPreview;

#[cfg(feature = "web")]
use wasm_bindgen::{JsCast as _, JsValue};

/// Camera panels render exactly one declared source. A recorded camera never
/// falls back to the live device when its clip is missing or still loading.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CameraEditorSource {
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
pub(crate) fn CameraEditor(
    element_id: String,
    source: CameraEditorSource,
    playing: bool,
) -> Element {
    // DOM media synchronization belongs to the media element that owns the
    // node. A prop change now seeks/plays only this camera instead of making
    // the application root scan every recorded element after each frame.
    use_effect(use_reactive(
        (&element_id, &source, &playing),
        move |(element_id, source, playing)| {
            if let CameraEditorSource::Recorded(source) = source
                && let Some(recorded) = source.renderable()
            {
                sync_recorded_camera(&element_id, recorded.video_seconds, playing);
            }
        },
    ));
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
    let studio = crate::app::state::studio();
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
pub(crate) struct HistoricalCamera {
    pub label: String,
    pub url: String,
    pub video_seconds: f64,
    pub episode_seconds: f64,
    pub approximate: bool,
}

#[cfg(test)]
pub(crate) fn historical_camera(
    preview: &HistoricalPreview,
    selected: Option<&(String, String, String)>,
) -> Option<HistoricalCamera> {
    historical_camera_at(preview, selected, preview.sample.seconds)
}

pub(crate) fn historical_camera_at(
    preview: &HistoricalPreview,
    selected: Option<&(String, String, String)>,
    at: f64,
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
    historical_camera_for_stream_at(preview, stream_id, &label, at)
}

pub(crate) fn recorded_scene_source(
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

pub(crate) fn recorded_camera_source_for_live_assignment(
    episode: &str,
    source: Option<&RecordedSource<HistoricalPreview>>,
    camera: &(String, String, String),
    at: f64,
) -> CameraEditorSource {
    CameraEditorSource::Recorded(map_recorded_camera(episode, source, |preview| {
        historical_camera_at(preview, Some(camera), at)
    }))
}

pub(crate) fn recorded_camera_source_for_stream(
    episode: &str,
    source: Option<&RecordedSource<HistoricalPreview>>,
    stream_id: &str,
    label: &str,
    at: f64,
) -> CameraEditorSource {
    CameraEditorSource::Recorded(map_recorded_camera(episode, source, |preview| {
        historical_camera_for_stream_at(preview, stream_id, label, at)
    }))
}

pub(crate) fn recorded_camera_registry_placeholder(
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

pub(crate) fn recorded_camera_streams(preview: &HistoricalPreview) -> Vec<(String, String)> {
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

pub(crate) fn historical_camera_for_stream_at(
    preview: &HistoricalPreview,
    stream_id: &str,
    label: &str,
    at: f64,
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
    let at = at.max(0.0);
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
            (at - offset).clamp(0.0, asset.duration_seconds.max(0.0)),
            false,
        )
    } else {
        let ratio = if episode_duration > 0.0 {
            (at / episode_duration).clamp(0.0, 1.0)
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
        episode_seconds: at,
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
