//! The selected episode owns one clock, one view window, and shared media limits.
//!
//! Every Data panel — cameras, plots, the timeline, messages — reads the same
//! playhead and the same visible span. Nothing here fetches data; the transport
//! only says *when* the operator is looking.
use super::episode_decoder::MediaBudget;
pub(super) use crate::components::timeline::KeyboardFocus;
use crate::components::timeline::{
    PlaybackClock, TimelineAction, TimelineEvent, TimelineInteraction, TimelineState, TransportKey,
    ViewWindow,
};
use dioxus::prelude::*;
use std::{collections::BTreeMap, rc::Rc};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct EpisodeTimeRange {
    start_ns: u64,
    end_ns: u64,
}
impl EpisodeTimeRange {
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
    pub(super) fn seconds_of(self, absolute_ns: u64) -> f64 {
        (i128::from(absolute_ns) - i128::from(self.start_ns)) as f64 / 1e9
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
pub(crate) struct EpisodeTransport {
    clock: Signal<PlaybackClock>,
    view: Signal<ViewWindow>,
    range: Signal<EpisodeTimeRange>,
    feeds: Signal<BTreeMap<String, FeedState>>,
    media: Rc<EpisodeMedia>,
    generation: Signal<u64>,
}
impl PartialEq for EpisodeTransport {
    fn eq(&self, other: &Self) -> bool {
        self.clock == other.clock
    }
}
impl EpisodeTransport {
    /// One caller-owned clock for an inline episode camera and its timeline.
    pub(in crate::activities) fn use_opened_episode(
        episode: &super::episode_list::OpenedEpisode,
    ) -> Self {
        let range = episode
            .record()
            .time_range()
            .unwrap_or_else(|| EpisodeTimeRange::new(0, 0));
        let transport = Self::use_range(range);
        let mut updating = transport.clone();
        use_effect(use_reactive(&range, move |range| {
            if updating.range() != range {
                updating.load(range);
            }
        }));
        transport
    }

    pub(in crate::activities) fn play_interval(&mut self, start: f64, end: f64) {
        self.clock.write().play_interval(start, end);
    }
    /// One transport per Data workspace; `load` points it at an episode.
    pub(super) fn use_transport() -> Self {
        Self::use_range(EpisodeTimeRange::new(0, 0))
    }
    fn use_range(initial: EpisodeTimeRange) -> Self {
        let clock = use_signal(|| PlaybackClock::new(initial.duration()));
        let view = use_signal(|| ViewWindow::full(initial.duration()));
        let range = use_signal(|| initial);
        let feeds = use_signal(BTreeMap::new);
        let media = use_hook(|| {
            Rc::new(EpisodeMedia {
                encoded: MediaBudget::new(128 * 1024 * 1024),
                // Accounted at four bytes per coded pixel; browsers keep H.264
                // output as NV12/I420, so this admits roughly three visible
                // 720p cameras with 30-frame keyframe intervals at once.
                decoded: MediaBudget::new(512 * 1024 * 1024),
            })
        });
        let transport = Self {
            clock,
            view,
            range,
            feeds,
            media,
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
    /// Point the shared clock at another episode, from its beginning.
    pub(super) fn load(&mut self, range: EpisodeTimeRange) {
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
    pub(super) fn seconds(&self) -> f64 {
        self.clock.read().seconds()
    }
    /// The playhead as an absolute log time, for latest-message lookups.
    pub(super) fn absolute_ns(&self) -> u64 {
        self.range().ns_at(self.seconds())
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
    use std::cell::RefCell;

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
