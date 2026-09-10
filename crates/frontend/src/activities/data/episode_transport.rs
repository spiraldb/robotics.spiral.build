//! The selected episode owns its single clock and shared media limits.
use super::episode_decoder::MediaBudget;
use crate::format::ClockPresentation;
use dioxus::prelude::*;
use std::{collections::BTreeMap, rc::Rc};

#[derive(Clone, Copy, PartialEq)]
pub(super) struct EpisodeTimeRange {
    start_ns: u64,
    end_ns: u64,
}
impl EpisodeTimeRange {
    pub(super) fn new(start_ns: u64, end_ns: u64) -> Self {
        Self { start_ns, end_ns }
    }
    pub(super) fn bounds(self) -> (u64, u64) {
        (self.start_ns, self.end_ns.saturating_add(1))
    }
    pub(super) fn start_ns(self) -> u64 {
        self.start_ns
    }
    pub(super) fn duration(self) -> f64 {
        self.end_ns.saturating_sub(self.start_ns) as f64 / 1e9
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
    fn seek(&mut self, seconds: f64) {
        if seconds.is_finite() {
            self.seconds = seconds.clamp(0.0, self.duration);
        }
    }
    fn toggle(&mut self) {
        if self.seconds >= self.duration {
            self.seconds = 0.0;
        }
        self.playing = !self.playing;
    }
    fn advance(&mut self, elapsed: f64) {
        self.seek(
            crate::components::preview_clock::PreviewClock::new(self.duration, Some(0.0))
                .advance(self.seconds, elapsed),
        );
        if self.seconds >= self.duration {
            self.playing = false;
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
pub(super) enum FeedState {
    Loading,
    Ready,
    Unavailable,
}

pub(super) struct EpisodeMedia {
    encoded: MediaBudget,
    decoded: MediaBudget,
}
impl EpisodeMedia {
    pub(super) fn encoded(&self) -> &MediaBudget {
        &self.encoded
    }
    pub(super) fn decoded(&self) -> &MediaBudget {
        &self.decoded
    }
}
#[derive(Clone)]
pub(super) struct EpisodeTransport {
    clock: Signal<PlaybackClock>,
    feeds: Signal<BTreeMap<String, FeedState>>,
    media: Rc<EpisodeMedia>,
}
impl PartialEq for EpisodeTransport {
    fn eq(&self, other: &Self) -> bool {
        self.clock == other.clock
    }
}
impl EpisodeTransport {
    pub(super) fn use_transport(timeline: EpisodeTimeRange) -> Self {
        let clock = use_signal(|| PlaybackClock::new(timeline.duration()));
        let feeds = use_signal(BTreeMap::new);
        let media = use_hook(|| {
            Rc::new(EpisodeMedia {
                encoded: MediaBudget::new(128 * 1024 * 1024),
                decoded: MediaBudget::new(256 * 1024 * 1024),
            })
        });
        let transport = Self {
            clock,
            feeds,
            media,
        };
        let running = transport.clone();
        let activity = crate::shell::activity::WorkspaceActivity::current();
        let panel = dioxus_workbench::PanelContext::current();
        let mut visibility_clock = clock;
        use_effect(move || {
            if !activity.is_active() || !panel.geometry().visible() {
                visibility_clock.write().playing = false;
            }
        });
        use_future(move || {
            let mut running = running.clone();
            async move {
                let performance = web_sys::window().and_then(|window| window.performance());
                let mut previous = performance.as_ref().map_or(0.0, |time| time.now());
                loop {
                    gloo_timers::future::TimeoutFuture::new(16).await;
                    let now = performance.as_ref().map_or(previous, |time| time.now());
                    let elapsed = (now - previous) / 1000.0;
                    previous = now;
                    if !running.clock.peek().playing {
                        continue;
                    }
                    if web_sys::window()
                        .and_then(|window| window.document())
                        .is_none_or(|document| document.hidden())
                    {
                        running.pause();
                    } else if !running
                        .feeds
                        .peek()
                        .values()
                        .any(|state| *state == FeedState::Loading)
                    {
                        running.clock.write().advance(elapsed);
                    }
                }
            }
        });
        transport
    }
    pub(super) fn seconds(&self) -> f64 {
        self.clock.read().seconds
    }
    pub(super) fn seek(&mut self, seconds: f64) {
        self.clock.write().seek(seconds);
    }
    pub(super) fn pause(&mut self) {
        self.clock.write().playing = false;
    }
    pub(super) fn toggle(&mut self) {
        self.clock.write().toggle();
    }
    pub(super) fn media(&self) -> Rc<EpisodeMedia> {
        self.media.clone()
    }
    pub(super) fn report(&mut self, topic: &str, state: FeedState) {
        if self.feeds.peek().get(topic) != Some(&state) {
            self.feeds.write().insert(topic.to_owned(), state);
        }
    }
    pub(super) fn forget(&mut self, topic: &str) {
        self.feeds.write().remove(topic);
    }
}

#[component]
pub(super) fn EpisodeTimeline(mut transport: EpisodeTransport) -> Element {
    let state = *transport.clock.read();
    let buffering = transport
        .feeds
        .read()
        .values()
        .any(|state| *state == FeedState::Loading);
    let mut seek = transport.clone();
    rsx! {
        footer { class: "episode-shared-transport", "aria-label": "Episode timeline",
            button { class: "data-text-action", disabled: state.duration <= 0.0, onclick: move |_| transport.toggle(),
                if state.playing { "Pause" } else { "Play" }
            }
            input { r#type: "range", min: "0", max: "{state.duration}", step: "0.001", value: "{state.seconds}",
                "aria-label": "Episode time", "aria-valuetext": "{state.seconds.clock_label()} of {state.duration.clock_label()}",
                disabled: state.duration <= 0.0,
                oninput: move |event| { if let Ok(seconds) = event.value().parse() { seek.seek(seconds); } }
            }
            output { "{state.seconds.clock_label()} / {state.duration.clock_label()}" }
            span { class: "episode-transport-status", role: "status", if buffering { "Buffering…" } else { "Space to play" } }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn one_clock_clamps_seeks_and_restarts_at_the_end() {
        let mut clock = PlaybackClock::new(2.0);
        clock.seek(3.0);
        assert_eq!(clock.seconds, 2.0);
        clock.toggle();
        assert_eq!(clock.seconds, 0.0);
        clock.seek(1.9);
        clock.advance(2.5);
        assert!(!clock.playing);
        clock.seek(f64::NAN);
        assert_eq!(clock.seconds, 2.0);
        clock.seek(-1.0);
        assert_eq!(clock.seconds, 0.0);
    }
}
