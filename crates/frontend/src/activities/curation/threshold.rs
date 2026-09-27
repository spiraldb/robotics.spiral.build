//! Where a rule's threshold falls among the values it judges: every item as
//! one dot on one axis, kept in ink, left out faded, and — on one of several
//! conditions — passing this one but left out by another as a ring, and the
//! threshold as a line across it with its slider's thumb at the foot of the
//! line. The gap
//! between the values beside the line says how firm the choice is, which the
//! count alone cannot, and dragging the line re-divides kept from left out.
//!
//! A Select's episode conditions, Score progress's progress rate, and Find
//! similar's similarity all slide on this one strip.
use crate::format::GroupedCountPresentation as _;
use dioxus::prelude::*;

/// The most dots one strip draws. Moments come one per frame, sixty thousand
/// over a sample; past a few thousand the strip reads the same with every nth
/// value, and the page stays responsive.
const MAX_STRIP_DOTS: usize = 2_400;

/// Which side of a rule one item falls on, as its dot shows it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DotSide {
    Kept,
    /// Left out by the threshold this strip slides.
    LeftOut,
    /// Passing this strip's threshold, but left out by another condition of
    /// the same rule.
    LeftOutElsewhere,
}

impl DotSide {
    const fn key(self) -> &'static str {
        match self {
            Self::Kept => "kept",
            Self::LeftOut => "left-out",
            Self::LeftOutElsewhere => "elsewhere",
        }
    }
}

/// One item's value on the strip.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StripDot {
    /// The item's place in what the rule judges.
    item: usize,
    value: f64,
    side: DotSide,
}

impl StripDot {
    /// One dot per item that has a value, in item order, every nth of them
    /// past `MAX_STRIP_DOTS`; `kept` says, by item, which side of the rule
    /// each is on.
    pub(super) fn of(values: &[Option<f64>], kept: &[bool]) -> Vec<Self> {
        let sides = kept
            .iter()
            .map(|kept| {
                if *kept {
                    DotSide::Kept
                } else {
                    DotSide::LeftOut
                }
            })
            .collect::<Vec<_>>();
        Self::on_sides(values, &sides)
    }
    /// The same, with each item's side said in full.
    pub(super) fn on_sides(values: &[Option<f64>], sides: &[DotSide]) -> Vec<Self> {
        let every = values.len().div_ceil(MAX_STRIP_DOTS).max(1);
        values
            .iter()
            .enumerate()
            .step_by(every)
            .filter_map(|(item, value)| {
                Some(Self {
                    item,
                    value: value.filter(|value| value.is_finite())?,
                    side: sides.get(item).copied().unwrap_or(DotSide::LeftOut),
                })
            })
            .collect()
    }
}

/// What a threshold is counted in, which is how its axis prints it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AxisUnit {
    /// A probability, 0% to 100%.
    Percent,
    /// Progress rate against expert demonstrations, as "0.90×".
    ProgressRate,
    /// A recorded fact, whose unit the rule's sentence says after it: a
    /// duration, count, or size, all measured from zero.
    Plain,
}

/// Where a rule's threshold stands, the axis it slides along, and which side
/// of it the rule keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Threshold {
    value: f64,
    scale: ThresholdScale,
    /// Whether the rule keeps what is at least the threshold, rather than
    /// at most it.
    keeps_above: bool,
}

impl Threshold {
    /// The threshold at `value` on an axis over the values `dots` judge, in
    /// `unit`, keeping what is at least it or, when not `keeps_above`, at
    /// most it.
    pub(super) fn over(dots: &[StripDot], value: f64, unit: AxisUnit, keeps_above: bool) -> Self {
        Self {
            value,
            scale: ThresholdScale::of(dots, value, unit),
            keeps_above,
        }
    }
    /// The threshold as the rule's sentence and the strip print it.
    pub(super) fn label(self) -> String {
        self.scale.label(self.value)
    }
    /// The low and high ends of the axis it slides along.
    pub(super) fn ends(self) -> (f64, f64) {
        (self.scale.low, self.scale.high)
    }
}

/// The axis a rule's threshold moves along, shared by the strip and
/// the slider on it, so the two always agree: 0% to 100% for a probability,
/// else the span of the judged values and the threshold, widened to whole
/// steps.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ThresholdScale {
    low: f64,
    high: f64,
    step: SliderStep,
    unit: AxisUnit,
}

