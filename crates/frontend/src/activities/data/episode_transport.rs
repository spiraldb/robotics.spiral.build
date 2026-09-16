//! The selected episode owns one clock, one view window, and shared media limits.
//!
//! Every Data panel — cameras, plots, the timeline, messages — reads the same
//! playhead and the same visible span. Nothing here fetches data; the transport
//! only says *when* the operator is looking.
use super::episode_decoder::MediaBudget;
use crate::format::{ClockPresentation, PreciseClockPresentation};
use dioxus::prelude::*;
use std::{collections::BTreeMap, rc::Rc};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct EpisodeTimeRange {
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
    pub(super) fn start_ns(self) -> u64 {
        self.start_ns
    }
    pub(super) fn duration(self) -> f64 {
        self.end_ns.saturating_sub(self.start_ns) as f64 / 1e9
    }
    pub(super) fn seconds_of(self, absolute_ns: u64) -> f64 {
        (i128::from(absolute_ns) - i128::from(self.start_ns)) as f64 / 1e9
    }
    pub(super) fn ns_at(self, seconds: f64) -> u64 {
        let offset = (seconds.max(0.0) * 1e9).round();
        self.start_ns.saturating_add(offset as u64)
    }
}

pub(super) const PLAYBACK_SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

#[derive(Clone, Copy, PartialEq, Debug)]
struct PlaybackClock {
    seconds: f64,
    playing: bool,
    duration: f64,
    speed: f64,
    looping: bool,
}
impl PlaybackClock {
    fn new(duration: f64) -> Self {
        Self {
            seconds: 0.0,
            playing: false,
            duration: duration.max(0.0),
            speed: 1.0,
            looping: false,
        }
    }
    fn seek(&mut self, seconds: f64) {
        if seconds.is_finite() {
            self.seconds = seconds.clamp(0.0, self.duration);
        }
    }
    fn toggle(&mut self) {
        if self.duration <= 0.0 {
            return;
        }
        if !self.playing && self.seconds >= self.duration {
            self.seconds = 0.0;
        }
        self.playing = !self.playing;
    }
    fn advance(&mut self, elapsed: f64) {
        let step = crate::components::preview_clock::PreviewClock::new(self.duration, Some(0.0))
            .advance(0.0, elapsed)
            * self.speed;
        let next = self.seconds + step;
        if next >= self.duration {
            if self.looping && self.duration > 0.0 {
                self.seconds = next % self.duration;
            } else {
                self.seconds = self.duration;
                self.playing = false;
            }
        } else {
            self.seconds = next;
        }
    }
}

/// The span of the episode currently spread across the timeline and plots.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct ViewWindow {
    start: f64,
    end: f64,
}
impl ViewWindow {
    const MIN_SPAN: f64 = 0.05;
    fn full(duration: f64) -> Self {
        Self {
            start: 0.0,
            end: duration.max(Self::MIN_SPAN),
        }
    }
    pub(super) fn start(self) -> f64 {
        self.start
    }
    pub(super) fn end(self) -> f64 {
        self.end
    }
    pub(super) fn span(self) -> f64 {
        self.end - self.start
    }
    /// Horizontal position of a moment, in percent of the visible width.
    pub(super) fn percent(self, seconds: f64) -> f64 {
        if !seconds.is_finite() || self.span() <= 0.0 {
            return 0.0;
        }
        ((seconds - self.start) / self.span() * 100.0).clamp(0.0, 100.0)
    }
    pub(super) fn seconds_at(self, fraction: f64) -> f64 {
        self.start + self.span() * fraction.clamp(0.0, 1.0)
    }
    fn zoomed(self, factor: f64, anchor: f64, duration: f64) -> Self {
        let span = (self.span() * factor).clamp(Self::MIN_SPAN, duration.max(Self::MIN_SPAN));
        let anchor = anchor.clamp(self.start, self.end);
        let ratio = if self.span() > 0.0 {
            (anchor - self.start) / self.span()
        } else {
            0.5
        };
        let start = anchor - span * ratio;
        Self::clamped(start, start + span, duration)
    }
    fn panned(self, delta: f64, duration: f64) -> Self {
        Self::clamped(self.start + delta, self.end + delta, duration)
    }
    fn clamped(start: f64, end: f64, duration: f64) -> Self {
        let duration = duration.max(Self::MIN_SPAN);
        let span = (end - start).clamp(Self::MIN_SPAN, duration);
        let start = start.clamp(0.0, duration - span);
        Self {
            start,
            end: start + span,
        }
    }
    /// Labelled divisions at a round interval, about eight across the span.
    pub(super) fn ticks(self) -> Vec<TimeTick> {
        const STEPS: [f64; 19] = [
            0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0,
            60.0, 120.0, 300.0, 600.0,
        ];
        let span = self.span();
        let step = STEPS
            .into_iter()
            .find(|step| span / step <= 8.0)
            .unwrap_or(600.0);
        let first = (self.start / step).ceil() as i64;
        let last = (self.end / step).floor() as i64;
        (first..=last)
            .map(|index| {
                let seconds = index as f64 * step;
                TimeTick {
                    seconds,
                    position: self.percent(seconds),
                    label: step.tick_label(seconds),
                }
            })
            .collect()
    }
}

