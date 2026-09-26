//! An open episode owns one clock and one view window.
//!
//! Every panel showing the episode — cameras, plots, the timeline, messages —
//! reads the same playhead and the same visible span. Nothing here fetches
//! data; the transport only says *when* the operator is looking.
use super::identity::Episode;
use crate::components::page_keys::PageKeys;
use crate::components::timeline::{
    KeyboardFocus, PlaybackClock, TimelineAction, TimelineEvent, TimelineInteraction,
    TimelineState, TransportKey, ViewWindow,
};
use dioxus::prelude::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(in crate::activities) struct EpisodeTimeRange {
    start_ns: u64,
    end_ns: u64,
}
impl EpisodeTimeRange {
    /// No recorded span: an episode whose metadata does not say when it ran.
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
    pub(super) fn bounds(self) -> (u64, u64) {
        (self.start_ns, self.end_ns.saturating_add(1))
    }
    fn duration(self) -> f64 {
        self.end_ns.saturating_sub(self.start_ns) as f64 / 1e9
    }
    pub(super) fn start_ns(self) -> u64 {
        self.start_ns
    }
    pub(super) fn ns_at(self, seconds: f64) -> u64 {
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
pub(super) struct EpisodeTransport {
    clock: Signal<PlaybackClock>,
    view: Signal<ViewWindow>,
    range: Signal<EpisodeTimeRange>,
    feeds: Signal<BTreeMap<String, FeedState>>,
}
impl PartialEq for EpisodeTransport {
    fn eq(&self, other: &Self) -> bool {
        self.clock == other.clock
    }
}
impl EpisodeTransport {
    /// Each inspection owns its transport for one episode's whole life, so
    /// every event its timeline raises is meant for it.
    const TIMELINE_TARGET: &str = "episode";
    /// The clock of an inspection, which shows one episode for its whole life.
    pub(super) fn use_episode(episode: &Episode) -> Self {
        Self::use_range(episode.range().unwrap_or(EpisodeTimeRange::EMPTY))
    }
    /// Play from `start`, exactly there — the playhead lands inside the
    /// highlight that marks the same interval — and on past its end, so the
    /// operator sees what came next rather than a clip cut off.
    pub(super) fn play_from(&mut self, start: f64) {
        self.clock.write().play_from(start);
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
    pub(super) fn range(&self) -> EpisodeTimeRange {
        *self.range.read()
    }
    pub(super) fn seconds(&self) -> f64 {
        self.clock.read().seconds()
    }
    pub(super) fn seek(&mut self, seconds: f64) {
        self.clock.write().seek(seconds);
    }
    fn pause(&mut self) {
        self.clock.write().set_playing(false);
    }
    fn set_speed(&mut self, speed: f64) {
        self.clock.write().set_speed(speed);
    }
    pub(super) fn view(&self) -> ViewWindow {
        *self.view.read()
    }
    pub(super) fn report(&mut self, topic: &str, state: FeedState) {
        if self.feeds.peek().get(topic) != Some(&state) {
            self.feeds.write().insert(topic.to_owned(), state);
        }
    }
    /// Stop waiting for a feed, such as a camera on a tab nobody is looking at.
    pub(super) fn forget(&mut self, topic: &str) {
        if self.feeds.peek().contains_key(topic) {
            self.feeds.write().remove(topic);
        }
    }
    /// Space, the arrows and Home/End also drive this clock while focus rests
    /// on the page itself, as long as its activity is on screen. A key
    /// pressed inside the inspection is handled there and never reaches this
    /// listener as a page key.
    pub(super) fn use_page_keys(&self) {
        let activity = crate::shell::activity::WorkspaceActivity::current();
        let mut transport = self.clone();
        PageKeys::use_listener(move |event| {
            PageKeys::on_page(event)
                && activity.is_active()
                && !KeyboardFocus::editing()
                && transport.handle_key(event)
        });
    }
    /// Shared keyboard transport: Space, arrows, Home/End, zoom keys, from a
    /// workspace event or from a listener on the page itself.
    /// Returns whether the key was consumed.
    pub(super) fn handle_key(&mut self, key: impl Into<TransportKey>) -> bool {
        if let Some(action) = self.timeline_state().key_action(key.into()) {
            self.apply(action);
            true
        } else {
            false
        }
    }
    pub(super) fn timeline_state(&self) -> TimelineState {
        TimelineState::new(Self::TIMELINE_TARGET, *self.clock.read())
            .with_window(self.view())
            .with_buffering(self.buffering())
    }
    pub(super) fn interaction(&self) -> TimelineInteraction {
        let mut transport = self.clone();
        TimelineInteraction::new(
            self.timeline_state(),
            EventHandler::new(move |event: TimelineEvent| transport.apply(event.action())),
        )
    }
    fn apply(&mut self, action: TimelineAction) {
        match action {
            TimelineAction::Seek { seconds, .. } => self.seek(seconds),
            TimelineAction::SetPlaying(playing) => self.clock.write().set_playing(playing),
            TimelineAction::SetSpeed(speed) => self.set_speed(speed),
            TimelineAction::SetLooping(looping) => self.clock.write().set_looping(looping),
            TimelineAction::SetWindow(window) => self.view.set(window),
            TimelineAction::ActivateInterval(interval) => self.play_from(interval.span().0),
        }
    }
    fn buffering(&self) -> bool {
        self.feeds
            .read()
            .values()
            .any(|state| *state == FeedState::Loading)
    }
}
