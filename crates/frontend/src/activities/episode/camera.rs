//! Recorded cameras render the episode playhead; they never own a playback clock.
use super::{
    decoder::{CameraPlayback, EncodedCamera},
    messages::MessageBuffer,
    transport::{EpisodeTransport, FeedState},
};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{MessageRead, MessageWindow};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::JsCast as _;

/// One recorded camera, named on its picture the way a monitor wall names
/// its screens: its panel carries no header to name it.
#[component]
pub(in crate::activities) fn EpisodeCamera(
    request: MessageRead,
    transport: EpisodeTransport,
    title: String,
) -> Element {
    let timeline = transport.range();
    let load_request = request.clone();
    let mut video = use_resource(move || {
        let request = load_request.clone();
        async move {
            let window = MessageWindow::new(
                timeline.bounds(),
                vec![
                    vec!["value".into(), "format".into()],
                    vec!["value".into(), "data".into()],
                ],
            );
            let source = MessageBuffer::load(request.with_window(window)).await?;
            Ok::<_, String>(Rc::new(EncodedCamera::from_messages(
                source.batches(),
                timeline.start_ns(),
            )?))
        }
    });
    let mut status = transport.clone();
    let status_topic = format!(
        "{} · {}",
        request.topic().topic(),
        request.revision().variant()
    );
    let panel = dioxus_workbench::PanelContext::current();
    let activity = crate::shell::activity::WorkspaceActivity::current();
    // Only a camera on screen holds the clock: one waiting behind another tab
    // would otherwise keep playback buffering until someone looked at it.
    use_effect(move || {
        if !(panel.geometry().visible() && activity.is_active()) {
            status.forget(&status_topic);
        } else if *video.state().read() == UseResourceState::Pending {
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
                Some(Ok(camera)) => rsx! { CameraFrames { camera: camera.clone(), transport, request: request.clone() } },
                Some(Err(error)) => rsx! { div { class: "episode-camera-message", role: "alert",
                    p { "The recorded camera could not be read: {error}" }
                    button { class: "data-text-action", onclick: move |_| video.restart(), "Retry camera" }
                } },
                None => rsx! {},
            } }
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
) -> Element {
    let topic = format!(
        "{} · {}",
        request.topic().topic(),
        request.revision().variant()
    );
    let name = request.topic().topic();
    let mut canvas = use_signal(|| None::<web_sys::HtmlCanvasElement>);
    let mut failure = use_signal(|| None::<String>);
    let mut covered = use_signal(|| false);
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
            if let Some(error) = failure() { div { class: "episode-camera-message", role: "alert",
                p { "The recorded camera could not be decoded: {error}" }
                button { class: "data-text-action", onclick: move |_| { failure.set(None); covered.set(false); playback.set(retry_playback()); }, "Retry decoding" }
            } }
            else if uncovered {
                p { class: "episode-camera-message", role: "status",
                    if frame_available { "Decoding recorded camera…" } else { "No frame at this time" }
                }
            }
        }
    }
}