impl ThresholdScale {
    /// The scale for `threshold` over the values `dots` judge. A recorded
    /// fact's axis starts at zero, where a new condition keeps everything, so
    /// the first drag away from it never rescales the axis under the pointer.
    /// With nothing judged yet the axis spans one unit either side of the
    /// threshold, so the slider still moves.
    fn of(dots: &[StripDot], threshold: f64, unit: AxisUnit) -> Self {
        if unit == AxisUnit::Percent {
            return Self {
                low: 0.0,
                high: 1.0,
                step: SliderStep::PERCENT,
                unit,
            };
        }
        let origin = (unit == AxisUnit::Plain).then_some(0.0);
        let (low, high) = dots
            .iter()
            .map(|dot| dot.value)
            .chain(std::iter::once(threshold))
            .chain(origin)
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
            unit,
        }
    }
    /// A slid value on the nearest step.
    fn snap(self, value: f64) -> f64 {
        self.step.snap(value)
    }
    /// A value as the rule and the strip print it: a whole percentage for a
    /// probability, otherwise with the step's own decimals, and a whole count
    /// in thousands: "150,000" messages.
    fn label(self, value: f64) -> String {
        match self.unit {
            AxisUnit::Percent => format!("{:.0}%", value * 100.0),
            AxisUnit::ProgressRate => format!("{value:.*}×", self.step.decimals().max(2)),
            AxisUnit::Plain if self.step.decimals() == 0 && value >= 0.0 => {
                (value.round() as u64).grouped_label()
            }
            AxisUnit::Plain => format!("{value:.*}", self.step.decimals()),
        }
    }
    /// The expert rate, on a progress-rate axis that reaches it.
    fn reference(self) -> Option<f64> {
        (self.unit == AxisUnit::ProgressRate && (self.low..=self.high).contains(&1.0))
            .then_some(1.0)
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
/// line with the slider's thumb at its foot. The dots and the thumb share one
/// track, inset by half a thumb at either end as a range input insets its
/// own, so a value's dot and the thumb set to it stand at the same pixel. A
/// dot is a pointer shortcut to its item; keyboard users reach the same item
/// in the index below, so no dot is a tab stop, and the slider takes the
/// arrows.
#[component]
pub(super) fn ThresholdStrip(
    dots: Vec<StripDot>,
    threshold: Threshold,
    chosen: Option<usize>,
    on_select: EventHandler<usize>,
    /// The threshold slid to a new value, on the nearest step.
    on_threshold: EventHandler<f64>,
    /// What the slider sets, for a page with more than one.
    #[props(default = "Threshold".to_owned())]
    label: String,
) -> Element {
    let Threshold {
        value,
        scale,
        keeps_above,
    } = threshold;
    rsx! {
        div { class: "curation-threshold-strip",
            div { class: "curation-threshold-strip-track", "aria-hidden": "true",
                span {
                    class: "curation-threshold-strip-threshold",
                    style: "left: {scale.place(value):.2}%",
                }
                for dot in dots {
                    button {
                        key: "{dot.item}",
                        r#type: "button",
                        class: "curation-threshold-strip-dot",
                        tabindex: "-1",
                        "data-side": dot.side.key(),
                        "aria-current": (chosen == Some(dot.item)).then_some("true"),
                        style: "left: {scale.place(dot.value):.2}%; --lift: {(dot.item * 7) % 5}",
                        title: "{scale.label(dot.value)}",
                        onclick: move |_| on_select.call(dot.item),
                    }
                }
            }
            // The slider's track fills on the side the rule keeps, so the
            // bar reads as what is selected before its value is.
            input {
                class: "curation-threshold-strip-slider",
                r#type: "range",
                style: "--at: {scale.place(value) / 100.0:.4}",
                "data-keeps": if keeps_above { "above" } else { "below" },
                min: "{scale.low}",
                max: "{scale.high}",
                step: "{scale.step.0}",
                value: "{value}",
                "aria-label": "{label}",
                "aria-valuetext": "{scale.label(value)}",
                oninput: move |event: FormEvent| {
                    if let Ok(slid) = event.value().parse::<f64>() {
                        on_threshold.call(scale.snap(slid));
                    }
                },
            }
            // The axis's ends, the threshold's value under its handle, and
            // on a progress-rate axis where the expert rate falls.
            span { class: "curation-threshold-strip-axis", "aria-hidden": "true",
                span { "{scale.label(scale.low)}" }
                if let Some(reference) = scale.reference() {
                    span {
                        class: "curation-threshold-strip-reference",
                        style: "--at: {scale.place(reference) / 100.0:.4}",
                        "1×"
                    }
                }
                span {
                    class: "curation-threshold-strip-value",
                    style: "--at: {scale.place(value) / 100.0:.4}",
                    "{scale.label(value)}"
                }
                span { "{scale.label(scale.high)}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AxisUnit, SliderStep, StripDot, ThresholdScale};

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
        let progress = ThresholdScale::of(&[], 0.89, AxisUnit::ProgressRate);
        assert_eq!(progress.label(0.8912), "0.89×");
        // With nothing judged the axis spans one unit either side, in
        // hundredths; a recorded fact's reaches down to zero, in tenths, and
        // carries no unit of its own.
        let seconds = ThresholdScale::of(&[], 20.0, AxisUnit::Plain);
        assert_eq!(seconds.label(20.0), "20.0");
        let counts = StripDot::of(&[Some(2_000.0), Some(531_000.0)], &[true, true]);
        let messages = ThresholdScale::of(&counts, 150_000.0, AxisUnit::Plain);
        assert_eq!(messages.label(150_000.0), "150,000");
        let similarity = ThresholdScale::of(&[], 0.5, AxisUnit::Percent);
        assert_eq!(
            (similarity.low, similarity.high, similarity.step.0),
            (0.0, 1.0, 0.01)
        );
        assert_eq!(similarity.label(0.734), "73%");
    }

    #[test]
    fn a_recorded_fact_slides_on_one_axis_from_zero() {
        let durations = StripDot::of(&[Some(13.57), Some(60.53)], &[true, true]);
        let fresh = ThresholdScale::of(&durations, 0.0, AxisUnit::Plain);
        let slid = ThresholdScale::of(&durations, 45.0, AxisUnit::Plain);
        assert_eq!((fresh.low, fresh.high), (0.0, 60.6));
        assert_eq!((slid.low, slid.high), (fresh.low, fresh.high));
    }
}
