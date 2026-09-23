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

/// The height a span row needs for one line of names, in pixels.
const ROW_HEIGHT: u32 = 28;

/// One row of named spans over the episode's time axis.
#[derive(Clone, PartialEq)]
pub(crate) struct IntervalTrack {
    id: String,
    label: String,
    spans: Vec<TimelineInterval>,
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
        }
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

#[component]
pub(crate) fn IntervalTrackRow(track: IntervalTrack, interaction: TimelineInteraction) -> Element {
    let view = interaction.state().window();
    let visible = track
        .spans
        .iter()
        .filter(|span| span.span().1 > view.start() && span.span().0 < view.end());
    rsx! {
        div { class: "data-stream", "aria-label": track.label.clone(),
            div { class: "data-stream-name data-interval-name", style: "height:{ROW_HEIGHT}px",
                strong { "{track.label}" }
            }
            div { class: "data-stream-track", style: "height:{ROW_HEIGHT}px",
                TimelineScrubber { interaction: interaction.clone() }
                for (index, span) in visible.enumerate() {
                    button { key: "{index}-{span.span().0}", r#type: "button",
                        "data-kind": span.kind_key(),
                        style: "position:absolute;top:3px;bottom:3px;left:{view.percent(span.span().0)}%;width:{(view.percent(span.span().1) - view.percent(span.span().0)).max(0.4)}%;display:flex;align-items:center;overflow:hidden;padding:0 4px;border:1px solid var(--tone-accent);border-radius:2px;background:var(--tone-selected);color:var(--tone-ink);font-size:10px;line-height:1;white-space:nowrap;cursor:pointer",
                        title: "{span.label()}", "aria-label": "{span.label()}",
                        onclick: {
                            let interaction = interaction.clone();
                            let span = (*span).clone();
                            move |_| interaction.emit(TimelineAction::ActivateInterval(span.clone()))
                        },
                        "{span.label()}"
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
        assert_eq!(track.spans[1].kind_key(), "labeled");
        assert!(!track.is_empty());
        assert!(IntervalTrack::new("none", "None", Vec::new()).is_empty());
    }
}
