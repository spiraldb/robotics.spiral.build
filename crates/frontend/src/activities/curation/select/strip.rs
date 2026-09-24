//! Where the threshold falls among the values a Select judges: every item as
//! one dot on one axis, kept in ink and left out faded, and the threshold as a
//! line across it. The gap between the values beside the line says how firm
//! the choice is, which the count alone cannot. The rule's slider moves along
//! the same axis, so the line follows it.
use super::SelectView;
use crate::activities::curation::document::Selection;
use crate::activities::curation::population::Population;
use crate::activities::curation::state::CurationController;
use dioxus::prelude::*;

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

/// The axis a numeric rule's threshold moves along, shared by the slider in
/// the rule's sentence and the strip beneath it, so the two always agree: 0%
/// to 100% for a probability, else the span of the judged values and the
/// threshold, widened to whole steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ThresholdScale {
    low: f64,
    high: f64,
    step: SliderStep,
    percent: bool,
}

impl ThresholdScale {
    /// The scale for `threshold` over the values `dots` judge. With nothing
    /// judged yet the axis spans one unit either side of the threshold, so the
    /// slider still moves.
    pub(super) fn of(dots: &[StripDot], threshold: f64, percent: bool) -> Self {
        if percent {
            return Self {
                low: 0.0,
                high: 1.0,
                step: SliderStep::PERCENT,
                percent,
            };
        }
        let (low, high) = dots
            .iter()
            .map(|dot| dot.value)
            .chain(std::iter::once(threshold))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
                (low.min(value), high.max(value))
            });
        let (low, high) = if high - low > f64::EPSILON {
            (low, high)
        } else {
            (threshold - 1.0, threshold + 1.0)
        };
        let step = SliderStep::of((low, high));
        let (low, high) = step.bounds((low, high));
        Self {
            low,
            high,
            step,
            percent,
        }
    }
    pub(super) fn low(self) -> f64 {
        self.low
    }
    pub(super) fn high(self) -> f64 {
        self.high
    }
    pub(super) fn step(self) -> f64 {
        self.step.0
    }
    /// A slid value on the nearest step.
    pub(super) fn snap(self, value: f64) -> f64 {
        self.step.snap(value)
    }
    /// A value as the rule and the strip print it: a whole percentage for a
    /// probability, otherwise with the step's own decimals.
    pub(super) fn label(self, value: f64) -> String {
        if self.percent {
            format!("{:.0}%", value * 100.0)
        } else {
            format!("{value:.*}", self.step.decimals())
        }
    }
    /// Where `value` falls across the strip, 0–100.
    fn place(self, value: f64) -> f64 {
        let spread = (self.high - self.low).max(f64::EPSILON);
        ((value - self.low) / spread).clamp(0.0, 1.0) * 100.0
    }
}

/// How finely a threshold slides: about a hundredth of the axis, at a power
/// of ten, so a slid threshold reads "0.89" rather than
/// "0.8900000000000001".
#[derive(Clone, Copy, Debug, PartialEq)]
struct SliderStep(f64);

impl SliderStep {
    /// A probability moves by whole percents.
    const PERCENT: Self = Self(0.01);

    fn of((low, high): (f64, f64)) -> Self {
        let spread = (high - low).max(f64::EPSILON);
        Self(10_f64.powf((spread / 100.0).log10().floor()))
    }
    fn decimals(self) -> usize {
        (-self.0.log10()).max(0.0).round() as usize
    }
    /// The axis widened to whole steps, so every place on the slider is a
    /// value the step can land on.
    fn bounds(self, (low, high): (f64, f64)) -> (f64, f64) {
        (
            self.snap((low / self.0).floor() * self.0),
            self.snap((high / self.0).ceil() * self.0),
        )
    }
    fn snap(self, value: f64) -> f64 {
        let snapped = (value / self.0).round() * self.0;
        // Rounding in binary leaves a tail; the step's own decimals are all a
        // threshold is worth.
        let scale = 10_f64.powi(self.decimals() as i32);
        (snapped * scale).round() / scale
    }
}

/// The strip itself: every judged value as a dot, and the threshold as a
/// line where the rule's slider stands. A dot is a pointer shortcut to its
/// item; keyboard users reach the same item in the index below, so no dot is
/// a tab stop.
#[component]
pub(super) fn ThresholdStrip(
    dots: Vec<StripDot>,
    scale: ThresholdScale,
    threshold: f64,
    chosen: Option<usize>,
    on_select: EventHandler<usize>,
) -> Element {
    rsx! {
        // A pointer shortcut, as the map is: the rule above and the lists
        // below say in words what it draws.
        div { class: "curation-select-strip", "aria-hidden": "true",
            span {
                class: "curation-select-strip-threshold",
                style: "left: {scale.place(threshold):.2}%",
            }
            for dot in dots {
                button {
                    key: "{dot.item}",
                    r#type: "button",
                    class: "curation-select-strip-dot",
                    tabindex: "-1",
                    "data-kept": "{dot.kept}",
                    "aria-current": (chosen == Some(dot.item)).then_some("true"),
                    style: "left: {scale.place(dot.value):.2}%; --lift: {(dot.item * 7) % 5}",
                    title: "{scale.label(dot.value)}",
                    onclick: move |_| on_select.call(dot.item),
                }
            }
            span { class: "curation-select-strip-axis",
                span { "{scale.label(scale.low)}" }
                span { "{scale.label(scale.high)}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SliderStep, ThresholdScale};

    #[test]
    fn a_slid_threshold_snaps_to_its_step_and_keeps_its_decimals() {
        let step = SliderStep::of((0.08, 1.39));
        assert_eq!(step.0, 0.01);
        assert_eq!(step.snap(0.890_000_000_1), 0.89);
        assert_eq!(step.bounds((0.0834, 1.3911)), (0.08, 1.4));
        let seconds = SliderStep::of((0.0, 317.0));
        assert_eq!(seconds.0, 1.0);
        assert_eq!(seconds.snap(20.4), 20.0);
    }

    #[test]
    fn a_scale_prints_values_in_its_own_steps() {
        let progress = ThresholdScale::of(&[], 0.89, false);
        assert_eq!(progress.label(0.8912), "0.89");
        let similarity = ThresholdScale::of(&[], 0.5, true);
        assert_eq!(
            (similarity.low(), similarity.high(), similarity.step()),
            (0.0, 1.0, 0.01)
        );
        assert_eq!(similarity.label(0.734), "73%");
    }
}
