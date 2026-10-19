//! Numeric fields of the open episode, drawn as stacked lanes on one clock.
//!
//! Each plotted stream loads its selected fields once and draws one lane per
//! field, one coloured trace per array index. Lanes share the visible span
//! with the Timeline, whose ruler is the one that reads time; the playhead
//! and the hover cursor cross every lane.
use super::{
    choices::StreamChoices,
    inspection::LoadedEpisode,
    messages::{MessageBuffer, MessageField},
    reads::StreamRead,
    streams::{RecordedStream, StreamCatalog, StreamKind},
    transport::EpisodeTransport,
};
use crate::components::charts::{CharmingChart, ChartPresentation};
use crate::components::icons::SignalStreamIcon;
use crate::components::time_series::{
    TimelineSignalLane, TimelineSignalLanes, TimelineSignalPoint, TraceChartLayout,
};
use crate::format::GroupedCountPresentation;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{MessageRead, MessageWindow};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant};
use std::rc::Rc;

const LANE_HEIGHT: f64 = 96.0;
const LANE_GAP: f64 = 12.0;
/// Room for a lane's value labels, which are small and few: the rest of the
/// width is the plot's. `--data-plot-left` in the stylesheet says the same.
const PLOT_MARGIN_LEFT: f64 = 40.0;
/// None: a plot ends where the timeline's tracks end, so the two share one
/// time axis edge to edge.
const PLOT_MARGIN_RIGHT: f64 = 0.0;
const QUICK_PLOT_STREAMS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PlotCursor {
    inspected_seconds: Option<f64>,
}

impl PlotCursor {
    fn inspect(&mut self, seconds: f64) {
        self.inspected_seconds = Some(seconds);
    }

    fn finish_inspecting(&mut self) {
        self.inspected_seconds = None;
    }

    const fn inspected_seconds(self) -> Option<f64> {
        self.inspected_seconds
    }
}

