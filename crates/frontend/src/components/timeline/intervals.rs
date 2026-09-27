//! A timeline row of named spans: the annotated subtasks of one episode, and
//! any other derived segmentation an activity wants to read and click.
//!
//! Numeric tracks draw intervals as anonymous overlays over a series. This row
//! has no series: the spans themselves are the content, so each carries its
//! own name and activating one returns it unchanged through
//! [`TimelineAction::ActivateInterval`]. The row shares the timeline's time
//! coordinates through [`TimelineScrubber`] and the interaction's view window,
//! exactly as `NumericTrackRow` does.
use super::{TimelineAction, TimelineInteraction, TimelineInterval, TimelineScrubber};
use dioxus::prelude::*;

/// One row of named spans over the episode's time axis, and which of them the
/// surface has chosen.
#[derive(Clone, PartialEq)]
pub(crate) struct IntervalTrack {
    id: String,
    label: String,
    spans: Vec<TimelineInterval>,
    /// Where the chosen span starts, which is how a surface names its row.
    chosen: Option<f64>,
}

impl IntervalTrack {
    /// Spans that do not describe a forward time range are dropped, so a row
    /// never renders a segment the operator cannot click.
    pub(crate) fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        spans: Vec<TimelineInterval>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            spans: spans.into_iter().filter(TimelineInterval::valid).collect(),
            chosen: None,
        }
    }
    /// Mark the span starting at `start` as the one the surface has chosen:
    /// the row its index shows as selected.
    pub(crate) fn with_chosen(mut self, start: Option<f64>) -> Self {
        self.chosen = start;
        self
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
    /// The span the playhead is inside, if it is inside one.
    fn at(&self, seconds: f64) -> Option<&TimelineInterval> {
        self.spans.iter().find(|span| {
            let (start, end) = span.span();
            start <= seconds && seconds < end
        })
    }
}

/// The spans as the lane draws them, and the one the playhead is in named in
/// full beside the lane: a span a few pixels wide cannot say its own name, and
/// the name is what the operator is checking against the picture.
#[component]
pub(crate) fn IntervalTrackRow(track: IntervalTrack, interaction: TimelineInteraction) -> Element {
    let view = interaction.state().window();
    let now = interaction.state().clock().seconds();
    let current = track.at(now).map(|span| span.span().0);
    let reading = track.at(now).map(|span| span.label().to_owned());
    let visible = track
        .spans
        .iter()
        // Keyed by place in the track: spans that overlap may share a start.
        .enumerate()
        .filter(|(_, span)| span.span().1 > view.start() && span.span().0 < view.end());
    rsx! {
        div { class: "data-stream data-interval-track", "aria-label": track.label.clone(),
            div { class: "data-stream-name data-interval-name",
                strong { "{track.label}" }
                output { class: "data-interval-now", title: reading.clone(), {reading.clone().unwrap_or_default()} }
            }
            div { class: "data-stream-track data-interval-lane",
                TimelineScrubber { interaction: interaction.clone() }
                for (place, span) in visible {
                    button { key: "{place}", r#type: "button",
                        class: "data-interval",
                        "data-chosen": (track.chosen == Some(span.span().0)).then_some("true"),
                        "data-current": (current == Some(span.span().0)).then_some("true"),
                        "data-tinted": span.tint().is_some().then_some("true"),
                        // A hairline apart from its neighbour, since spans
                        // of one measurement touch end to start.
                        style: "left:{view.percent(span.span().0):.3}%;width:max(2px, calc({view.percent(span.span().1) - view.percent(span.span().0):.3}% - 2px));{span.tint().unwrap_or_default()}",
                        title: "{span.label()}", "aria-label": "{span.label()}",
                        onclick: {
                            let interaction = interaction.clone();
                            let span = (*span).clone();
                            move |_| interaction.emit(TimelineAction::ActivateInterval(span.clone()))
                        },
                        span { class: "data-interval-label", "{span.label()}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_row_keeps_only_spans_that_describe_a_forward_range() {
        let track = IntervalTrack::new(
            "subtasks",
            "Subtasks",
            vec![
                TimelineInterval::labeled(0.0, 8.0, "grasp the corners"),
                TimelineInterval::labeled(8.0, 8.0, "empty"),
                TimelineInterval::labeled(f64::NAN, 3.0, "unmeasured"),
                TimelineInterval::labeled(8.0, 13.0, "make the first fold"),
            ],
        );
        assert_eq!(track.spans.len(), 2);
        assert_eq!(track.spans[1].span(), (8.0, 13.0));
        assert!(!track.is_empty());
        assert!(IntervalTrack::new("none", "None", Vec::new()).is_empty());
        assert_eq!(
            track.at(9.0).map(TimelineInterval::label),
            Some("make the first fold")
        );
        assert_eq!(
            track.at(8.0).map(TimelineInterval::label),
            Some("make the first fold")
        );
        assert!(track.at(13.0).is_none());
    }
}
