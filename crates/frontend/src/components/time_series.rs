//! Schema-neutral time-series models and the shared trace renderer.
use crate::components::charts::{ChartPoints, StudioTooltip};
use charming::Chart;
use charming::component::{Axis, Grid};
use charming::datatype::{CompositeValue, DataFrame, DataPoint};
use charming::element::{
    AxisLabel, AxisLine, AxisTick, AxisType, Emphasis, LineStyle, Sampling, SplitLine, Symbol,
};
use charming::series::Line;
const TRACE_POINT_BUDGET: usize = 512;
const TRACE_LANE_HEIGHT: f64 = 28.0;
/// One measured point. `None` creates a visible gap rather than bridging a
/// missing sample with a line the hardware never reported.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TimelineSignalPoint {
    seconds: f64,
    value: Option<f64>,
}

impl TimelineSignalPoint {
    pub(crate) fn value(&self) -> Option<f64> {
        self.value
    }

    pub(crate) const fn new(seconds: f64, value: Option<f64>) -> Self {
        Self { seconds, value }
    }

    pub(crate) const fn seconds(&self) -> f64 {
        self.seconds
    }
}

/// One named series within a lane. A series without its own colour inherits
/// the lane's; a coloured series stands on its own within the lane.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TimelineSignalSeries {
    name: String,
    points: Vec<TimelineSignalPoint>,
    color: Option<String>,
}

impl TimelineSignalSeries {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn new(name: impl Into<String>, points: Vec<TimelineSignalPoint>) -> Self {
        Self {
            name: name.into(),
            points,
            color: None,
        }
    }

    pub(crate) fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    pub(crate) fn color(&self) -> Option<&str> {
        self.color.as_deref()
    }

    pub(crate) fn points(&self) -> &[TimelineSignalPoint] {
        &self.points
    }
}

/// A named lane supplied by any message or recording source.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TimelineSignalLane {
    id: String,
    label: String,
    color: String,
    series: Vec<TimelineSignalSeries>,
}

impl TimelineSignalLane {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn label(&self) -> &str {
        &self.label
    }
    pub(crate) fn color(&self) -> &str {
        &self.color
    }

    pub(crate) fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        color: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            color: color.into(),
            series: Vec::new(),
        }
    }

    pub(crate) fn with_series(mut self, series: Vec<TimelineSignalSeries>) -> Self {
        self.series = series;
        self
    }

    pub(crate) fn series(&self) -> &[TimelineSignalSeries] {
        &self.series
    }

    /// Whether any series of the lane carries a single value.
    pub(crate) fn has_values(&self) -> bool {
        self.series
            .iter()
            .any(|series| series.points.iter().any(|point| point.value.is_some()))
    }
}

/// How lanes sit on the canvas: the visible span, lane geometry, and whether
/// each lane carries a readable value axis.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TraceChartLayout {
    window: (f64, f64),
    lane_height: f64,
    lane_gap: f64,
    margins: (f64, f64),
    labelled: bool,
    tooltip: bool,
}

impl TraceChartLayout {
    /// Live's compact stack: hairline lanes from the episode start, no axes.
    pub(crate) fn sparkline(duration: f64) -> Self {
        Self {
            window: (0.0, duration.max(0.001)),
            lane_height: TRACE_LANE_HEIGHT,
            lane_gap: 0.0,
            margins: (0.0, 0.0),
            labelled: false,
            tooltip: true,
        }
    }

    /// Inspection lanes: tall, gapped, with value labels and a light grid.
    pub(crate) fn labelled(window: (f64, f64), lane_height: f64) -> Self {
        Self {
            window: (window.0, window.1.max(window.0 + 0.001)),
            lane_height,
            lane_gap: 0.0,
            margins: (0.0, 0.0),
            labelled: true,
            tooltip: false,
        }
    }

    pub(crate) fn with_window(mut self, window: (f64, f64)) -> Self {
        self.window = window;
        self
    }

    pub(crate) fn with_gap(mut self, gap: f64) -> Self {
        self.lane_gap = gap.max(0.0);
        self
    }

    pub(crate) fn with_margins(mut self, left: f64, right: f64) -> Self {
        self.margins = (left.max(0.0), right.max(0.0));
        self
    }

    /// Vertical pitch of one lane, including its gap.
    pub(crate) fn lane_pitch(&self) -> f64 {
        self.lane_height + self.lane_gap
    }
}

pub(crate) trait TimelineSignalLanes {
    fn chart_with(&self, layout: &TraceChartLayout) -> Chart;
}

