//! Recorded cameras render the episode playhead; they never own a playback clock.
use super::{
    episode_decoder::{DecodedGroup, EncodedCamera, MediaReservation},
    episode_transport::{EpisodeTimeRange, EpisodeTransport, FeedState},
    message_buffer::MessageBuffer,
};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{MessageRead, MessageWindow};
use std::rc::Rc;
use wasm_bindgen::JsCast as _;

struct LoadedCamera {
    encoded: EncodedCamera,
    _memory: MediaReservation,
}
impl PartialEq for LoadedCamera {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

#[component]
pub(super) fn EpisodeCamera(
    request: MessageRead,
    timeline: EpisodeTimeRange,
    transport: EpisodeTransport,
) -> Element {
    let media = transport.media();
    let load_request = request.clone();
    let mut video = use_resource(move || {
        let media = media.clone();
        let request = load_request.clone();
        async move {
            let window = MessageWindow::new(
                timeline.bounds(),
                vec![
                    vec!["value".into(), "format".into()],
                    vec!["value".into(), "data".into()],
                ],
            );
            let source = MessageBuffer::load(request.with_window(window), true).await?;
            let encoded = EncodedCamera::from_messages(source.batches(), timeline.start_ns())?;
            let memory = media.encoded().reserve(encoded.memory_bytes())?;
            Ok::<_, String>(Rc::new(LoadedCamera {
                encoded,
                _memory: memory,
            }))
        }
    });
    let mut status = transport.clone();
    let status_topic = format!(
        "{} · {}",
        request.topic().topic(),
        request.revision().variant()
    );
    use_effect(move || {
        if *video.state().read() == UseResourceState::Pending {
            status.report(&status_topic, FeedState::Loading);
        } else if matches!(&*video.read(), Some(Err(_))) {
            status.report(&status_topic, FeedState::Unavailable);
        }
    });
    let mut cleanup = transport.clone();
    let cleanup_topic = format!(
        "{} · {}",
        request.topic().topic(),
        request.revision().variant()
    );
    use_drop(move || cleanup.forget(&cleanup_topic));
    let pending = *video.state().read() == UseResourceState::Pending;
    rsx! {
        div { class: "episode-camera-panel",
            if pending { p { class: "episode-camera-message", role: "status", "Loading recorded camera…" } }
            else { match &*video.read() {
                Some(Ok(camera)) => rsx! { CameraFrames { camera: camera.clone(), transport, topic: format!("{} · {}", request.topic().topic(), request.revision().variant()) } },
                Some(Err(error)) => rsx! { div { class: "episode-camera-message", role: "alert",
                    p { "{error}" }
                    button { class: "data-text-action", onclick: move |_| video.restart(), "Retry camera" }
                } },
                None => rsx! {},
            } }
        }
    }
}

#[component]
fn CameraFrames(camera: Rc<LoadedCamera>, transport: EpisodeTransport, topic: String) -> Element {
    let mut canvas = use_signal(|| None::<web_sys::HtmlCanvasElement>);
    let mut drawing_error = use_signal(|| None::<String>);
    let mut revision = use_signal(|| 0u64);
    let panel = dioxus_workbench::PanelContext::current();
    let activity = crate::shell::activity::WorkspaceActivity::current();
    let group_camera = camera.clone();
    let group_transport = transport.clone();
    let group = use_memo(move || {
        if !panel.geometry().visible() || !activity.is_active() {
            return None;
        }
        let seconds = group_transport.seconds();
        if group_camera
            .encoded
            .last_frame_seconds()
            .is_none_or(|end| seconds > end)
        {
            return None;
        }
        group_camera.encoded.group_at(seconds)
    });
    let decode_camera = camera.clone();
    let media = transport.media();
    let frames = use_resource(move || {
        let range = group();
        let _ = revision();
        let camera = decode_camera.clone();
        let budget = media.decoded().clone();
        async move {
            match range {
                Some(range) => {
                    DecodedGroup::decode_with_budget(&camera.encoded, range, Some(budget))
                        .await
                        .map(Some)
                }
                None => Ok(None),
            }
        }
    });
    let render_transport = transport.clone();
    use_effect(move || {
        let seconds = render_transport.seconds();
        let Some(canvas) = canvas() else {
            return;
        };
        let pending = *frames.state().read() == UseResourceState::Pending;
        let current = frames.read();
        let result = match &*current {
            Some(Ok(Some(decoded))) if !pending && group().is_some() => {
                decoded.draw(&canvas, seconds)
            }
            _ => DecodedGroup::clear(&canvas),
        };
        if let Err(error) = result {
            drawing_error.set(Some(error));
        }
    });
    let mut status = transport.clone();
    let report_topic = topic.clone();
    use_effect(move || {
        let visible = panel.geometry().visible() && activity.is_active();
        let failed = drawing_error().is_some() || matches!(&*frames.read(), Some(Err(_)));
        let state = if !visible || failed {
            FeedState::Unavailable
        } else if *frames.state().read() == UseResourceState::Pending {
            FeedState::Loading
        } else {
            FeedState::Ready
        };
        status.report(&report_topic, state);
    });
    let seconds = transport.seconds();
    let sample = camera.encoded.frame_seconds(seconds);
    let error = drawing_error().or_else(|| {
        frames
            .read()
            .as_ref()
            .and_then(|result| result.as_ref().err().cloned())
    });
    let pending = *frames.state().read() == UseResourceState::Pending;
    let uncovered = group().is_none();
    rsx! {
        div { class: "episode-camera-image",
            canvas { class: "episode-camera-canvas", width: 640, height: 360,
                role: "img", "aria-label": "{topic} at episode time {seconds:.3} seconds",
                onmounted: move |event| {
                    if let Some(element) = event.data().downcast::<web_sys::Element>() {
                        canvas.set(Some(element.clone().unchecked_into()));
                    } else { drawing_error.set(Some("Camera canvas is unavailable".into())); }
                }
            }
            if let Some(error) = error { div { class: "episode-camera-message", role: "alert",
                p { "{error}" }
                button { class: "data-text-action", onclick: move |_| { drawing_error.set(None); revision += 1; }, "Retry decoding" }
            } }
            else if pending { p { class: "episode-camera-message", role: "status", "Decoding recorded camera…" } }
            else if uncovered { p { class: "episode-camera-message", "No frame at this time" } }
        }
        div { class: "episode-camera-time",
            if let Some(sample) = sample.filter(|_| !uncovered && !pending) {
                {format!("Frame {sample:.3} s · age {:.3} s", (seconds - sample).max(0.0))}
            } else { "Recorded camera" }
        }
    }
}
