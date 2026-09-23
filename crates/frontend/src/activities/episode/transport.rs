//! An open episode owns one clock and one view window.
//!
//! Every panel showing the episode — cameras, plots, the timeline, messages —
//! reads the same playhead and the same visible span. Nothing here fetches
//! data; the transport only says *when* the operator is looking.
use super::identity::Episode;
use crate::components::timeline::{
    PlaybackClock, TimelineAction, TimelineEvent, TimelineInteraction, TimelineState, TransportKey,
    ViewWindow,
};
use dioxus::prelude::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(in crate::activities) struct EpisodeTimeRange {
    start_ns: u64,
    end_ns: u64,
}
impl EpisodeTimeRange {
    /// No recorded span: what the clock reads before an episode opens.
    const EMPTY: Self = Self {
        start_ns: 0,
        end_ns: 0,
    };
    pub(super) fn new(start_ns: u64, end_ns: u64) -> Self {
        Self {
            start_ns,
            end_ns: end_ns.max(start_ns),
        }
    }
    /// Half-open bounds that include the final message.
    pub(in crate::activities) fn bounds(self) -> (u64, u64) {
        (self.start_ns, self.end_ns.saturating_add(1))
    }
    fn duration(self) -> f64 {
        self.end_ns.saturating_sub(self.start_ns) as f64 / 1e9
    }
    pub(in crate::activities) fn start_ns(self) -> u64 {
        self.start_ns
    }
    pub(in crate::activities) fn seconds_of(self, absolute_ns: u64) -> f64 {
        (i128::from(absolute_ns) - i128::from(self.start_ns)) as f64 / 1e9
    }
    pub(in crate::activities) fn ns_at(self, seconds: f64) -> u64 {
        let offset = (seconds.max(0.0) * 1e9).round();
        self.start_ns.saturating_add(offset as u64)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum FeedState {
    Loading,
    Ready,
    Unavailable,
}

#[derive(Clone)]
pub(in crate::activities) struct EpisodeTransport {
    clock: Signal<PlaybackClock>,
    view: Signal<ViewWindow>,
    range: Signal<EpisodeTimeRange>,
    feeds: Signal<BTreeMap<String, FeedState>>,
    generation: Signal<u64>,
}
impl PartialEq for EpisodeTransport {
    fn eq(&self, other: &Self) -> bool {
        self.clock == other.clock
    }
}
impl EpisodeTransport {
    /// The clock of a composition that shows one episode for its whole life.
    pub(super) fn use_episode(episode: &Episode) -> Self {
        Self::use_range(episode.range().unwrap_or(EpisodeTimeRange::EMPTY))
    }
    /// One transport per Data workspace; `open` points it at an episode.
    pub(in crate::activities) fn use_transport() -> Self {
        Self::use_range(EpisodeTimeRange::EMPTY)
    }
    /// Point the clock at another episode, from its beginning.
    pub(in crate::activities) fn open(&mut self, episode: &Episode) {
        self.load(episode.range().unwrap_or(EpisodeTimeRange::EMPTY));
    }
    /// Show no episode at all.
    pub(in crate::activities) fn close(&mut self) {
        self.load(EpisodeTimeRange::EMPTY);
    }
    pub(super) fn play_interval(&mut self, start: f64, end: f64) {
        self.clock.write().play_interval(start, end);
    }
    fn use_range(initial: EpisodeTimeRange) -> Self {
        let clock = use_signal(|| PlaybackClock::new(initial.duration()));
        let view = use_signal(|| ViewWindow::full(initial.duration()));
        let range = use_signal(|| initial);
        let feeds = use_signal(BTreeMap::new);
        let transport = Self {
            clock,
            view,
            range,
            feeds,
            generation: use_signal(|| 0),
        };
        let running = transport.clone();
        let activity = crate::shell::activity::WorkspaceActivity::current();
        let mut visibility_clock = clock;
        use_effect(move || {
            if !activity.is_active() {
                visibility_clock.write().set_playing(false);
            }
        });
        let active = use_memo(move || activity.is_active());
        PlaybackClock::use_ticker(
            active.into(),
            use_callback(move |elapsed| {
                let mut running = running.clone();
                if !running.clock.peek().playing() {
                    return;
                }
                if web_sys::window()
                    .and_then(|window| window.document())
                    .is_none_or(|document| document.hidden())
                {
                    running.pause();
                } else if !running.buffering() {
                    running.clock.write().advance(elapsed);
                }
            }),
        );
        transport
    }
    fn load(&mut self, range: EpisodeTimeRange) {
        let duration = range.duration();
        self.clock.write().reset(duration);
        let next = self.generation.peek().wrapping_add(1);
        self.generation.set(next);
        self.view.set(ViewWindow::full(duration));
        self.range.set(range);
        self.feeds.write().clear();
    }
    pub(in crate::activities) fn range(&self) -> EpisodeTimeRange {
        *self.range.read()
    }
    pub(in crate::activities) fn seconds(&self) -> f64 {
        self.clock.read().seconds()
    }
    pub(in crate::activities) fn seek(&mut self, seconds: f64) {
        self.clock.write().seek(seconds);
    }
    fn pause(&mut self) {
        self.clock.write().set_playing(false);
    }
    fn set_speed(&mut self, speed: f64) {
        self.clock.write().set_speed(speed);
    }
    pub(in crate::activities) fn view(&self) -> ViewWindow {
        *self.view.read()
    }
    pub(super) fn report(&mut self, topic: &str, state: FeedState) {
        if self.feeds.peek().get(topic) != Some(&state) {
            self.feeds.write().insert(topic.to_owned(), state);
        }
    }
    pub(super) fn forget(&mut self, topic: &str) {
        self.feeds.write().remove(topic);
    }
    /// Shared keyboard transport: Space, arrows, Home/End, zoom keys, from a
    /// workspace event or from a listener on the page itself.
    /// Returns whether the key was consumed.
    pub(in crate::activities) fn handle_key(&mut self, key: impl Into<TransportKey>) -> bool {
        if let Some(action) = self.timeline_state().key_action(key.into()) {
            self.apply(action);
            true
        } else {
            false
        }
    }
    pub(in crate::activities) fn timeline_state(&self) -> TimelineState {
        TimelineState::new(self.generation.read().to_string(), *self.clock.read())
            .with_window(self.view())
            .with_buffering(self.buffering())
    }
    pub(in crate::activities) fn interaction(&self) -> TimelineInteraction {
        let mut transport = self.clone();
        TimelineInteraction::new(
            self.timeline_state(),
            EventHandler::new(move |event: TimelineEvent| {
                if event.target() == transport.generation.peek().to_string() {
                    transport.apply(event.action());
                }
            }),
        )
    }
    fn apply(&mut self, action: TimelineAction) {
        match action {
            TimelineAction::Seek { seconds, .. } => self.seek(seconds),
            TimelineAction::SetPlaying(playing) => self.clock.write().set_playing(playing),
            TimelineAction::SetSpeed(speed) => self.set_speed(speed),
            TimelineAction::SetLooping(looping) => self.clock.write().set_looping(looping),
            TimelineAction::SetWindow(window) => self.view.set(window),
            TimelineAction::ActivateInterval(_) => {}
        }
    }
    fn buffering(&self) -> bool {
        self.feeds
            .read()
            .values()
            .any(|state| *state == FeedState::Loading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        components::timeline::SeekPhase,
        shell::activity::{Activity, WorkspaceActivity},
    };
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct TransportHarness(Rc<RefCell<Option<EpisodeTransport>>>);
    impl TransportHarness {
        fn render(self) -> Element {
            let active = use_signal(|| true);
            WorkspaceActivity::provide(Activity::Data, active.into());
            *self.0.borrow_mut() = Some(EpisodeTransport::use_range(EpisodeTimeRange::new(
                0,
                10_000_000_000,
            )));
            rsx! {}
        }
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn timeline_events_cannot_seek_a_replacement_episode_even_with_identical_bounds() {
        let harness = TransportHarness::default();
        let mut dom = VirtualDom::new_with_props(TransportHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.runtime().in_scope(ScopeId::APP, || {
            let mut transport = harness.0.borrow().as_ref().unwrap().clone();
            let stale = transport.interaction();
            transport.load(EpisodeTimeRange::new(0, 10_000_000_000));
            stale.emit(TimelineAction::Seek {
                seconds: 5.0,
                phase: SeekPhase::Commit,
            });
            assert_eq!(transport.seconds(), 0.0);
            transport.interaction().emit(TimelineAction::Seek {
                seconds: 12.0,
                phase: SeekPhase::Commit,
            });
            assert_eq!(transport.seconds(), 10.0);
        });
    }
}