impl TimelineSignalLanes for [TimelineSignalLane] {
    fn chart_with(&self, layout: &TraceChartLayout) -> Chart {
        let mut chart = Chart::new().animation(false);
        if layout.tooltip {
            chart = chart.tooltip(StudioTooltip::numeric(3));
        }

        for (lane_index, lane) in self.iter().enumerate() {
            let grid_index = lane_index as f64;
            let y_axis = if layout.labelled {
                TimelineChartStyle::labelled_value_axis(grid_index)
            } else {
                TimelineChartStyle::hidden_axis(AxisType::Value, grid_index).scale(true)
            };
            chart = chart
                .grid(
                    Grid::new()
                        .id(format!("timeline-grid-{lane_index}"))
                        .left(layout.margins.0)
                        .right(layout.margins.1)
                        .top(lane_index as f64 * layout.lane_pitch() + layout.lane_gap / 2.0)
                        .height(layout.lane_height),
                )
                .x_axis(
                    TimelineChartStyle::hidden_axis(AxisType::Value, grid_index)
                        .min(layout.window.0)
                        .max(layout.window.1),
                )
                .y_axis(y_axis);

            for (series_index, series) in lane.series.iter().enumerate() {
                let mark_samples = series.has_isolated_samples();
                let line_style = match &series.color {
                    Some(color) => LineStyle::new()
                        .color(color.clone())
                        .width(1.4)
                        .opacity(0.95),
                    None => LineStyle::new()
                        .color(lane.color.clone())
                        .width(if series_index == 0 { 1.5 } else { 1.2 })
                        .opacity(if series_index == 0 { 0.95 } else { 0.52 }),
                };
                chart = chart.series(
                    Line::new()
                        .id(format!("{}-{}-{series_index}", lane.id, series.name))
                        .name(format!("{} · {}", lane.label, series.name))
                        .x_axis_index(grid_index)
                        .y_axis_index(grid_index)
                        .show_symbol(mark_samples)
                        .symbol(if mark_samples {
                            Symbol::Circle
                        } else {
                            Symbol::None
                        })
                        .symbol_size(3.0)
                        .sampling(Sampling::Minmax)
                        .connect_nulls(false)
                        .line_style(line_style)
                        .emphasis(Emphasis::new().disabled(true))
                        .data(series.chart_data(TRACE_POINT_BUDGET)),
                );
            }
        }

        chart
    }
}

struct TimelineChartStyle;

impl TimelineChartStyle {
    fn hidden_axis(kind: AxisType, grid_index: f64) -> Axis {
        Axis::new()
            .type_(kind)
            .grid_index(grid_index)
            .show(false)
            .axis_line(AxisLine::new().show(false))
            .axis_tick(AxisTick::new().show(false))
            .axis_label(AxisLabel::new().show(false))
            .split_line(SplitLine::new().show(false))
    }

    /// A quiet value axis: small faint labels and hairline guides, no spine.
    fn labelled_value_axis(grid_index: f64) -> Axis {
        Axis::new()
            .type_(AxisType::Value)
            .grid_index(grid_index)
            .scale(true)
            .split_number(3.0)
            .axis_line(AxisLine::new().show(false))
            .axis_tick(AxisTick::new().show(false))
            .axis_label(AxisLabel::new().show(true).font_size(9.0).color("#71706c"))
            .split_line(
                SplitLine::new()
                    .show(true)
                    .line_style(LineStyle::new().color("#e8e8e6").width(1.0)),
            )
    }

    fn point(x: f64, y: Option<f64>) -> DataPoint {
        DataPoint::from(vec![CompositeValue::from(x), CompositeValue::from(y)])
    }
}

/// Convert gaps to ECharts null points and retain extrema before crossing the
/// Wasm boundary. Pathological intermittent signals keep their longest valid
/// runs so the payload remains bounded instead of freezing the operator UI.
impl TimelineSignalSeries {
    fn chart_data(&self, budget: usize) -> DataFrame {
        let mut runs = Vec::<Vec<(f64, f64)>>::new();
        let mut current = Vec::<(f64, f64)>::new();

        for point in &self.points {
            match point.value.filter(|value| value.is_finite()) {
                Some(value) if point.seconds.is_finite() => current.push((point.seconds, value)),
                _ if !current.is_empty() => runs.push(std::mem::take(&mut current)),
                _ => {}
            }
        }
        if !current.is_empty() {
            runs.push(current);
        }
        if runs.is_empty() {
            return Vec::new();
        }

        let max_runs = (budget / 4).max(1);
        if runs.len() > max_runs {
            let mut selected = (0..runs.len()).collect::<Vec<_>>();
            selected.sort_by_key(|index| std::cmp::Reverse(runs[*index].len()));
            selected.truncate(max_runs);
            selected.sort_unstable();
            runs = selected
                .into_iter()
                .map(|index| std::mem::take(&mut runs[index]))
                .collect();
        }

        let total_valid = runs.iter().map(Vec::len).sum::<usize>().max(1);
        let point_budget = budget.saturating_sub(runs.len().saturating_sub(1));
        let mut data = DataFrame::new();
        let mut previous_end = None;

        for run in runs {
            let share =
                ((run.len() as f64 / total_valid as f64) * point_budget as f64).round() as usize;
            let share = share.max(4).min(run.len());
            let reduced = run.downsample_extrema(share);
            if let (Some(end), Some((start, _))) = (previous_end, reduced.first()) {
                data.push(TimelineChartStyle::point((end + start) / 2.0, None));
            }
            previous_end = reduced.last().map(|(seconds, _)| *seconds);
            data.extend(
                reduced
                    .into_iter()
                    .map(|(seconds, value)| TimelineChartStyle::point(seconds, Some(value))),
            );
        }

        data
    }
}

