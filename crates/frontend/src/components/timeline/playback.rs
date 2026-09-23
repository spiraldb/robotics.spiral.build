//! Local playback math and shared time coordinates. No media or activity dependencies.
use crate::format::{ClockPresentation, PreciseClockPresentation};
use dioxus::prelude::*;

pub(crate) const PLAYBACK_SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct PlaybackClock {
    seconds: f64,
    playing: bool,
    duration: f64,
    speed: f64,
    looping: bool,
    stop_at: Option<f64>,
}
impl PlaybackClock {
    pub(crate) fn new(duration: f64) -> Self {
        Self {
            seconds: 0.0,
            playing: false,
            duration: duration.max(0.0),
            speed: 1.0,
            looping: false,
            stop_at: None,
        }
    }
    pub(crate) fn seek(&mut self, seconds: f64) {
        if seconds.is_finite() {
            self.stop_at = None;
            self.seconds = seconds.clamp(0.0, self.duration);
        }
    }
    pub(crate) fn play_interval(&mut self, start: f64, end: f64) {
        if !start.is_finite() || !end.is_finite() || end <= start {
            return;
        }
        self.seek(start);
        let end = end.clamp(self.seconds, self.duration);
        self.stop_at = Some(end);
        self.playing = end > self.seconds;
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
    pub(crate) fn advance(&mut self, elapsed: f64) {
        let step = elapsed.clamp(0.0, 0.25).min(self.duration) * self.speed;
        let next = self.seconds + step;
        if let Some(end) = self.stop_at.filter(|end| next >= *end) {
            self.seconds = end;
            self.playing = false;
            self.stop_at = None;
        } else if next >= self.duration {
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
    /// One browser timer for local playback. Callers decide whether a tick may
    /// advance their clock (for example, when media is buffering).
    pub(crate) fn use_ticker(active: ReadSignal<bool>, on_tick: EventHandler<f64>) {
        let mut previous = use_signal(|| None::<f64>);
        let _ = active();
        let mut timer = use_future(move || async move {
            let performance = web_sys::window().and_then(|window| window.performance());
            loop {
                gloo_timers::future::TimeoutFuture::new(16).await;
                let now = performance.as_ref().map_or(0.0, |time| time.now() / 1000.0);
                let elapsed = previous
                    .replace(Some(now))
                    .map_or(0.0, |then| (now - then).clamp(0.0, 0.25));
                on_tick.call(elapsed);
            }
        });
        use_hook_did_run(move |did_run| {
            if did_run && active() {
                timer.resume();
            } else {
                timer.pause();
                previous.set(None);
            }
        });
    }
    pub(crate) fn seconds(self) -> f64 {
        self.seconds
    }
    pub(super) fn duration(self) -> f64 {
        self.duration
    }
    pub(crate) fn playing(self) -> bool {
        self.playing
    }
    pub(super) fn speed(self) -> f64 {
        self.speed
    }
    pub(super) fn looping(self) -> bool {
        self.looping
    }
    pub(crate) fn set_playing(&mut self, playing: bool) {
        if playing != self.playing {
            self.toggle();
        }
    }
    pub(crate) fn set_speed(&mut self, speed: f64) {
        if PLAYBACK_SPEEDS.contains(&speed) {
            self.speed = speed;
        }
    }
    pub(crate) fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
    }
}

/// The span of the episode currently spread across the timeline and plots.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct ViewWindow {
    start: f64,
    end: f64,
}
impl ViewWindow {
    const MIN_SPAN: f64 = 0.05;
    pub(crate) fn full(duration: f64) -> Self {
        Self {
            start: 0.0,
            end: duration.max(Self::MIN_SPAN),
        }
    }
    pub(crate) fn start(self) -> f64 {
        self.start
    }
    pub(crate) fn end(self) -> f64 {
        self.end
    }
    pub(super) fn span(self) -> f64 {
        self.end - self.start
    }
    /// Horizontal position of a moment, in percent of the visible width.
    pub(crate) fn percent(self, seconds: f64) -> f64 {
        if !seconds.is_finite() || self.span() <= 0.0 {
            return 0.0;
        }
        ((seconds - self.start) / self.span() * 100.0).clamp(0.0, 100.0)
    }
    pub(crate) fn seconds_at(self, fraction: f64) -> f64 {
        self.start + self.span() * fraction.clamp(0.0, 1.0)
    }
    pub(super) fn zoomed(self, factor: f64, anchor: f64, duration: f64) -> Self {
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
    pub(super) fn panned(self, delta: f64, duration: f64) -> Self {
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
    pub(crate) fn ticks(self) -> Vec<TimeTick> {
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
pub(crate) struct TimeTick {
    seconds: f64,
    position: f64,
    label: String,
}
impl TimeTick {
    pub(crate) fn seconds(&self) -> f64 {
        self.seconds
    }
    pub(crate) fn position(&self) -> f64 {
        self.position
    }
    pub(crate) fn label(&self) -> &str {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn interval_review_stops_exactly_and_manual_seek_clears_boundary() {
        let mut clock = PlaybackClock::new(30.0);
        clock.looping = true;
        clock.play_interval(23.9, 24.0);
        clock.advance(0.2);
        assert_eq!(clock.seconds, 24.0);
        assert!(!clock.playing);
        clock.play_interval(23.0, 24.0);
        clock.seek(25.0);
        assert_eq!(clock.stop_at, None);
        clock.advance(0.2);
        assert!(clock.seconds > 25.0);
    }
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
