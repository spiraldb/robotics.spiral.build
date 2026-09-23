//! Dockable camera panel and recorded-media mapping.

use dioxus::prelude::*;

use crate::activities::live::recording_review::EpisodePreview;
use crate::client::http::CameraApiPath;
use crate::client::project::ProjectClient;
use crate::format::ClockPresentation;
use crate::panels::recorded_source::{LoadState, RecordedSource};

use wasm_bindgen::JsCast as _;

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
pub(super) fn CameraEditor(element_id: String, source: CameraEditorSource) -> Element {
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
                            if updating { "Updating from episode…" }
                            else { "Synced to episode timeline" }
                        }
                        span {
                            class: "status-dot",
                            style: if warning.is_some() { "background: var(--tone-caution)" } else { "background: var(--tone-good)" },
                        }
                    }
                    div { class: "camera-editor-frame",
                        RecordedCameraFrame { key: "{recorded.episode}:{recorded.stream}", recorded, element_id }
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
                        "This episode has no recorded video for this camera.".to_owned(),
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
    let setup = crate::robots::RobotSetupState::current();
    let workspace = crate::shell::activity::WorkspaceActivity::current();
    let panel = dioxus_workbench::PanelContext::current();
    let status = setup
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
            if workspace.is_active() && panel.geometry().visible() {
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
    episode: String,
    stream: String,
    label: String,
    episode_seconds: f64,
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

    pub(super) fn for_live_assignment(
        self,
        camera: &(String, String, String),
    ) -> CameraEditorSource {
        self.for_stream(&format!("{}.camera.{}", camera.0, camera.1), &camera.1)
    }

    fn for_stream(self, stream_id: &str, label: &str) -> CameraEditorSource {
        let at = self.at;
        CameraEditorSource::Recorded(self.map_camera(|preview| {
            Some(RecordedCamera {
                episode: preview.episode().to_owned(),
                stream: stream_id.into(),
                label: label.into(),
                episode_seconds: at.max(0.0),
            })
        }))
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
                    reason: "This episode has no recorded video for this camera.".to_owned(),
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

#[component]
fn RecordedCameraFrame(recorded: RecordedCamera, element_id: String) -> Element {
    let episode = recorded.episode.clone();
    let stream = recorded.stream.clone();
    let source = use_resource(move || {
        let episode = episode.clone();
        let stream = stream.clone();
        async move {
            crate::activities::live::RecordingClient::current()
                .camera_preview(episode, stream)
                .await
                .map(std::rc::Rc::new)
        }
    });
    match &*source.read() {
        Some(Ok(video)) => {
            rsx! { CameraFrameCanvas { source: video.clone(), seconds: recorded.episode_seconds, label: recorded.label, element_id } }
        }
        Some(Err(error)) => rsx! { div { class: "camera-live-error", role: "alert", "{error}" } },
        None => rsx! { div { class: "editor-empty", role: "status", "Loading recorded camera…" } },
    }
}

#[component]
fn CameraFrameCanvas(
    source: std::rc::Rc<spiral_robotics_common::datasets::EpisodeVideo>,
    seconds: f64,
    label: String,
    element_id: String,
) -> Element {
    use crate::activities::episode::{DecodedGroup, EncodedCamera};
    use std::rc::Rc;
    let encoded = use_hook(|| Rc::new(EncodedCamera::new(&source, 0)));
    let mut canvas = use_signal(|| None::<web_sys::HtmlCanvasElement>);
    let mut drawing_error = use_signal(|| None::<String>);
    let camera = encoded.clone();
    let group = use_memo(use_reactive((&seconds,), move |(seconds,)| {
        camera
            .as_ref()
            .as_ref()
            .ok()
            .and_then(|camera| camera.group_at(seconds))
    }));
    let frames = use_resource(move || {
        let range = group();
        let encoded = encoded.clone();
        async move {
            let camera = encoded.as_ref().as_ref().map_err(Clone::clone)?;
            match range {
                Some(range) => DecodedGroup::decode(camera, range).await.map(Some),
                None => Ok(None),
            }
        }
    });
    use_effect(use_reactive((&seconds,), move |(seconds,)| {
        let Some(canvas) = canvas() else {
            return;
        };
        let pending = *frames.state().read() == UseResourceState::Pending;
        let result = match &*frames.read() {
            Some(Ok(Some(decoded))) if !pending => decoded.draw(&canvas, seconds),
            _ => DecodedGroup::clear(&canvas),
        };
        if let Err(error) = result {
            drawing_error.set(Some(error));
        }
    }));
    let decoding = *frames.state().read() == UseResourceState::Pending;
    let no_frame = matches!(&*frames.read(), Some(Ok(None)));
    let error = drawing_error().or_else(|| {
        frames
            .read()
            .as_ref()
            .and_then(|result| result.as_ref().err().cloned())
    });
    rsx! {
        canvas { id: element_id, class: "h-full w-full object-contain", width: 640, height: 360, role: "img",
            "aria-label": "Recorded {label} camera at {seconds.clock_label()}",
            onmounted: move |event| {
                if let Some(element) = event.data().downcast::<web_sys::Element>() { canvas.set(Some(element.clone().unchecked_into())); }
            }
        }
        if let Some(error) = error {
            div { class: "camera-live-error", role: "alert", "{error}" }
        } else if decoding {
            div { class: "camera-live-error", role: "status", "Decoding recorded camera…" }
        } else if no_frame {
            div { class: "camera-live-error", role: "status", "No camera frame at this time." }
        }
    }
}
