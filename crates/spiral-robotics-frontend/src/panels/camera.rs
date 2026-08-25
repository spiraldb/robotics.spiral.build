//! Dockable camera panel and recorded-media mapping.

use dioxus::prelude::*;

use crate::client::historical::HistoricalPreview;
use crate::shell::activity::{LoadState, RecordedSource, TimelineSubject};
use crate::shell::frame::format_elapsed_status;

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
                let updating = source.state().is_loading();
                let warning = source.state().reason().map(str::to_owned);
                rsx! {
                    div { class: "camera-editor-meta",
                        span {
                            if recorded.approximate { "Approximate timeline sync" }
                            else if updating { "Updating from episode…" }
                            else { "Synced to episode timeline" }
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
                let (title, detail) = match &source.state() {
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
    let studio = crate::shell::state::studio();
    let status = studio
        .snapshot()
        .read()
        .cameras()
        .iter()
        .find(|camera| camera.id() == device)
        .cloned();
    // Safari can fire image lifecycle events repeatedly for an MJPEG resource.
    // Mutating component state from those events made Dioxus patch the same
    // live image element while WebKit was painting it, which presented as a flash. The
    // backend camera status is already authoritative and lets the image node
    // remain completely stable for the life of this panel.
    // A camera can fail without reporting an error string: denied OS permission
    // leaves the device listed, `error: null`, and `streaming: false` forever.
    // Reading only `error` meant nothing was drawn over the mat, so the browser
    // filled the empty <img> with its own broken-image rendering — the alt text,
    // in the default ink, at the top-left corner of a near-black panel. That is
    // exactly the permanently blank preview the design system says to replace
    // with an explicit report, and it arrived in the one place that had no say
    // over how it looked. `streaming` is the backend's own verdict, so it costs
    // no image lifecycle events to consult.
    let problem = match status {
        None => Some("This assigned camera is not currently detected.".to_owned()),
        Some(status) => status.error().map(str::to_owned).or_else(|| {
            if !status.streaming() {
                Some("This camera is assigned but its capture stream is not running.".to_owned())
            } else if status.frames() == 0 {
                // Denied camera permission on macOS looks exactly like this from
                // the backend: the capture thread opens, reports `streaming`,
                // and never delivers a frame or an error. A healthy camera
                // leaves this state within one frame period.
                Some(
                    "No frames received yet. On macOS this is usually camera permission — check System Settings › Privacy & Security › Camera."
                        .to_owned(),
                )
            } else {
                None
            }
        }),
    };

    rsx! {
        div { class: "camera-editor-frame camera-live-frame",
            img {
                class: "camera-stream-image",
                src: crate::client::http::camera_stream_url(&device),
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
    label: String,
    url: String,
    video_seconds: f64,
    episode_seconds: f64,
    approximate: bool,
}

impl HistoricalCamera {
    #[cfg(test)]
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    #[cfg(test)]
    pub(crate) const fn video_seconds(&self) -> f64 {
        self.video_seconds
    }

    #[cfg(test)]
    pub(crate) const fn episode_seconds(&self) -> f64 {
        self.episode_seconds
    }

    #[cfg(test)]
    pub(crate) const fn approximate(&self) -> bool {
        self.approximate
    }
}

#[cfg(test)]
pub(crate) fn historical_camera(
    preview: &HistoricalPreview,
    selected: Option<&(String, String, String)>,
) -> Option<HistoricalCamera> {
    historical_camera_at(preview, selected, preview.sample().seconds())
}

pub(crate) fn historical_camera_at(
    preview: &HistoricalPreview,
    selected: Option<&(String, String, String)>,
    at: f64,
) -> Option<HistoricalCamera> {
    let expected = selected.map(|(robot, role, _)| format!("{robot}.camera.{role}"));
    let stream_id = if let Some(expected) = expected.as_deref() {
        preview
            .timeline()
            .videos()
            .iter()
            .find(|asset| asset.stream_id() == expected)
            .map(|asset| asset.stream_id())?
    } else {
        preview
            .timeline()
            .videos()
            .first()
            .map(|asset| asset.stream_id())?
    };
    let label = selected
        .filter(|(robot, role, _)| stream_id == format!("{robot}.camera.{role}"))
        .map(|(_, role, _)| role.clone())
        .or_else(|| stream_id.rsplit('.').next().map(str::to_owned))
        .unwrap_or_else(|| "recorded".to_owned());
    HistoricalCameraStream::new(preview, stream_id, &label).at(at)
}

pub(crate) fn recorded_scene_source(
    subject: &TimelineSubject,
    source: Option<&RecordedSource<HistoricalPreview>>,
) -> crate::panels::scene::SceneSource {
    let Some(episode) = subject.recorded_episode() else {
        return crate::panels::scene::SceneSource::Live;
    };
    let source = source
        .filter(|source| source.episode() == episode)
        .cloned()
        .map(|source| source.map(HistoricalPreview::into_sample))
        .unwrap_or_else(|| {
            RecordedSource::new(episode, 0.0, LoadState::Loading { previous: None })
        });
    crate::panels::scene::SceneSource::Recorded(source)
}

fn map_recorded_camera(
    episode: &str,
    source: Option<&RecordedSource<HistoricalPreview>>,
    select: impl Fn(&HistoricalPreview) -> Option<HistoricalCamera>,
) -> RecordedSource<HistoricalCamera> {
    let Some(source) = source.filter(|source| source.episode() == episode) else {
        return RecordedSource::new(episode, 0.0, LoadState::Loading { previous: None });
    };
    let state = match source.state() {
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
    RecordedSource::new(source.episode(), source.seconds(), state)
}

pub(crate) struct RecordedCameraSourceRequest<'a> {
    episode: &'a str,
    source: Option<&'a RecordedSource<HistoricalPreview>>,
    at: f64,
}

impl<'a> RecordedCameraSourceRequest<'a> {
    pub(crate) const fn new(
        episode: &'a str,
        source: Option<&'a RecordedSource<HistoricalPreview>>,
        at: f64,
    ) -> Self {
        Self {
            episode,
            source,
            at,
        }
    }

    pub(crate) fn for_live_assignment(
        self,
        camera: &(String, String, String),
    ) -> CameraEditorSource {
        CameraEditorSource::Recorded(map_recorded_camera(self.episode, self.source, |preview| {
            historical_camera_at(preview, Some(camera), self.at)
        }))
    }

    pub(crate) fn for_stream(self, stream_id: &str, label: &str) -> CameraEditorSource {
        CameraEditorSource::Recorded(map_recorded_camera(self.episode, self.source, |preview| {
            HistoricalCameraStream::new(preview, stream_id, label).at(self.at)
        }))
    }
}

pub(crate) fn recorded_camera_registry_placeholder(
    episode: &str,
    source: Option<&RecordedSource<HistoricalPreview>>,
) -> CameraEditorSource {
    let (title, detail) = match source
        .filter(|source| source.episode() == episode)
        .map(RecordedSource::state)
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

impl HistoricalPreview {
    pub(crate) fn recorded_camera_streams(&self) -> Vec<(String, String)> {
        let mut streams = Vec::new();
        for asset in self.timeline().videos() {
            if streams
                .iter()
                .any(|(stream_id, _)| stream_id == asset.stream_id())
            {
                continue;
            }
            let label = asset
                .stream_id()
                .rsplit('.')
                .next()
                .filter(|label| !label.is_empty())
                .unwrap_or("camera")
                .to_owned();
            streams.push((asset.stream_id().to_owned(), label));
        }
        streams
    }
}

struct HistoricalCameraStream<'a> {
    preview: &'a HistoricalPreview,
    stream_id: &'a str,
    label: &'a str,
}

impl<'a> HistoricalCameraStream<'a> {
    const fn new(preview: &'a HistoricalPreview, stream_id: &'a str, label: &'a str) -> Self {
        Self {
            preview,
            stream_id,
            label,
        }
    }

    fn at(self, at: f64) -> Option<HistoricalCamera> {
        let assets = self
            .preview
            .timeline()
            .videos()
            .iter()
            .filter(|asset| asset.stream_id() == self.stream_id)
            .collect::<Vec<_>>();
        let mut exact_assets = assets
            .iter()
            .copied()
            .filter(|asset| asset.timeline_offset_seconds().is_some())
            .collect::<Vec<_>>();
        exact_assets.sort_by(|left, right| {
            left.timeline_offset_seconds()
                .unwrap_or_default()
                .total_cmp(&right.timeline_offset_seconds().unwrap_or_default())
        });
        let at = at.max(0.0);
        let asset = if exact_assets.is_empty() {
            assets
                .iter()
                .copied()
                .min_by_key(|asset| asset.segment_index())?
        } else {
            exact_assets
                .iter()
                .copied()
                .filter(|asset| {
                    let start = asset.timeline_offset_seconds().unwrap_or_default();
                    let end = start + asset.duration_seconds().max(0.0);
                    start <= at && at < end
                })
                .max_by(|left, right| {
                    left.timeline_offset_seconds()
                        .unwrap_or_default()
                        .total_cmp(&right.timeline_offset_seconds().unwrap_or_default())
                })
                .or_else(|| {
                    exact_assets.iter().copied().min_by(|left, right| {
                        video_segment_distance(left, at)
                            .total_cmp(&video_segment_distance(right, at))
                    })
                })?
        };
        let label = self.label.to_owned();
        let episode_duration = self
            .preview
            .timeline()
            .episode()
            .duration_seconds()
            .max(0.0);
        let (video_seconds, approximate) = if let Some(offset) = asset.timeline_offset_seconds() {
            (
                (at - offset).clamp(0.0, asset.duration_seconds().max(0.0)),
                false,
            )
        } else {
            let ratio = if episode_duration > 0.0 {
                (at / episode_duration).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (ratio * asset.duration_seconds().max(0.0), true)
        };
        let url = crate::client::http::episode_video_url(
            self.preview.episode(),
            asset.stream_id(),
            asset.segment_index(),
        );
        Some(HistoricalCamera {
            label,
            url,
            video_seconds,
            episode_seconds: at,
            approximate,
        })
    }
}

fn video_segment_distance(asset: &crate::client::http::VideoAssetMetadata, at: f64) -> f64 {
    let start = asset.timeline_offset_seconds().unwrap_or_default();
    let end = start + asset.duration_seconds().max(0.0);
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