impl TimelineSignalSeries {
    fn has_isolated_samples(&self) -> bool {
        let valid = |index: usize| {
            self.points.get(index).is_some_and(|point| {
                point.seconds.is_finite() && point.value.is_some_and(f64::is_finite)
            })
        };
        (0..self.points.len())
            .any(|index| valid(index) && (index == 0 || !valid(index - 1)) && !valid(index + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn sparse_numeric_observations_keep_markers_and_native_scaling() {
        let series = TimelineSignalSeries::new(
            "sensor",
            vec![
                TimelineSignalPoint::new(0.0, Some(0.000001)),
                TimelineSignalPoint::new(1.0, None),
                TimelineSignalPoint::new(2.0, Some(0.000002)),
            ],
        );
        assert!(series.has_isolated_samples());
        let lane = TimelineSignalLane::new("sensor", "sensor", "blue").with_series(vec![series]);
        let chart = serde_json::to_value(
            [lane]
                .as_slice()
                .chart_with(&TraceChartLayout::sparkline(2.0)),
        )
        .unwrap();
        assert_eq!(chart["series"][0]["symbol"], "circle");
        assert_eq!(chart["series"][0]["showSymbol"], true);
        let axis = chart["yAxis"].get(0).unwrap_or(&chart["yAxis"]);
        assert_eq!(axis["scale"], true);
        assert!(axis.get("min").is_none());
        assert!(
            chart["series"][0]["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|point| point[1].is_null())
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn labelled_layouts_window_the_axis_and_colour_each_series() {
        let series = |name: &str, color: &str| {
            TimelineSignalSeries::new(
                name,
                vec![
                    TimelineSignalPoint::new(0.0, Some(1.0)),
                    TimelineSignalPoint::new(1.0, Some(2.0)),
                ],
            )
            .with_color(color)
        };
        let lane =
            TimelineSignalLane::new("position[]", "position[]", "#000000").with_series(vec![
                series("position[0]", "#111111"),
                series("position[1]", "#222222"),
            ]);
        let layout = TraceChartLayout::labelled((0.5, 0.75), 96.0)
            .with_gap(10.0)
            .with_margins(46.0, 10.0);
        assert_eq!(layout.lane_pitch(), 106.0);
        let chart = serde_json::to_value([lane].as_slice().chart_with(&layout)).unwrap();
        // Charming flattens single-element component lists into one object.
        let first = |value: &serde_json::Value| value.get(0).unwrap_or(value).clone();
        assert_eq!(first(&chart["xAxis"])["min"], 0.5);
        assert_eq!(first(&chart["xAxis"])["max"], 0.75);
        assert_eq!(first(&chart["yAxis"])["axisLabel"]["show"], true);
        assert_eq!(first(&chart["grid"])["left"], 46.0);
        assert_eq!(first(&chart["grid"])["top"], 5.0);
        assert_eq!(chart["series"][1]["lineStyle"]["color"], "#222222");
        assert!(chart.get("tooltip").is_none());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn traces_break_at_missing_samples_and_preserve_a_peak() {
        let mut points = (0..600)
            .map(|index| TimelineSignalPoint {
                seconds: index as f64 / 20.0,
                value: Some(0.0),
            })
            .collect::<Vec<_>>();
        points[250].value = Some(90.0);
        points[400].value = None;

        let data = TimelineSignalSeries::new("test", points).chart_data(TRACE_POINT_BUDGET);
        let data = serde_json::to_value(data).expect("trace data should serialize");
        let rows = data.as_array().expect("trace data should be an array");

        assert!(rows.iter().any(|row| row[1].is_null()));
        assert!(rows.iter().any(|row| row[1] == 90.0));
        assert!(rows.len() <= TRACE_POINT_BUDGET + 4);
    }
}
