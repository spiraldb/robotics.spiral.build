//! Dockable camera panel and recorded-media mapping.

use dioxus::prelude::*;

use crate::activities::data::episode_cache::EpisodePreview;
use crate::client::http::{CameraApiPath, EpisodeApiPath};
use crate::client::project::ProjectClient;
use crate::format::ClockPresentation;
use crate::panels::recorded_source::{LoadState, RecordedSource};

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
    Recorded(RecordedSource<RecordedCamera>),
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
    let workspace = crate::shell::activity::WorkspaceActivity::current();
    let playing = playing && workspace.is_active();
    // DOM media synchronization belongs to the media element that owns the
    // node. A prop change now seeks/plays only this camera instead of making
    // the application root scan every recorded element after each frame.
    use_effect(use_reactive(
        (&element_id, &source, &playing),
        move |(element_id, source, playing)| {
            if let CameraEditorSource::Recorded(source) = source
                && let Some(recorded) = source.renderable()
            {
                recorded.sync_element(&element_id, playing);
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
                let sync_camera = recorded.clone();
                let scoped_url = ProjectClient::current().scoped_url(&recorded.url);
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
                            src: scoped_url,
                            preload: "auto",
                            autoplay: playing,
                            muted: true,
                            playsinline: true,
                            onloadedmetadata: move |_| sync_camera.sync_element(&element_id, playing),
                            "aria-label": "Recorded {recorded.label} camera at {recorded.episode_seconds.clock_label()}",
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
    let studio = crate::shell::state::StudioProjectState::current();
    let workspace = crate::shell::activity::WorkspaceActivity::current();
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
            if workspace.is_active() {
            img {
                class: "camera-stream-image",
                src: ProjectClient::current().scoped_url(&device.camera_stream_url()),
                alt: "Live view from the {role} camera on {robot}",
                loading: "eager",
                decoding: "sync",
                draggable: "false",
            }
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
pub(crate) struct RecordedCamera {
    label: String,
    url: String,
    video_seconds: f64,
    episode_seconds: f64,
    approximate: bool,
}

impl RecordedCamera {
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

    fn sync_element(&self, element_id: &str, should_play: bool) {
        let Some(element) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(element_id))
        else {
            return;
        };
        let target = self.video_seconds.max(0.0);
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
}

pub(crate) struct RecordedCameraSourceRequest<'a> {
    episode: &'a str,
    source: Option<&'a RecordedSource<EpisodePreview>>,
    at: f64,
}

impl<'a> RecordedCameraSourceRequest<'a> {
    pub(crate) const fn new(
        episode: &'a str,
        source: Option<&'a RecordedSource<EpisodePreview>>,
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
        self.for_stream(&format!("{}.camera.{}", camera.0, camera.1), &camera.1)
    }

    pub(crate) fn for_stream(self, stream_id: &str, label: &str) -> CameraEditorSource {
        let at = self.at;
        CameraEditorSource::Recorded(
            self.map_camera(|preview| RecordedCameraStream::new(preview, stream_id, label).at(at)),
        )
    }

    fn map_camera(
        self,
        select: impl Fn(&EpisodePreview) -> Option<RecordedCamera>,
    ) -> RecordedSource<RecordedCamera> {
        let Some(source) = self
            .source
            .filter(|source| source.episode() == self.episode)
        else {
            return RecordedSource::new(self.episode, 0.0, LoadState::Loading { previous: None });
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
}

impl EpisodePreview {
    #[cfg(test)]
    pub(crate) fn recorded_camera(
        &self,
        selected: Option<&(String, String, String)>,
    ) -> Option<RecordedCamera> {
        self.recorded_camera_at(selected, self.sample().seconds())
    }

    #[cfg(test)]
    pub(crate) fn recorded_camera_at(
        &self,
        selected: Option<&(String, String, String)>,
        at: f64,
    ) -> Option<RecordedCamera> {
        let expected = selected.map(|(robot, role, _)| format!("{robot}.camera.{role}"));
        let stream_id = if let Some(expected) = expected.as_deref() {
            self.timeline()
                .videos()
                .iter()
                .find(|asset| asset.stream_id() == expected)
                .map(|asset| asset.stream_id())?
        } else {
            self.timeline()
                .videos()
                .first()
                .map(|asset| asset.stream_id())?
        };
        let label = selected
            .filter(|(robot, role, _)| stream_id == format!("{robot}.camera.{role}"))
            .map(|(_, role, _)| role.clone())
            .or_else(|| stream_id.rsplit('.').next().map(str::to_owned))
            .unwrap_or_else(|| "recorded".to_owned());
        RecordedCameraStream::new(self, stream_id, &label).at(at)
    }
}

struct RecordedCameraStream<'a> {
    preview: &'a EpisodePreview,
    stream_id: &'a str,
    label: &'a str,
}

impl<'a> RecordedCameraStream<'a> {
    const fn new(preview: &'a EpisodePreview, stream_id: &'a str, label: &'a str) -> Self {
        Self {
            preview,
            stream_id,
            label,
        }
    }

    fn at(self, at: f64) -> Option<RecordedCamera> {
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
                        left.distance_from(at).total_cmp(&right.distance_from(at))
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
        let url = self
            .preview
            .episode()
            .video_url(asset.stream_id(), asset.segment_index());
        Some(RecordedCamera {
            label,
            url,
            video_seconds,
            episode_seconds: at,
            approximate,
        })
    }
}

trait VideoSegmentTimeline {
    fn distance_from(&self, at: f64) -> f64;
}

impl VideoSegmentTimeline for crate::activities::data::episode_cache::VideoAssetMetadata {
    fn distance_from(&self, at: f64) -> f64 {
        let start = self.timeline_offset_seconds().unwrap_or_default();
        let end = start + self.duration_seconds().max(0.0);
        if at < start {
            start - at
        } else if at > end {
            at - end
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::activities::data::episode_cache::{
        EpisodeTimeline, TimelineSample, VideoAssetMetadata,
    };
    use crate::panels::timeline::panel::RecordedTimelineTraces;
    use spiral_robotics_common::recording::{
        EpisodeInfo, EpisodeVideoFormat, EpisodeVideoIdentity, EpisodeVideoTimeline,
    };
    use std::sync::Arc;
    fn episode_summary() -> EpisodeInfo {
        EpisodeInfo::new("take-1", "default")
            .with_source("Teleoperation")
            .with_timeline(600, 10.0)
            .with_sample_hz(60.0)
    }
    fn preview(at: f64, videos: Vec<VideoAssetMetadata>) -> EpisodePreview {
        EpisodePreview::fixture(
            "take-1".to_owned(),
            Arc::new(EpisodeTimeline::new(episode_summary(), videos)),
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
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn recorded_camera_selects_the_exact_pause_segment() {
        let preview = preview(
            4.25,
            vec![video(0, Some(0.0), 2.0), video(1, Some(4.0), 3.0)],
        );

        let camera = preview.recorded_camera(None).expect("recorded camera");

        assert!(!camera.approximate());
        assert_eq!(camera.video_seconds(), 0.25);
        assert_eq!(
            camera.url(),
            "/api/episodes/take-1/videos/follower.camera.wrist?segment=1"
        );
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn recorded_camera_keeps_legacy_sync_explicitly_approximate() {
        let preview = preview(5.0, vec![video(0, None, 8.0)]);

        let camera = preview.recorded_camera(None).expect("recorded camera");

        assert!(camera.approximate());
        assert_eq!(camera.video_seconds(), 4.0);
        assert_eq!(
            camera.url(),
            "/api/episodes/take-1/videos/follower.camera.wrist?segment=0"
        );
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn recorded_video_sync_uses_the_controller_clock_not_a_late_sample_response() {
        let preview = preview(1.0, vec![video(0, Some(0.0), 8.0)]);

        let camera = preview
            .recorded_camera_at(None, 5.0)
            .expect("recorded camera");

        assert_eq!(camera.video_seconds(), 5.0);
        assert_eq!(camera.episode_seconds(), 5.0);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
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
