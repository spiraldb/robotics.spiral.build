//! Recorded cameras render the episode playhead; they never own a playback clock.
use super::{
    episode_decoder::{CameraPlayback, EncodedCamera, MediaReservation},
    episode_transport::{EpisodeTimeRange, EpisodeTransport, FeedState},
    message_buffer::MessageBuffer,
};
use crate::format::PreciseClockPresentation;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{MessageRead, MessageWindow};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::JsCast as _;

struct LoadedCamera {
    encoded: Rc<EncodedCamera>,
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
    #[props(default = true)] show_frame_timing: bool,
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
                encoded: Rc::new(encoded),
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
                Some(Ok(camera)) => rsx! { CameraFrames { camera: camera.clone(), transport, show_frame_timing, topic: format!("{} · {}", request.topic().topic(), request.revision().variant()) } },
                Some(Err(error)) => rsx! { div { class: "episode-camera-message", role: "alert",
                    p { "{error}" }
                    button { class: "data-text-action", onclick: move |_| video.restart(), "Retry camera" }
                } },
                None => rsx! {},
            } }
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
const UNCOVERED_RETRIES: u8 = 8;

#[component]
fn CameraFrames(
    camera: Rc<LoadedCamera>,
    transport: EpisodeTransport,
    topic: String,
    show_frame_timing: bool,
) -> Element {
    let mut canvas = use_signal(|| None::<web_sys::HtmlCanvasElement>);
    let mut failure = use_signal(|| None::<String>);
    let mut covered = use_signal(|| false);
    let panel = dioxus_workbench::PanelContext::current();
    let activity = crate::shell::activity::WorkspaceActivity::current();
    let media = transport.media();
    // The decode loop takes the playback out of its cell while it awaits the
    // decoder and puts it back afterwards, so no borrow spans an await.
    let fresh_playback = {
        let camera = camera.clone();
        move || {
            Rc::new(RefCell::new(Some(CameraPlayback::new(
                camera.encoded.clone(),
                media.decoded().clone(),
            ))))
        }
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
    use_effect(move || {
        let visible = panel.geometry().visible() && activity.is_active();
        let seconds = clock.seconds();
        let playback = playback();
        let Some(canvas) = canvas() else { return };
        if !visible || failure.peek().is_some() {
            return;
        }
        drive_requests.wanted.set(Some(seconds));
        if drive_requests.running.replace(true) {
            return;
        }
        let requests = drive_requests.clone();
        let mut status = status.clone();
        let topic = drive_topic.clone();
        spawn(async move {
            let mut retries = 0_u8;
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
                let prepared = decoder.prepare(target).await;
                let outcome = prepared.and_then(|prepared| {
                    decoder
                        .draw(&canvas, target)
                        .map(|drawn| (drawn, prepared.covered()))
                });
                *playback.borrow_mut() = Some(decoder);
                match outcome {
                    Ok((drawn, ready)) => {
                        covered.set(drawn);
                        status.report(&topic, FeedState::Ready);
                        // A slow decoder answers after the wait; ask again for a paused playhead.
                        if !ready && requests.wanted.get().is_none() && retries < UNCOVERED_RETRIES
                        {
                            retries += 1;
                            requests.wanted.set(Some(target));
                        } else if ready {
                            retries = 0;
                        }
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
    let sample = camera.encoded.frame_seconds(seconds);
    let uncovered = !covered();
    let retry_playback = fresh_playback.clone();
    rsx! {
        div { class: "episode-camera-image",
            canvas { class: "episode-camera-canvas", width: 640, height: 360,
                role: "img", "aria-label": "{topic} at episode time {seconds:.3} seconds",
                onmounted: move |event| {
                    if let Some(element) = event.data().downcast::<web_sys::Element>() {
                        canvas.set(Some(element.clone().unchecked_into()));
                    } else { failure.set(Some("Camera canvas is unavailable".into())); }
                }
            }
            if let Some(error) = failure() { div { class: "episode-camera-message", role: "alert",
                p { "{error}" }
                button { class: "data-text-action", onclick: move |_| { failure.set(None); covered.set(false); playback.set(retry_playback()); }, "Retry decoding" }
            } }
            else if uncovered {
                p { class: "episode-camera-message",
                    match camera.encoded.first_frame_seconds().filter(|first| seconds < *first) {
                        Some(first) => rsx! { "First frame at {first.precise_clock_label()}" },
                        None if sample.is_some() => rsx! { "Decoding recorded camera…" },
                        None => rsx! { "No frame at this time" },
                    }
                }
            }
        }
        if show_frame_timing {
        div { class: "episode-camera-time",
            if let Some(sample) = sample.filter(|_| !uncovered) {
                {format!("Frame {} · age {:.3} s", sample.precise_clock_label(), (seconds - sample).max(0.0))}
            } else { "Recorded camera" }
        }
        }
    }
}
