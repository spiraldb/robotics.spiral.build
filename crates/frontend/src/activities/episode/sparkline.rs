//! A plotted stream's values along its timeline row: the shape of every
//! shown trace on the episode's clock, so when the robot moved reads beside
//! what its cameras saw. Each field is scaled to its own range, as its plot
//! lane is. The traces are drawn in one faint ink, the way an editor draws
//! a soundtrack under its pictures: where they bunch and swing is what the
//! row says, and telling trace from trace is the plot's job.
use super::plots::PlottedStream;
use crate::components::time_series::{TimelineSignalLane, TimelineSignalSeries};
use crate::components::timeline::ViewWindow;
use dioxus::prelude::*;
use std::{fmt::Write as _, rc::Rc};

/// Samples a trace keeps before it is thinned: a few per pixel of a wide row.
const TRACE_BUDGET: usize = 1_600;
/// The share of the row's height a trace leaves clear above and below.
const INSET: f64 = 10.0;

trait SparklineTraces {
    /// The traces of this lane not named in `hidden`, scaled together, as
    /// path data in seconds across and hundredths of the row's height down.
    fn sparkline_traces(&self, hidden: &[String]) -> Vec<String>;
}

impl SparklineTraces for TimelineSignalLane {
    fn sparkline_traces(&self, hidden: &[String]) -> Vec<String> {
        let shown = self
            .series()
            .iter()
            .filter(|series| !hidden.iter().any(|name| name == series.name()))
            .collect::<Vec<_>>();
        let (low, high) = shown
            .iter()
            .flat_map(|series| series.points().iter().filter_map(|point| point.value()))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
                (low.min(value), high.max(value))
            });
        if !low.is_finite() {
            return Vec::new();
        }
        // A flat lane draws along the middle of the row.
        let y = |value: f64| {
            if high > low {
                100.0 - INSET - (value - low) / (high - low) * (100.0 - 2.0 * INSET)
            } else {
                50.0
            }
        };
        shown
            .into_iter()
            .map(|series| series.sparkline_path(y))
            .collect()
    }
}

trait SparklinePath {
    /// The trace as path data, broken where a sample has no value. A trace
    /// over budget keeps each stretch's lowest and highest sample, so a
    /// spike survives thinning.
    fn sparkline_path(&self, y: impl Fn(f64) -> f64) -> String;
}

impl SparklinePath for TimelineSignalSeries {
    fn sparkline_path(&self, y: impl Fn(f64) -> f64) -> String {
        let points = self.points();
        let stretch = points.len().div_ceil(TRACE_BUDGET / 2).max(1);
        let mut path = String::new();
        let mut connected = false;
        for chunk in points.chunks(stretch) {
            let valued = chunk
                .iter()
                .filter_map(|point| point.value().map(|value| (point.seconds(), value)))
                .collect::<Vec<_>>();
            let (Some(lowest), Some(highest)) = (
                valued.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1)),
                valued.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1)),
            ) else {
                connected = false;
                continue;
            };
            let kept = if stretch == 1 || lowest.0 == highest.0 {
                vec![lowest]
            } else if lowest.0 < highest.0 {
                vec![lowest, highest]
            } else {
                vec![highest, lowest]
            };
            for (seconds, value) in kept {
                let command = if connected { 'L' } else { 'M' };
                let _ = write!(path, "{command}{seconds:.4},{:.2} ", y(value));
                connected = true;
            }
        }
        path
    }
}

/// The traces of `plotted` along a row showing `view`: every lane, or only
/// the one named `lane`, without the traces named in `hidden`.
#[component]
pub(super) fn ValuesSparkline(
    plotted: Rc<PlottedStream>,
    lane: Option<String>,
    hidden: Vec<String>,
    view: ViewWindow,
) -> Element {
    let traces = use_memo(use_reactive(
        (&plotted, &lane, &hidden),
        |(plotted, lane, hidden)| {
            plotted
                .lanes()
                .iter()
                .filter(|candidate| lane.as_deref().is_none_or(|lane| candidate.id() == lane))
                .flat_map(|lane| lane.sparkline_traces(&hidden))
                .collect::<Vec<_>>()
        },
    ));
    let span = view.end() - view.start();
    rsx! {
        svg { class: "data-sparkline", view_box: "{view.start()} 0 {span} 100", preserve_aspect_ratio: "none", "aria-hidden": "true",
            for (index, trace) in traces.read().iter().enumerate() {
                path { key: "{index}", d: "{trace}", vector_effect: "non-scaling-stroke" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::time_series::TimelineSignalPoint;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_lane_scales_its_shown_traces_together_and_breaks_at_missing_values() {
        let series = |name: &str, values: &[Option<f64>]| {
            TimelineSignalSeries::new(
                name,
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| TimelineSignalPoint::new(index as f64, *value))
                    .collect(),
            )
        };
        let lane = TimelineSignalLane::new("state[]", "state[]", "#2a78d6").with_series(vec![
            series("state[0]", &[Some(0.0), None, Some(1.0)]),
            series("state[1]", &[Some(100.0), Some(100.0), Some(100.0)]),
        ]);
        let traces = lane.sparkline_traces(&["state[1]".to_owned()]);
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0], "M0.0000,90.00 M2.0000,10.00 ");
        let flat = lane.sparkline_traces(&["state[0]".to_owned()]);
        assert_eq!(flat[0], "M0.0000,50.00 L1.0000,50.00 L2.0000,50.00 ");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_long_trace_keeps_each_stretch_extremes_in_time_order() {
        let points = (0..TRACE_BUDGET * 2)
            .map(|index| {
                let spike = if index == 7 { 50.0 } else { 0.0 };
                TimelineSignalPoint::new(index as f64, Some(spike))
            })
            .collect();
        let path = TimelineSignalSeries::new("spiky", points).sparkline_path(|value| value);
        // The first stretch keeps its first and last sample; the second, its
        // floor before its spike.
        assert!(path.starts_with("M0.0000,0.00 L3.0000,0.00 L4.0000,0.00 L7.0000,50.00 "));
        assert_eq!(path.matches(' ').count(), TRACE_BUDGET);
    }
}