#[component]
pub(super) fn PlotsPanel(loaded: LoadedEpisode) -> Element {
    let hover = use_signal(PlotCursor::default);
    let catalog = loaded.catalog().clone();
    let transport = loaded.transport().clone();
    let choices = loaded.choices();
    let plotted = choices.plotted_streams();
    if plotted.is_empty() {
        return rsx! { PlotsEmpty { catalog, choices } };
    }
    let playhead = transport.view().percent(transport.seconds()) / 100.0;
    rsx! {
        section { class: "data-plots", "aria-label": "Plots", style: "--playhead: {playhead:.6}",
            div { class: "data-plots-scroll",
                for key in plotted {
                    if let Some(stream) = catalog.get(&key) {
                        StreamPlot {
                            key: "{key}",
                            stream: stream.clone(),
                            fields: choices.plotted_fields(&key),
                            loaded: loaded.clone(),
                            hover,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlotsEmpty(catalog: StreamCatalog, mut choices: StreamChoices) -> Element {
    let mut candidates = catalog
        .streams()
        .iter()
        .filter(|stream| stream.kind() == StreamKind::Data && !choices.stream_is_quiet(stream))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|stream| std::cmp::Reverse(stream.samples()));
    candidates.truncate(QUICK_PLOT_STREAMS);
    rsx! {
        div { class: "data-plots-empty",
            strong { "Nothing plotted" }
            if !candidates.is_empty() {
                ul { class: "data-plots-quick", "aria-label": "Plot a stream",
                    for stream in candidates {
                        li { key: "{stream.key()}",
                            Button { purpose: ActionPurpose::Select, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                                onclick: { let stream = stream.clone(); move |_| choices.set_stream_plotted(&stream, true) },
                                SignalStreamIcon {}
                                span { "{stream.label()}" }
                            }
                            span { class: "data-plots-quick-count",
                                {stream.samples().grouped_label()}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A stream's chosen fields read over a span, laid out as one lane per field.
pub(super) struct PlottedStream {
    buffer: Rc<MessageBuffer>,
    fields: Vec<MessageField>,
    lanes: Vec<TimelineSignalLane>,
}

impl PlottedStream {
    /// `fields` of `read` logged within `bounds`, on a clock that starts at
    /// `origin`.
    pub(super) async fn read(
        read: MessageRead,
        fields: Vec<MessageField>,
        bounds: (u64, u64),
        origin: u64,
    ) -> Result<Self, String> {
        let projections = fields.iter().map(MessageField::projection).collect();
        let buffer =
            MessageBuffer::load(read.with_window(MessageWindow::new(bounds, projections))).await?;
        let lanes = buffer.lanes(&fields, origin)?;
        Ok(Self {
            buffer,
            fields,
            lanes,
        })
    }
    pub(super) fn lanes(&self) -> &[TimelineSignalLane] {
        &self.lanes
    }
}

/// Values are read once and handed around; a view compares the read it was
/// handed by identity rather than by every sample in it.
impl PartialEq for PlottedStream {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

#[component]
fn StreamPlot(
    stream: RecordedStream,
    fields: Vec<MessageField>,
    loaded: LoadedEpisode,
    hover: Signal<PlotCursor>,
) -> Element {
    let transport = loaded.transport().clone();
    let mut choices = loaded.choices();
    let mut reads = loaded.reads();
    use_effect(use_reactive((&stream, &fields), move |(stream, fields)| {
        reads.want_values(&stream, fields)
    }));
    let whole = reads.values(&stream);
    // An episode too long to read whole is plotted for the span in view, read
    // again a moment after the view settles.
    let too_long = whole
        .as_ref()
        .and_then(StreamRead::failure)
        .is_some_and(|error| error.contains("exceeds"));
    let range = transport.range();
    let view_source = transport.clone();
    let read = stream.read().clone();
    let mut in_view = use_resource(use_reactive(
        (&too_long, &fields),
        move |(too_long, fields)| {
            let view = too_long.then(|| view_source.view());
            let read = read.clone();
            async move {
                let Some(view) = view else {
                    return Ok(None);
                };
                gloo_timers::future::TimeoutFuture::new(250).await;
                let bounds = (
                    range.ns_at(view.start()),
                    range.ns_at(view.end()).saturating_add(1),
                );
                PlottedStream::read(read, fields, bounds, range.start_ns())
                    .await
                    .map(|plotted| Some(Rc::new(plotted)))
            }
        },
    ));
    let shown = if too_long {
        let pending = *in_view.state().read() == UseResourceState::Pending;
        match &*in_view.read() {
            Some(Ok(plotted)) if pending => Some(StreamRead::Reading(plotted.clone())),
            Some(Ok(Some(plotted))) => Some(StreamRead::Ready(plotted.clone())),
            Some(Ok(None)) | None => Some(StreamRead::Reading(None)),
            Some(Err(error)) => Some(StreamRead::Failed(error.clone())),
        }
    } else {
        whole
    };
    let pending = shown.as_ref().is_none_or(StreamRead::reading);
    let hide_stream = stream.clone();
    let retry_stream = stream.clone();
    let retry_fields = fields.clone();
    let skeleton_height = fields.len() as f64 * (LANE_HEIGHT + LANE_GAP);
    rsx! {
        section { class: "data-plot", "aria-label": "{stream.label()} plots", "aria-busy": pending,
            header { class: "data-plot-head",
                SignalStreamIcon {}
                strong { class: "truncate", title: stream.name(), "{stream.label()}" }
                // The head says only what is out of the ordinary.
                span { class: "data-plot-meta",
                    if too_long && shown.as_ref().and_then(StreamRead::latest).is_some() {
                        "Visible span only"
                    }
                    if pending { span { class: "data-plot-loading", role: "status", "Loading…" } }
                }
                Button { purpose: ActionPurpose::Remove, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                    title: "Stop plotting this stream", onclick: move |_| choices.set_stream_plotted(&hide_stream, false), "Hide" }
            }
            match shown {
                Some(StreamRead::Failed(error)) => rsx! {
                    div { class: "data-plot-error", role: "alert",
                        p { "{error}" }
                        Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                            onclick: move |_| if too_long { in_view.restart() } else { reads.read_values(&retry_stream, retry_fields.clone()) },
                            "Retry" }
                    }
                },
                shown => match shown.as_ref().and_then(StreamRead::latest) {
                    Some(plotted) => rsx! {
                        PlotLanes { plotted: plotted.clone(), stream_key: stream.key().to_owned(), transport: transport.clone(), choices, hover }
                    },
                    None => rsx! { div { class: "data-plot-skeleton", style: "height: {skeleton_height}px", "aria-hidden": "true" } },
                },
            }
        }
    }
}

/// How many decimals a lane's readouts show: enough to tell apart values a
/// thousandth of the lane's range apart, never fewer than two nor more than
/// six. The exact stored value stays in the tooltip.
trait ReadoutPrecision {
    fn readout_decimals(&self) -> usize;
}

impl ReadoutPrecision for TimelineSignalLane {
    fn readout_decimals(&self) -> usize {
        let (low, high) = self
            .series()
            .iter()
            .flat_map(|series| {
                series
                    .points()
                    .iter()
                    .filter_map(TimelineSignalPoint::value)
            })
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
                (low.min(value), high.max(value))
            });
        if !low.is_finite() || !high.is_finite() {
            return 2;
        }
        let span = (high - low).max(high.abs().max(low.abs()) * 1e-6);
        if span <= 0.0 {
            return 2;
        }
        (3.0_f64 - span.log10()).ceil().clamp(2.0, 6.0) as usize
    }
}

/// A stored value at reading precision: floating-point text rounds to the
/// lane's decimals, while integers, booleans and text stay as written.
trait CompactReadout {
    fn compact_readout(&self, decimals: usize) -> String;
}

impl CompactReadout for str {
    fn compact_readout(&self, decimals: usize) -> String {
        if !(self.contains('.') || self.contains('e') || self.contains('E')) {
            return self.to_owned();
        }
        let Ok(value) = self.parse::<f64>() else {
            return self.to_owned();
        };
        let rounded = format!("{value:.decimals$}");
        // A rounded "-0.000" is zero, not a negative reading.
        match rounded.strip_prefix('-') {
            Some(digits) if digits.chars().all(|c| c == '0' || c == '.') => digits.to_owned(),
            _ => rounded,
        }
    }
}

trait TraceNamePresentation {
    /// The part of a trace name that its lane title does not already say:
    /// an array trace's index, and nothing for a lane's one scalar trace.
    fn trace_short_name(&self, lane: &str) -> String;
}

impl TraceNamePresentation for str {
    fn trace_short_name(&self, lane: &str) -> String {
        let stem = lane.trim_end_matches("[]");
        match self.strip_prefix(stem) {
            Some(rest) => rest.to_owned(),
            None => self.to_owned(),
        }
    }
}

#[component]
fn PlotLanes(
    plotted: Rc<PlottedStream>,
    stream_key: String,
    transport: EpisodeTransport,
    mut choices: StreamChoices,
    mut hover: Signal<PlotCursor>,
) -> Element {
    let view = transport.view();
    let range = transport.range();
    let seconds = transport.seconds();
    // A lane without one value in this episode would be a lane of nothing;
    // it is named once in the footer instead.
    let (lanes, empty): (Vec<_>, Vec<_>) = plotted
        .lanes
        .iter()
        .cloned()
        .partition(TimelineSignalLane::has_values);
    let empty = empty
        .iter()
        .map(TimelineSignalLane::label)
        .collect::<Vec<_>>()
        .join(", ");
    let visible_lanes = lanes
        .iter()
        .map(|lane| {
            let series = lane
                .series()
                .iter()
                .filter(|series| !choices.trace_hidden(&stream_key, series.name()))
                .cloned()
                .collect();
            lane.clone().with_series(series)
        })
        .collect::<Vec<_>>();
    let layout = TraceChartLayout::labelled((view.start(), view.end()), LANE_HEIGHT)
        .with_gap(LANE_GAP)
        .with_margins(PLOT_MARGIN_LEFT, PLOT_MARGIN_RIGHT);
    let chart = use_memo(use_reactive(
        (&visible_lanes, &layout),
        |(lanes, layout)| lanes.as_slice().chart_with(&layout),
    ));
    let id = use_hook(|| format!("data-plot-{}", dioxus::dioxus_core::current_scope_id().0));
    let inspected_seconds = hover().inspected_seconds();
    let at = inspected_seconds.unwrap_or(seconds);
    // Before the first sample the readouts hold that sample, as the cameras
    // hold their first frame, instead of a column of dashes.
    let held_first = plotted
        .buffer
        .first_time()
        .filter(|first| range.ns_at(at) < *first);
    let at_ns = held_first.unwrap_or_else(|| range.ns_at(at));
    let values = plotted
        .buffer
        .values(&plotted.fields, at_ns)
        .unwrap_or_default();
    let height = lanes.len() as f64 * (LANE_HEIGHT + LANE_GAP);
    let hover_x = inspected_seconds.map(|moment| view.percent(moment) / 100.0);
    let mut seek = transport.clone();
    let mut surface = use_signal(|| None::<web_sys::Element>);
    rsx! {
        div { class: "data-plot-lanes", style: "height: {height}px",
            div { class: "data-plot-readouts",
                for lane in lanes.iter() {
                    {
                    let decimals = lane.readout_decimals();
                    rsx! {
                    div { key: "{lane.id()}", class: "data-plot-readout", style: "height: {LANE_HEIGHT + LANE_GAP}px",
                        div { class: "data-plot-readout-title", title: lane.label(), "{lane.label()}" }
                        div { class: "data-plot-traces",
                            for series in lane.series() {
                                {
                                    let hidden = choices.trace_hidden(&stream_key, series.name());
                                    let name = series.name().to_owned();
                                    let key = stream_key.clone();
                                    let color = series.color().unwrap_or(lane.color()).to_owned();
                                    let short = series.name().trace_short_name(lane.label());
                                    let exact = values.get(series.name()).cloned().unwrap_or_else(|| "—".into());
                                    let shown = exact.compact_readout(decimals);
                                    rsx! {
                                        button { key: "{name}", r#type: "button", class: "data-plot-trace", "aria-pressed": !hidden,
                                            title: if hidden { format!("Show {name}") } else { format!("Hide {name} · exact {exact}") },
                                            onclick: move |_| choices.toggle_trace(&key, &name),
                                            span { class: "data-plot-swatch", style: "background: {color}", "aria-hidden": "true" }
                                            span { class: "data-plot-trace-name", "{short}" }
                                            span { class: "data-plot-trace-value", title: "{exact}", "{shown}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    }
                    }
                }
            }
            div { class: "data-plot-canvas",
                CharmingChart { id, chart: chart(), presentation: ChartPresentation::new("data-plot-chart", "Selected fields over episode time") }
                input { r#type: "range", class: "data-plot-scrub", tabindex: "-1", "aria-hidden": "true",
                    min: "{view.start()}", max: "{view.end()}", step: "0.001", value: "{seconds}",
                    onmounted: move |event| {
                        if let Some(element) = event.data().downcast::<web_sys::Element>() {
                            surface.set(Some(element.clone()));
                        }
                    },
                    oninput: move |event| if let Ok(value) = event.value().parse::<f64>() {
                        seek.seek(value);
                        // The native range maps its thumb to a slightly inset
                        // track. Retire the pointer preview after seeking so a
                        // click cannot leave that earlier time beside the
                        // committed playhead.
                        hover.write().finish_inspecting();
                    },
                    onpointerup: move |_| hover.write().finish_inspecting(),
                    onmousemove: move |event| {
                        let Some(element) = surface() else { return };
                        let width = f64::from(element.client_width());
                        if width <= 0.0 {
                            return;
                        }
                        let x = event.data().element_coordinates().x;
                        hover.write().inspect(view.seconds_at(x / width));
                    },
                    onmouseleave: move |_| hover.write().finish_inspecting(),
                }
                div { class: "data-plot-playhead", "aria-hidden": "true" }
                if let Some(x) = hover_x {
                    div { class: "data-plot-hover", style: "--x: {x:.6}", "aria-hidden": "true" }
                }
            }
        }
        if !empty.is_empty() {
            div { class: "data-plot-empty-note", title: "These fields have no value in this episode", "no values: {empty}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::time_series::TimelineSignalSeries;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn readouts_round_to_the_lane_range_and_keep_integers_whole() {
        let lane = |values: &[f64]| {
            let points = values
                .iter()
                .enumerate()
                .map(|(index, value)| TimelineSignalPoint::new(index as f64, Some(*value)))
                .collect();
            TimelineSignalLane::new("lane", "lane", "#2a78d6")
                .with_series(vec![TimelineSignalSeries::new("lane", points)])
        };
        assert_eq!(lane(&[-1.0, 1.0]).readout_decimals(), 3);
        assert_eq!(lane(&[0.10, 0.12]).readout_decimals(), 5);
        assert_eq!(lane(&[0.0, 500.0]).readout_decimals(), 2);
        assert_eq!(lane(&[0.0, 0.000_000_1]).readout_decimals(), 6);
        assert_eq!(lane(&[]).readout_decimals(), 2);
        assert_eq!("-0.23022049286644".compact_readout(3), "-0.230");
        assert_eq!("-0.0001".compact_readout(3), "0.000");
        assert_eq!(
            "18446744073709551615".compact_readout(3),
            "18446744073709551615"
        );
        assert_eq!("true".compact_readout(3), "true");
        assert_eq!("—".compact_readout(3), "—");
        let empty =
            TimelineSignalLane::new("empty", "acceleration[]", "#2a78d6").with_series(vec![
                TimelineSignalSeries::new(
                    "acceleration[0]",
                    vec![TimelineSignalPoint::new(0.0, None)],
                ),
            ]);
        assert!(!empty.has_values());
        assert!(lane(&[1.0]).has_values());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn trace_names_drop_what_their_lane_already_says() {
        assert_eq!("position[3]".trace_short_name("position[]"), "[3]");
        assert_eq!("temperature".trace_short_name("temperature"), "");
        assert_eq!("other".trace_short_name("position[]"), "other");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn committing_a_seek_retires_the_hover_cursor() {
        let mut cursor = PlotCursor::default();
        cursor.inspect(11.0);
        assert_eq!(cursor.inspected_seconds(), Some(11.0));

        cursor.finish_inspecting();
        assert_eq!(cursor.inspected_seconds(), None);
    }
}
