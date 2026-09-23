//! Where the threshold falls among the values a Select judges: every item as
//! one dot on one axis, kept in ink and left out faded, and the threshold as a
//! line across it. The gap between the values beside the line says how firm
//! the choice is, which the count alone cannot.
use super::SelectView;
use crate::activities::curation::document::Selection;
use crate::activities::curation::population::Population;
use crate::activities::curation::state::CurationController;
use dioxus::prelude::*;
use std::fmt::{Display, Formatter, Result as FormatResult};

/// One item's value on the strip.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StripDot {
    /// The item's place in the Select's input.
    item: usize,
    value: f64,
    kept: bool,
}

impl StripDot {
    /// The values a numeric rule judges, one per item that has one: an
    /// episode's recorded fact, a subtask's similarity, a moment's progress.
    /// A rule of categories has no axis.
    pub(super) fn for_view(
        view: &SelectView,
        controller: CurationController,
        kept: &[bool],
    ) -> Option<Vec<Self>> {
        let values: Vec<Option<f64>> = match (&view.selection, &view.input) {
            (Selection::Episodes { fact, .. }, Population::Episodes(episodes)) => episodes
                .iter()
                .map(|id| view.sample.episode(id).and_then(|episode| fact.of(episode)))
                .collect(),
            (Selection::Similar { .. }, Population::Subtasks { rows, .. }) => controller
                .with_payload(|payload| {
                    let ranking = payload.current_ranking(view.measure.id());
                    rows.iter()
                        .map(|row| ranking.and_then(|ranking| ranking.probability(row)))
                        .collect()
                }),
            (Selection::Progressing { .. }, Population::Moments(rows)) => {
                rows.iter().map(|row| row.velocity()).collect()
            }
            _ => return None,
        };
        Some(
            values
                .into_iter()
                .enumerate()
                .filter_map(|(item, value)| {
                    Some(Self {
                        item,
                        value: value.filter(|value| value.is_finite())?,
                        kept: kept.get(item).copied().unwrap_or(false),
                    })
                })
                .collect(),
        )
    }
}

/// A value as the strip prints it: a percentage for a similarity, otherwise
/// as many digits as its size earns.
struct StripValue(f64, bool);

impl Display for StripValue {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FormatResult {
        let Self(value, percent) = *self;
        if percent {
            return write!(formatter, "{:.0}%", value * 100.0);
        }
        match value.abs() {
            magnitude if magnitude >= 100.0 => write!(formatter, "{value:.0}"),
            magnitude if magnitude >= 10.0 => write!(formatter, "{value:.1}"),
            _ => write!(formatter, "{value:.2}"),
        }
    }
}

/// The strip itself. A dot is a pointer shortcut to its item; keyboard users
/// reach the same item in the index below, so no dot is a tab stop.
#[component]
pub(super) fn ThresholdStrip(
    dots: Vec<StripDot>,
    threshold: f64,
    /// Whether values are probabilities, drawn from 0% to 100%.
    percent: bool,
    chosen: Option<usize>,
    on_select: EventHandler<usize>,
) -> Element {
    let (low, high) = if percent {
        (0.0, 1.0)
    } else {
        dots.iter()
            .map(|dot| dot.value)
            .chain(std::iter::once(threshold))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
                (low.min(value), high.max(value))
            })
    };
    let spread = (high - low).max(f64::EPSILON);
    let place = move |value: f64| ((value - low) / spread).clamp(0.0, 1.0) * 100.0;
    rsx! {
        div {
            class: "curation-select-strip",
            role: "group",
            "aria-label": "Every value the rule judges, and where it divides them",
            span {
                class: "curation-select-strip-threshold",
                style: "left: {place(threshold):.2}%",
                title: "Keeps {StripValue(threshold, percent)}",
            }
            for dot in dots {
                button {
                    key: "{dot.item}",
                    r#type: "button",
                    class: "curation-select-strip-dot",
                    tabindex: "-1",
                    "data-kept": "{dot.kept}",
                    "aria-current": (chosen == Some(dot.item)).then_some("true"),
                    style: "left: {place(dot.value):.2}%; --lift: {(dot.item * 7) % 5}",
                    title: "{StripValue(dot.value, percent)}",
                    "aria-label": "{StripValue(dot.value, percent)}",
                    onclick: move |_| on_select.call(dot.item),
                }
            }
            span { class: "curation-select-strip-axis", "aria-hidden": "true",
                span { "{StripValue(low, percent)}" }
                span { "{StripValue(high, percent)}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StripValue;

    #[test]
    fn values_print_with_the_digits_their_size_earns() {
        assert_eq!(StripValue(0.734, true).to_string(), "73%");
        assert_eq!(StripValue(0.0421, false).to_string(), "0.04");
        assert_eq!(StripValue(23.45, false).to_string(), "23.4");
        assert_eq!(StripValue(4812.0, false).to_string(), "4812");
    }
}