#[derive(Clone, PartialEq, Debug)]
pub(super) struct TimeTick {
    seconds: f64,
    position: f64,
    label: String,
}
impl TimeTick {
    pub(super) fn seconds(&self) -> f64 {
        self.seconds
    }
    pub(super) fn position(&self) -> f64 {
        self.position
    }
    pub(super) fn label(&self) -> &str {
        &self.label
    }
}

trait TickStep {
    fn tick_label(self, seconds: f64) -> String;
}
impl TickStep for f64 {
    fn tick_label(self, seconds: f64) -> String {
        if self >= 1.0 {
            let minutes = (seconds / 60.0).floor() as u64;
            let remainder = (seconds - minutes as f64 * 60.0).round() as u64;
            format!("{minutes}:{remainder:02}")
        } else if self >= 0.1 {
            seconds.clock_label()
        } else {
            seconds.precise_clock_label()
        }
    }
}

/// Transport shortcuts must not steal keys from text entry or menus.
pub(super) struct KeyboardFocus;
impl KeyboardFocus {
    fn active_tag() -> Option<(String, Option<String>)> {
        let element = web_sys::window()?.document()?.active_element()?;
        if element.get_attribute("contenteditable").is_some() {
            return Some(("CONTENTEDITABLE".into(), None));
        }
        Some((element.tag_name(), element.get_attribute("type")))
    }
    pub(super) fn editing() -> bool {
        Self::active_tag().is_some_and(|(tag, kind)| {
            matches!(
                tag.as_str(),
                "INPUT" | "SELECT" | "TEXTAREA" | "SUMMARY" | "CONTENTEDITABLE"
            ) && kind.as_deref() != Some("range")
        })
    }
    /// A focused button already answers Space with a click.
    fn on_button() -> bool {
        Self::active_tag().is_some_and(|(tag, _)| tag == "BUTTON")
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
    view: Signal<ViewWindow>,
    range: Signal<EpisodeTimeRange>,
    feeds: Signal<BTreeMap<String, FeedState>>,
    media: Rc<EpisodeMedia>,
}
impl PartialEq for EpisodeTransport {
    fn eq(&self, other: &Self) -> bool {
        self.clock == other.clock
    }
}
impl EpisodeTransport {
    /// One transport per Data workspace; `load` points it at an episode.
    pub(super) fn use_transport() -> Self {
        let clock = use_signal(|| PlaybackClock::new(0.0));
        let view = use_signal(|| ViewWindow::full(0.0));
        let range = use_signal(|| EpisodeTimeRange::new(0, 0));
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
        };
        let running = transport.clone();
        let activity = crate::shell::activity::WorkspaceActivity::current();
        let mut visibility_clock = clock;
        use_effect(move || {
            if !activity.is_active() {
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
                    } else if !running.buffering() {
                        running.clock.write().advance(elapsed);
                    }
                }
            }
        });
        transport
    }
    /// Point the shared clock at another episode, from its beginning.
    pub(super) fn load(&mut self, range: EpisodeTimeRange) {
        let duration = range.duration();
        let speed = self.clock.peek().speed;
        let looping = self.clock.peek().looping;
        let mut clock = PlaybackClock::new(duration);
        clock.speed = speed;
        clock.looping = looping;
        self.clock.set(clock);
        self.view.set(ViewWindow::full(duration));
        self.range.set(range);
        self.feeds.write().clear();
    }
    pub(super) fn range(&self) -> EpisodeTimeRange {
        *self.range.read()
    }
    pub(super) fn duration(&self) -> f64 {
        self.clock.read().duration
    }
    pub(super) fn seconds(&self) -> f64 {
        self.clock.read().seconds
    }
    /// The playhead as an absolute log time, for latest-message lookups.
    pub(super) fn absolute_ns(&self) -> u64 {
        self.range().ns_at(self.seconds())
    }
    pub(super) fn playing(&self) -> bool {
        self.clock.read().playing
    }
    pub(super) fn speed(&self) -> f64 {
        self.clock.read().speed
    }
    pub(super) fn looping(&self) -> bool {
        self.clock.read().looping
    }
    pub(super) fn seek(&mut self, seconds: f64) {
        self.clock.write().seek(seconds);
    }
    pub(super) fn step(&mut self, delta: f64) {
        let seconds = self.clock.peek().seconds + delta;
        self.clock.write().seek(seconds);
    }
    pub(super) fn pause(&mut self) {
        self.clock.write().playing = false;
    }
    pub(super) fn toggle(&mut self) {
        self.clock.write().toggle();
    }
    pub(super) fn set_speed(&mut self, speed: f64) {
        if PLAYBACK_SPEEDS.contains(&speed) {
            self.clock.write().speed = speed;
        }
    }
    pub(super) fn toggle_loop(&mut self) {
        let looping = self.clock.peek().looping;
        self.clock.write().looping = !looping;
    }
    pub(super) fn view(&self) -> ViewWindow {
        *self.view.read()
    }
    pub(super) fn zoom(&mut self, factor: f64, anchor: f64) {
        let duration = self.duration();
        let next = self.view.peek().zoomed(factor, anchor, duration);
        self.view.set(next);
    }
    pub(super) fn pan(&mut self, delta: f64) {
        let duration = self.duration();
        let next = self.view.peek().panned(delta, duration);
        self.view.set(next);
    }
    pub(super) fn fit(&mut self) {
        let duration = self.duration();
        self.view.set(ViewWindow::full(duration));
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
    /// Shared keyboard transport: Space, arrows, Home/End, zoom keys.
    /// Returns whether the key was consumed.
    pub(super) fn handle_key(&mut self, event: &KeyboardEvent) -> bool {
        if self.duration() <= 0.0 {
            return false;
        }
        let modifiers = event.modifiers();
        let step = if modifiers.shift() {
            1.0
        } else if modifiers.alt() {
            0.01
        } else {
            0.1
        };
        match event.key() {
            Key::Character(character) if character == " " => {
                if KeyboardFocus::on_button() {
                    return false;
                }
                if !event.is_auto_repeating() {
                    self.toggle();
                }
            }
            Key::ArrowLeft => self.step(-step),
            Key::ArrowRight => self.step(step),
            Key::Home => self.seek(0.0),
            Key::End => {
                let end = self.duration();
                self.seek(end);
            }
            Key::Character(character) if character == "+" || character == "=" => {
                let anchor = self.seconds();
                self.zoom(0.5, anchor);
            }
            Key::Character(character) if character == "-" => {
                let anchor = self.seconds();
                self.zoom(2.0, anchor);
            }
            Key::Character(character) if character == "0" => self.fit(),
            _ => return false,
        }
        true
    }
    pub(super) fn buffering(&self) -> bool {
        self.feeds
            .read()
            .values()
            .any(|state| *state == FeedState::Loading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn one_clock_clamps_seeks_loops_and_honours_speed() {
        let mut clock = PlaybackClock::new(2.0);
        clock.seek(3.0);
        assert_eq!(clock.seconds, 2.0);
        clock.toggle();
        assert_eq!(clock.seconds, 0.0);
        assert!(clock.playing);
        clock.speed = 2.0;
        clock.advance(0.1);
        assert!((clock.seconds - 0.2).abs() < 1e-9);
        clock.seek(1.9);
        clock.advance(0.2);
        assert_eq!(clock.seconds, 2.0);
        assert!(!clock.playing);
        clock.looping = true;
        clock.seek(1.9);
        clock.playing = true;
        clock.advance(0.1);
        assert!(clock.playing);
        assert!(clock.seconds < 0.2);
        clock.seek(f64::NAN);
        assert!(clock.seconds.is_finite());
        clock.seek(-1.0);
        assert_eq!(clock.seconds, 0.0);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn view_window_zooms_around_the_anchor_and_stays_inside_the_episode() {
        let full = ViewWindow::full(20.0);
        let zoomed = full.zoomed(0.5, 5.0, 20.0);
        assert!((zoomed.span() - 10.0).abs() < 1e-9);
        assert!((zoomed.start - 2.5).abs() < 1e-9);
        let panned = zoomed.panned(100.0, 20.0);
        assert_eq!(panned.end, 20.0);
        let tiny = full.zoomed(1e-9, 5.0, 20.0);
        assert!((tiny.span() - ViewWindow::MIN_SPAN).abs() < 1e-9);
        assert_eq!(full.zoomed(4.0, 5.0, 20.0), full);
        assert_eq!(full.percent(5.0), 25.0);
        assert_eq!(full.seconds_at(0.25), 5.0);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn ticks_pick_a_round_step_for_the_visible_span() {
        let ticks = ViewWindow::full(20.5).ticks();
        assert_eq!(ticks.first().map(TimeTick::label), Some("0:00"));
        assert_eq!(ticks.len(), 5);
        assert_eq!(ticks[1].seconds(), 5.0);
        let fine = ViewWindow {
            start: 4.0,
            end: 4.5,
        }
        .ticks();
        assert_eq!(fine[0].label(), "0:04.0");
        let sub = ViewWindow {
            start: 4.0,
            end: 4.05,
        }
        .ticks();
        assert_eq!(sub[0].label(), "0:04.000");
        assert!(!ViewWindow::full(0.0).ticks().is_empty());
    }
}
