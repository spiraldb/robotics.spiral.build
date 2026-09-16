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
use crate::components::interaction::{Action, ActionPurpose, ActionSize, ActionVariant};
use crate::components::time_series::{TimelineSignalLane, TimelineSignalLanes, TraceChartLayout};
use crate::format::{GroupedCountPresentation, PreciseClockPresentation};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::MessageWindow;
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
        .filter(|stream| stream.kind() == StreamKind::Data && stream.samples() != Some(0))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|stream| std::cmp::Reverse(stream.samples().unwrap_or(0)));
    candidates.truncate(QUICK_PLOT_STREAMS);
    rsx! {
        div { class: "data-plots-empty",
            strong { "Nothing plotted yet" }
            p { "Tick a stream in the Timeline to draw all of its numeric fields, or expand it and pick fields one by one." }
            if !candidates.is_empty() {
                ul { class: "data-plots-quick", "aria-label": "Plot a stream",
                    for stream in candidates {
                        li { key: "{stream.key()}",
                            Action { purpose: ActionPurpose::Select, variant: ActionVariant::Outline, size: ActionSize::Compact,
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
                Action { purpose: ActionPurpose::Remove, variant: ActionVariant::Link, size: ActionSize::Compact,
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
                        Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Outline, size: ActionSize::Compact, onclick: move |_| data.restart(), "Retry" }
                    }
                },
                None => rsx! { div { class: "data-plot-skeleton", style: "height: {skeleton_height}px", "aria-hidden": "true" } },
            }
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
    let visible_lanes = plotted
        .lanes
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
    let at_ns = range.ns_at(at);
    let values = plotted
        .buffer
        .values(&plotted.fields, at_ns)
        .unwrap_or_default();
    let sample = plotted.buffer.sample_time(at_ns).ok().flatten();
    let height = plotted.lanes.len() as f64 * (LANE_HEIGHT + LANE_GAP);
    let hover_x = hover().map(|moment| view.percent(moment) / 100.0);
    let mut seek = transport.clone();
    let mut surface = use_signal(|| None::<web_sys::Element>);
    rsx! {
        div { class: "data-plot-lanes", style: "height: {height}px",
            div { class: "data-plot-readouts",
                for lane in plotted.lanes.iter() {
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
                                    let value = values.get(series.name()).cloned().unwrap_or_else(|| "—".into());
                                    rsx! {
                                        button { key: "{name}", r#type: "button", class: "data-plot-trace", "aria-pressed": !hidden,
                                            title: if hidden { format!("Show {name}") } else { format!("Hide {name}") },
                                            onclick: move |_| choices.toggle_trace(&key, &name),
                                            span { class: "data-plot-swatch", style: "background: {color}", "aria-hidden": "true" }
                                            span { class: "data-plot-trace-name", "{short}" }
                                            span { class: "data-plot-trace-value", title: "{value}", "{value}" }
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
        div { class: "data-plot-foot",
            match sample {
                Some(sample) => rsx! {
                    span { "sample {range.seconds_of(sample).precise_clock_label()}" }
                    span { "age {(at - range.seconds_of(sample)).max(0.0):.3} s" }
                    if hover().is_some() { span { class: "data-plot-foot-note", "at cursor" } }
                },
                None => rsx! { span { "No message at this time" } },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn trace_names_drop_what_their_lane_already_says() {
        assert_eq!("position[3]".trace_short_name("position[]"), "[3]");
        assert_eq!("temperature".trace_short_name("temperature"), "temperature");
        assert_eq!("other".trace_short_name("position[]"), "other");
    }
}
