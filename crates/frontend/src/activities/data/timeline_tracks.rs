//! Typed derived tracks on the episode timeline's shared time coordinates.
use super::{episode_transport::EpisodeTransport, timeline_panel::TrackScrubber};
use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
pub(crate) struct TimelineSample {
    seconds: f64,
    value: f64,
}
impl TimelineSample {
    pub(crate) fn new(seconds: f64, value: f64) -> Self {
        Self { seconds, value }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum IntervalKind {
    Excluded,
    Unknown,
}
impl IntervalKind {
    fn key(self) -> &'static str {
        match self {
            Self::Excluded => "excluded",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct TimelineInterval {
    span: (f64, f64),
    label: String,
    kind: IntervalKind,
    context: f64,
}
impl TimelineInterval {
    pub(crate) fn excluded(start: f64, end: f64, label: impl Into<String>) -> Self {
        Self {
            span: (start, end),
            label: label.into(),
            kind: IntervalKind::Excluded,
            context: 0.0,
        }
    }
    pub(crate) fn unknown(start: f64, end: f64, label: impl Into<String>) -> Self {
        Self {
            span: (start, end),
            label: label.into(),
            kind: IntervalKind::Unknown,
            context: 0.0,
        }
    }
    /// Playback may include context without moving the annotation itself.
    pub(crate) fn with_context(mut self, seconds: f64) -> Self {
        self.context = if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        };
        self
    }
    fn valid(&self) -> bool {
        self.span.0.is_finite() && self.span.1.is_finite() && self.span.1 > self.span.0
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct NumericTrack {
    samples: Vec<TimelineSample>,
    cutoff: Option<f64>,
    intervals: Vec<TimelineInterval>,
}
impl NumericTrack {
    pub(crate) fn new(
        samples: Vec<TimelineSample>,
        cutoff: Option<f64>,
        intervals: Vec<TimelineInterval>,
    ) -> Self {
        let mut samples = samples;
        samples.retain(|sample| sample.seconds.is_finite() && sample.seconds >= 0.0);
        samples.sort_by(|left, right| left.seconds.total_cmp(&right.seconds));
        Self {
            samples,
            cutoff: cutoff.filter(|value| value.is_finite()),
            intervals: intervals
                .into_iter()
                .filter(TimelineInterval::valid)
                .collect(),
        }
    }
    fn at(&self, seconds: f64) -> Option<f64> {
        let index = self
            .samples
            .partition_point(|sample| sample.seconds <= seconds);
        index
            .checked_sub(1)
            .and_then(|index| self.samples.get(index))
            .map(|sample| sample.value)
            .filter(|value| value.is_finite())
    }
    fn domain(&self) -> (f64, f64) {
        let values = self
            .samples
            .iter()
            .map(|sample| sample.value)
            .chain(self.cutoff)
            .filter(|value| value.is_finite());
        let (min, max) = values.fold((0.0_f64, 0.0_f64), |(min, max), value| {
            (min.min(value), max.max(value))
        });
        if min == max {
            (min - 0.5, max + 0.5)
        } else {
            (min, max)
        }
    }
    fn y(&self, value: f64) -> f64 {
        let (min, max) = self.domain();
        88.0 - (value - min) / (max - min) * 80.0
    }
    fn path(&self) -> String {
        let (min, max) = self.domain();
        let mut path = String::new();
        let mut connected = false;
        for sample in &self.samples {
            if !sample.value.is_finite() {
                connected = false;
                continue;
            }
            let command = if connected { 'L' } else { 'M' };
            let y = 88.0 - (sample.value - min) / (max - min) * 80.0;
            use std::fmt::Write as _;
            let _ = write!(path, "{command}{:.6},{y:.3} ", sample.seconds);
            connected = true;
        }
        path
    }
}

#[derive(Clone, PartialEq)]
pub(crate) enum TimelineTrackState {
    Loading(String),
    Failed(String),
    Ready(NumericTrack),
}

#[derive(Clone, PartialEq)]
pub(crate) struct TimelineTrack {
    id: String,
    label: String,
    state: TimelineTrackState,
}
impl TimelineTrack {
    pub(crate) fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        state: TimelineTrackState,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            state,
        }
    }
    pub(super) fn id(&self) -> &str {
        &self.id
    }
}

#[component]
pub(super) fn NumericTrackRow(track: TimelineTrack, transport: EpisodeTransport) -> Element {
    let view = transport.view();
    let value = match &track.state {
        TimelineTrackState::Ready(series) => series.at(transport.seconds()),
        _ => None,
    };
    let path = use_memo(use_reactive((&track.state,), move |(state,)| match state {
        TimelineTrackState::Ready(series) => series.path(),
        _ => String::new(),
    }));
    rsx! {
        div { class: "data-stream data-numeric-track", "aria-label": track.label.clone(),
            div { class: "data-stream-name data-numeric-name",
                strong { "{track.label}" }
                if let Some(value) = value { output { "{value:.3}" } }
                if let TimelineTrackState::Ready(series) = &track.state {
                    if let Some(cutoff) = series.cutoff { span { "Cutoff {cutoff}" } }
                }
            }
            div { class: "data-stream-track data-numeric-plot",
                match track.state {
                    TimelineTrackState::Loading(message) => rsx! { p { class: "data-numeric-status", role: "status", "{message}" } },
                    TimelineTrackState::Failed(message) => rsx! { p { class: "data-numeric-status", role: "alert", "{message}" } },
                    TimelineTrackState::Ready(series) => rsx! {
                        svg { view_box: "{view.start()} 0 {view.span()} 100", preserve_aspect_ratio: "none", role: "img", "aria-label": "{track.label} over episode time",
                            for interval in &series.intervals {
                                rect { x: "{interval.span.0}", width: "{interval.span.1 - interval.span.0}", y: "0", height: "100", class: "data-numeric-overlay", "data-kind": interval.kind.key() }
                            }
                            if let Some(cutoff) = series.cutoff {
                                line { x1: "{view.start()}", x2: "{view.end()}", y1: "{series.y(cutoff)}", y2: "{series.y(cutoff)}", class: "data-numeric-cutoff", vector_effect: "non-scaling-stroke" }
                            }
                            path { d: "{path}", class: "data-numeric-line", vector_effect: "non-scaling-stroke" }
                        }
                        TrackScrubber { transport: transport.clone() }
                        for interval in series.intervals.iter().filter(|interval| interval.span.1 > view.start() && interval.span.0 < view.end()) {
                            button { r#type: "button", class: "data-numeric-interval", "data-kind": interval.kind.key(),
                                style: "left:{view.percent(interval.span.0)}%;width:{view.percent(interval.span.1) - view.percent(interval.span.0)}%",
                                title: "{interval.label} · Play interval", "aria-label": "{interval.label} · Play interval",
                                onclick: { let mut transport = transport.clone(); let (start, end) = (interval.span.0 - interval.context, interval.span.1 + interval.context); move |_| transport.play_interval(start, end) },
                            }
                        }
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn series_order_gaps_and_cutoff_share_time_coordinates() {
        let track = NumericTrack::new(
            vec![
                TimelineSample::new(2.0, 3.0),
                TimelineSample::new(0.0, 1.0),
                TimelineSample::new(1.0, f64::NAN),
            ],
            Some(4.0),
            Vec::new(),
        );
        assert_eq!(track.at(0.5), Some(1.0));
        assert_eq!(track.at(1.5), None);
        assert_eq!(track.at(2.0), Some(3.0));
        assert_eq!(track.domain(), (0.0, 4.0));
        assert_eq!(track.path().matches('M').count(), 2);
    }
}
