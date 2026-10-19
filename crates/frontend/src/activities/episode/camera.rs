//! Recorded cameras render the episode playhead; they never own a playback clock.
use super::{
    boxes::{CameraBoxOverlay, CameraBoxes},
    decoder::{CameraPlayback, EncodedCamera},
    reads::{StreamRead, StreamReads},
    streams::RecordedStream,
    transport::{EpisodeTransport, FeedState},
};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::MessageRead;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::JsCast as _;

/// One recorded camera, named on its picture the way a monitor wall names
/// its screens: its panel carries no header to name it. A surface may mark
/// it with `boxes`.
#[component]
pub(super) fn EpisodeCamera(
    camera: RecordedStream,
    reads: StreamReads,
    transport: EpisodeTransport,
    title: String,
    #[props(default)] boxes: Option<CameraBoxes>,
) -> Element {
    let mut wanting = reads;
    use_effect(use_reactive(&camera, move |camera| {
        wanting.want_camera(&camera)
    }));
    let video = reads.camera(&camera);
    let request = camera.read().clone();
    let feed = format!(
        "{} · {}",
        request.topic().topic(),
        request.revision().stream()
    );
    let mut status = transport.clone();
    let status_feed = feed.clone();
    let status_camera = camera.clone();
    let panel = dioxus_workbench::PanelContext::current();
    let activity = crate::shell::activity::WorkspaceActivity::current();
    // Only a camera on screen holds the clock: one waiting behind another tab
    // would otherwise keep playback buffering until someone looked at it.
    use_effect(move || {
        let video = reads.camera(&status_camera);
        if !(panel.geometry().visible() && activity.is_active()) {
            status.forget(&status_feed);
        } else if video.as_ref().is_none_or(StreamRead::reading) {
            status.report(&status_feed, FeedState::Loading);
        } else if video.as_ref().and_then(StreamRead::failure).is_some() {
            status.report(&status_feed, FeedState::Unavailable);
        }
    });
    let mut cleanup = transport.clone();
    use_drop(move || cleanup.forget(&feed));
    let mut retrying = reads;
    rsx! {
        div { class: "episode-camera-panel",
            match video {
                Some(StreamRead::Ready(encoded)) => rsx! { CameraFrames { camera: encoded, transport, request, boxes } },
                Some(StreamRead::Failed(error)) => rsx! { div { class: "episode-camera-message", role: "alert",
                    p { "The recorded camera could not be read: {error}" }
                    button { class: "data-text-action", onclick: move |_| retrying.read_camera(&camera), "Retry" }
                } },
                _ => rsx! { p { class: "episode-camera-message", role: "status", "Loading…" } },
            }
            span { class: "camera-caption", "{title}" }
        }
    }
}

/// Frames the operator is asking for, and whether a decode loop is serving them.
struct FrameRequests {
    wanted: Cell<Option<f64>>,
    running: Cell<bool>,
}

/// Seeking this deep into a keyframe interval is reported as buffering so the
/// shared clock waits instead of racing ahead of the picture.
const QUIET_RESTART_PACKETS: usize = 15;

#[component]
fn CameraFrames(
    camera: Rc<EncodedCamera>,
    transport: EpisodeTransport,
    request: MessageRead,
    boxes: Option<CameraBoxes>,
) -> Element {
    let topic = format!(
        "{} · {}",
        request.topic().topic(),
        request.revision().stream()
    );
    let name = request.topic().topic();
    let mut canvas = use_signal(|| None::<web_sys::HtmlCanvasElement>);
    let mut failure = use_signal(|| None::<String>);
    let mut covered = use_signal(|| false);
    // The painted picture's own size, which boxes over it are drawn in.
    let mut picture = use_signal(|| None::<(u32, u32)>);
    let panel = dioxus_workbench::PanelContext::current();
    let activity = crate::shell::activity::WorkspaceActivity::current();
    // The decode loop takes the playback out of its cell while it awaits the
    // decoder and puts it back afterwards, so no borrow spans an await.
    let fresh_playback = {
        let camera = camera.clone();
        move || Rc::new(RefCell::new(Some(CameraPlayback::new(camera.clone()))))
    };
    let mut playback = use_signal(fresh_playback.clone());
    let requests = use_hook(|| {
        Rc::new(FrameRequests {
            wanted: Cell::new(None),
            running: Cell::new(false),
        })
    });
    let status = transport.clone();
    let clock = transport.clone();
    let drive_requests = requests.clone();
    let drive_topic = topic.clone();
    let drive_camera = camera.clone();
    use_effect(move || {
        let visible = panel.geometry().visible() && activity.is_active();
        let seconds = clock.seconds();
        let playback = playback();
        let Some(canvas) = canvas() else { return };
        if !visible {
            status.clone().forget(&drive_topic);
            return;
        }
        if failure.peek().is_some() {
            return;
        }
        // A playhead before the first recorded frame holds that frame; an
        // opened episode shows a picture, never an empty mat.
        let target = drive_camera.held_frame_seconds(seconds);
        drive_requests.wanted.set(Some(target));
        if drive_requests.running.replace(true) {
            return;
        }
        let requests = drive_requests.clone();
        let mut status = status.clone();
        let topic = drive_topic.clone();
        spawn(async move {
            while let Some(target) = requests.wanted.take() {
                let Some(mut decoder) = playback.borrow_mut().take() else {
                    break;
                };
                let deep_restart = decoder
                    .restart_cost(target)
                    .is_some_and(|packets| packets > QUIET_RESTART_PACKETS);
                if deep_restart {
                    status.report(&topic, FeedState::Loading);
                }
                let outcome = decoder
                    .prepare(target)
                    .await
                    .and_then(|()| decoder.draw(&canvas, target));
                *playback.borrow_mut() = Some(decoder);
                match outcome {
                    Ok(drawn) => {
                        covered.set(drawn);
                        let size = drawn.then(|| (canvas.width(), canvas.height()));
                        if *picture.peek() != size {
                            picture.set(size);
                        }
                        status.report(&topic, FeedState::Ready);
                    }
                    Err(error) => {
                        failure.set(Some(error));
                        status.report(&topic, FeedState::Unavailable);
                        break;
                    }
                }
            }
            requests.running.set(false);
        });
    });
    let seconds = transport.seconds();
    let frame_available = camera
        .first_frame_after(seconds)
        .or_else(|| camera.frame_seconds(seconds))
        .is_some();
    let uncovered = !covered();
    let retry_playback = fresh_playback.clone();
    rsx! {
        div { class: "episode-camera-image",
            canvas { class: "episode-camera-canvas", width: 640, height: 360,
                role: "img", "aria-label": "Recorded camera {name}",
                onmounted: move |event| {
                    if let Some(element) = event.data().downcast::<web_sys::Element>() {
                        canvas.set(Some(element.clone().unchecked_into()));
                    } else { failure.set(Some("Camera canvas is unavailable".into())); }
                }
            }
            if let (Some(boxes), Some(size)) = (boxes, picture()) {
                CameraBoxOverlay { boxes, seconds, size }
            }
            if let Some(error) = failure() { div { class: "episode-camera-message", role: "alert",
                p { "The recorded camera could not be decoded: {error}" }
                button { class: "data-text-action", onclick: move |_| { failure.set(None); covered.set(false); playback.set(retry_playback()); }, "Retry" }
            } }
            else if uncovered {
                p { class: "episode-camera-message", role: "status",
                    if frame_available { "Decoding…" } else { "No frame at this time" }
                }
            }
        }
    }
}
