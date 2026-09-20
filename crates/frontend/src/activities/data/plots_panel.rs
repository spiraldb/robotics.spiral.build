//! Numeric fields of the open episode, drawn as stacked lanes on one clock.
//!
//! Each plotted stream loads its selected fields once and draws one lane per
//! field, one coloured trace per array index. Lanes share the visible span
//! with the Timeline; the playhead and the hover cursor cross every lane.
use super::{
    episode_streams::{EpisodeStreams, RecordedStream, StreamCatalog, StreamChoices, StreamKind},
    episode_transport::EpisodeTransport,
    message_buffer::{MessageBuffer, MessageField},
};
use crate::components::charts::{CharmingChart, ChartPresentation};
use crate::components::icons::SignalStreamIcon;
use crate::components::time_series::{
    TimelineSignalLane, TimelineSignalLanes, TimelineSignalPoint, TraceChartLayout,
};
use crate::format::GroupedCountPresentation;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::MessageWindow;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant};
use std::rc::Rc;

const LANE_HEIGHT: f64 = 96.0;
const LANE_GAP: f64 = 12.0;
const PLOT_MARGIN_LEFT: f64 = 56.0;
const PLOT_MARGIN_RIGHT: f64 = 10.0;
const QUICK_PLOT_STREAMS: usize = 8;

