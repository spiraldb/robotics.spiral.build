//! One selected episode camera, with an episode-relative browser playback clock.
use super::DatasetClient;
use super::episode_decoder::{DecodedGroup, EncodedCamera};
use crate::format::ClockPresentation;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{EpisodeTopicRequest, EpisodeVideo};
use std::rc::Rc;
use wasm_bindgen::JsCast as _;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct EpisodeTimeRange {
    start_ns: u64,
    end_ns: u64,
}
impl EpisodeTimeRange {
    pub(super) fn new(start_ns: u64, end_ns: u64) -> Self {
        Self { start_ns, end_ns }
    }
    fn duration(self) -> f64 {
        self.end_ns.saturating_sub(self.start_ns) as f64 / 1e9
    }
}

#[component]
pub(super) fn EpisodePlayback(request: EpisodeTopicRequest, timeline: EpisodeTimeRange) -> Element {
    let mut video = use_resource(use_reactive((&request,), move |(request,)| async move {
        DatasetClient::current()
            .episode_video(request)
            .await
            .map_err(|error| error.to_string())
    }));
    let mut revision = use_signal(|| 0_u32);
    let pending = *video.state().read() == UseResourceState::Pending;
    rsx! {
        section { class: "episode-playback", "aria-label": "Episode video playback",
            div { class: "episode-playback-heading",
                span { class: "text-xs text-ink-soft break-all", "{request.topic()}" }
                button { class: "data-text-action", disabled: pending, onclick: move |_| { revision += 1; video.restart(); }, "Retry video" }
            }
            if pending { div { class: "episode-playback-placeholder", role: "status", "Loading episode video…" } }
            else { match &*video.read() {
                Some(Ok(source)) => rsx! { CameraPlayback { key: "{revision}", source: source.clone(), timeline } },
                Some(Err(error)) => rsx! { div { class: "episode-playback-placeholder text-danger", role: "alert", "{error}. Use Retry video to try again." } },
                None => rsx! {},
            } }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct PlaybackClock {
    seconds: f64,
    playing: bool,
    duration: f64,
}
impl PlaybackClock {
    fn new(duration: f64) -> Self {
        Self {
            seconds: 0.0,
            playing: false,
            duration,
        }
    }
    fn toggle(&mut self) {
        if self.seconds >= self.duration {
            self.seconds = 0.0;
        }
        self.playing = !self.playing;
    }
    fn seek(&mut self, seconds: f64) {
        if seconds.is_finite() {
            self.seconds = seconds.clamp(0.0, self.duration);
        }
    }
    fn advance(&mut self, elapsed: f64) {
        self.seek(self.seconds + elapsed.max(0.0));
        if self.seconds >= self.duration {
            self.playing = false;
        }
    }
}

#[component]
fn CameraPlayback(source: EpisodeVideo, timeline: EpisodeTimeRange) -> Element {
    let encoded = use_hook(|| Rc::new(EncodedCamera::new(&source, timeline.start_ns)));
    let mut clock = use_signal(|| {
        let mut clock = PlaybackClock::new(timeline.duration());
        if let Ok(camera) = encoded.as_ref() {
            clock.seek(camera.first_frame_seconds());
        }
        clock
    });
    let mut canvas = use_signal(|| None::<web_sys::HtmlCanvasElement>);
    let mut drawing_error = use_signal(|| None::<String>);
    let workspace = crate::shell::activity::WorkspaceActivity::current();
    let panel = dioxus_workbench::PanelContext::current();
    let active = workspace.is_active() && panel.geometry().visible();
    use_effect(use_reactive((&active,), move |(active,)| {
        if !active && clock.peek().playing {
            clock.write().playing = false;
        }
    }));
    let group_source = encoded.clone();
    let group = use_memo(move || {
        group_source
            .as_ref()
            .as_ref()
            .ok()
            .and_then(|camera| camera.group_at(clock().seconds))
    });
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
    use_effect(move || {
        let Some(canvas) = canvas() else {
            return;
        };
        let seconds = clock().seconds;
        let pending = *frames.state().read() == UseResourceState::Pending;
        let current = frames.read();
        let result = match &*current {
            Some(Ok(Some(decoded))) if !pending => decoded.draw(&canvas, seconds),
            _ => DecodedGroup::clear(&canvas),
        };
        if let Err(error) = result
            && drawing_error.peek().as_ref() != Some(&error)
        {
            drawing_error.set(Some(error));
        }
    });
    use_future(move || async move {
        let performance = web_sys::window().and_then(|window| window.performance());
        let mut previous = performance
            .as_ref()
            .map(|time| time.now())
            .unwrap_or_default();
        loop {
            gloo_timers::future::TimeoutFuture::new(16).await;
            let now = performance
                .as_ref()
                .map(|time| time.now())
                .unwrap_or(previous);
            let elapsed = (now - previous) / 1000.0;
            previous = now;
            if !clock.peek().playing {
                continue;
            }
            let hidden = web_sys::window()
                .and_then(|window| window.document())
                .is_none_or(|document| document.hidden());
            if hidden || drawing_error.peek().is_some() || matches!(&*frames.peek(), Some(Err(_))) {
                clock.write().playing = false;
                continue;
            }
            if *frames.state().peek() == UseResourceState::Ready {
                clock.write().advance(elapsed);
            }
        }
    });
    let state = clock();
    let pending = *frames.state().read() == UseResourceState::Pending;
    let error = drawing_error().or_else(|| {
        frames
            .read()
            .as_ref()
            .and_then(|result| result.as_ref().err().cloned())
    });
    let disabled = error.is_some() || state.duration <= 0.0;
    rsx! {
        canvas { class: "episode-playback-canvas", width: 640, height: 360,
            role: "img", "aria-label": "Recorded camera at {state.seconds.clock_label()}",
            onmounted: move |event| {
                let mounted = event.data();
                match mounted.downcast::<web_sys::Element>() {
                    Some(element) => canvas.set(Some(element.clone().unchecked_into())),
                    None => drawing_error.set(Some("Camera canvas is unavailable".into())),
                }
            }
        }
        div { class: "episode-playback-controls",
            button { class: "data-text-action episode-playback-toggle", disabled, onclick: move |_| clock.write().toggle(),
                if state.playing { "Pause" } else { "Play" }
            }
            input { class: "episode-playback-seek", r#type: "range", min: "0", max: "{state.duration}", step: "any", value: "{state.seconds}", disabled,
                "aria-label": "Episode time", "aria-valuetext": "{state.seconds.clock_label()} of {state.duration.clock_label()}",
                oninput: move |event| { if let Ok(seconds) = event.value().parse::<f64>() { clock.write().seek(seconds); } }
            }
            span { class: "episode-playback-time", "{state.seconds.clock_label()} / {state.duration.clock_label()}" }
        }
        if let Some(error) = error { p { class: "mt-2 text-xs text-danger", role: "alert", "{error}. Use Retry video to reload." } }
        else if pending { p { class: "mt-2 text-xs text-ink-soft", role: "status", "Decoding video…" } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn clock_seeks_clamps_and_restarts_after_the_final_frame() {
        let mut clock = PlaybackClock::new(10.0);
        clock.seek(11.0);
        assert_eq!(clock.seconds, 10.0);
        clock.toggle();
        assert_eq!(clock.seconds, 0.0);
        assert!(clock.playing);
        clock.advance(11.0);
        assert_eq!(clock.seconds, 10.0);
        assert!(!clock.playing);
        clock.seek(f64::NAN);
        assert_eq!(clock.seconds, 10.0);
        clock.seek(-2.0);
        assert_eq!(clock.seconds, 0.0);
    }
}