#[component]
pub(super) fn PlotsPanel(
    streams: Option<EpisodeStreams>,
    transport: EpisodeTransport,
    choices: StreamChoices,
) -> Element {
    let hover = use_signal(|| None::<f64>);
    let Some(streams) = streams else {
        return rsx! {
            div { class: "data-plots-empty",
                strong { "No episode open" }
                p { "Choose an episode under Datasets. Its numeric streams plot here on the shared timeline." }
            }
        };
    };
    let catalog = match streams {
        EpisodeStreams::Loading => {
            return rsx! { div { class: "data-plots-empty", role: "status", span { "Finding recorded streams…" } } };
        }
        EpisodeStreams::Failed(error) => {
            return rsx! { div { class: "data-plots-empty", role: "alert", strong { "Streams unavailable" } p { "{error}" } } };
        }
        EpisodeStreams::Ready(catalog) => catalog,
    };
    let plotted = choices.plotted_streams();
    if plotted.is_empty() {
        return rsx! { PlotsEmpty { catalog, choices } };
    }
    let view = transport.view();
    let playhead = view.percent(transport.seconds()) / 100.0;
    rsx! {
        section { class: "data-plots", "aria-label": "Plots", style: "--playhead: {playhead:.6}",
            div { class: "data-plots-ruler", "aria-hidden": "true",
                for tick in view.ticks() {
                    div { key: "{tick.seconds()}", class: "data-plots-tick", style: "--x: {tick.position() / 100.0:.6}",
                        "data-edge": if tick.position() > 90.0 { "end" } else { "start" },
                        span { "{tick.label()}" }
                    }
                }
            }
            div { class: "data-plots-scroll",
                for key in plotted {
                    if let Some(stream) = catalog.get(&key) {
                        StreamPlot {
                            key: "{key}",
                            stream: stream.clone(),
                            fields: choices.plotted_fields(&key),
                            transport: transport.clone(),
                            choices,
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
    candidates.sort_by_key(|stream| std::cmp::Reverse(stream.samples().unwrap_or(0)));
    candidates.truncate(QUICK_PLOT_STREAMS);
    rsx! {
        div { class: "data-plots-empty",
            strong { "Nothing plotted" }
            p { "Tick a stream in the Timeline to draw its numeric fields." }
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
                                {stream.samples().map(GroupedCountPresentation::grouped_label).unwrap_or_default()}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Whether a stream's traces cover the whole episode or only the visible span.
#[derive(Clone, Copy, PartialEq, Debug)]
enum LoadSpan {
    Episode,
    Visible,
}

#[derive(PartialEq)]
struct PlottedStream {
    buffer: Rc<MessageBuffer>,
    fields: Vec<MessageField>,
    lanes: Vec<TimelineSignalLane>,
}

#[component]
fn StreamPlot(
    stream: RecordedStream,
    fields: Vec<MessageField>,
    transport: EpisodeTransport,
    mut choices: StreamChoices,
    hover: Signal<Option<f64>>,
) -> Element {
    let mut load_span = use_signal(|| LoadSpan::Episode);
    let range = transport.range();
    let read = stream.read().clone();
    let load_fields = fields.clone();
    let view_source = transport.clone();
    let mut data = use_resource(move || {
        let fields = load_fields.clone();
        let read = read.clone();
        let span = load_span();
        let bounds = match span {
            LoadSpan::Episode => range.bounds(),
            LoadSpan::Visible => {
                let view = view_source.view();
                (
                    range.ns_at(view.start()),
                    range.ns_at(view.end()).saturating_add(1),
                )
            }
        };
        async move {
            if fields.is_empty() {
                return Ok::<_, String>(None);
            }
            if span == LoadSpan::Visible {
                gloo_timers::future::TimeoutFuture::new(250).await;
            }
            let projections = fields.iter().map(MessageField::projection).collect();
            let buffer = MessageBuffer::load(
                read.with_window(MessageWindow::new(bounds, projections)),
                true,
            )
            .await?;
            let lanes = buffer.lanes(&fields, range.start_ns())?;
            Ok(Some(Rc::new(PlottedStream {
                buffer,
                fields,
                lanes,
            })))
        }
    });
    // A whole episode that exceeds the read bounds falls back to the visible span.
    use_effect(move || {
        let exceeded = matches!(&*data.read(), Some(Err(error)) if error.contains("exceeds"));
        if exceeded && *load_span.peek() == LoadSpan::Episode {
            load_span.set(LoadSpan::Visible);
        }
    });
    let pending = *data.state().read() == UseResourceState::Pending;
    let hide_stream = stream.clone();
    let skeleton_height = fields.len() as f64 * (LANE_HEIGHT + LANE_GAP);
    rsx! {
        section { class: "data-plot", "aria-label": "{stream.label()} plots", "aria-busy": pending,
            header { class: "data-plot-head",
                SignalStreamIcon {}
                strong { class: "truncate", title: stream.name(), "{stream.label()}" }
                span { class: "data-plot-meta",
                    match &*data.read() {
                        Some(Ok(Some(plotted))) => rsx! {
                            "{(plotted.buffer.rows() as u64).grouped_label()} messages"
                            if load_span() == LoadSpan::Visible { " · visible span only" }
                        },
                        _ => rsx! {},
                    }
                    if pending { span { class: "data-plot-loading", role: "status", "Loading…" } }
                }
                Button { purpose: ActionPurpose::Remove, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                    title: "Stop plotting this stream", onclick: move |_| choices.set_stream_plotted(&hide_stream, false), "Hide" }
            }
            match &*data.read() {
                Some(Ok(Some(plotted))) => rsx! {
                    PlotLanes { plotted: plotted.clone(), stream_key: stream.key().to_owned(), transport: transport.clone(), choices, hover }
                },
                Some(Ok(None)) => rsx! {},
                Some(Err(error)) => rsx! {
                    div { class: "data-plot-error", role: "alert",
                        p { "{error}" }
                        Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, size: ButtonSize::Compact, onclick: move |_| data.restart(), "Retry" }
                    }
                },
                None => rsx! { div { class: "data-plot-skeleton", style: "height: {skeleton_height}px", "aria-hidden": "true" } },
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
    /// The part of a trace name that its lane title does not already say.
    fn trace_short_name(&self, lane: &str) -> String;
}

impl TraceNamePresentation for str {
    fn trace_short_name(&self, lane: &str) -> String {
        let stem = lane.trim_end_matches("[]");
        match self.strip_prefix(stem) {
            Some(rest) if !rest.is_empty() => rest.to_owned(),
            _ => self.to_owned(),
        }
    }
}

#[component]
fn PlotLanes(
    plotted: Rc<PlottedStream>,
    stream_key: String,
    transport: EpisodeTransport,
    mut choices: StreamChoices,
    mut hover: Signal<Option<f64>>,
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
    let at = hover().unwrap_or(seconds);
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
    let hover_x = hover().map(|moment| view.percent(moment) / 100.0);
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
                    oninput: move |event| if let Ok(value) = event.value().parse::<f64>() { seek.seek(value); },
                    onmousemove: move |event| {
                        let Some(element) = surface() else { return };
                        let width = f64::from(element.client_width());
                        if width <= 0.0 {
                            return;
                        }
                        let x = event.data().element_coordinates().x;
                        hover.set(Some(view.seconds_at(x / width)));
                    },
                    onmouseleave: move |_| hover.set(None),
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
        assert_eq!("temperature".trace_short_name("temperature"), "temperature");
        assert_eq!("other".trace_short_name("position[]"), "other");
    }
}
