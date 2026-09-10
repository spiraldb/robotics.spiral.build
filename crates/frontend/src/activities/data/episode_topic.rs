//! One topic panel chooses its presentation from the recorded schema.
use super::{
    episode_playback::EpisodeCamera,
    episode_transport::{EpisodeTimeRange, EpisodeTransport},
    message_buffer::{MessageBuffer, MessageField},
};
use crate::{
    activities::data::query_result::{QueryExecutionPresentation, QueryState},
    components::{
        charts::{CharmingChart, ChartPresentation},
        time_series::{TimelineSignalLane, TimelineSignalLanes},
    },
};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetTopic, MessageRead, MessageWindow};
use std::rc::Rc;

#[derive(Clone, PartialEq)]
pub(super) struct TopicSource {
    request: MessageRead,
    initial_mode: TopicMode,
}
impl TopicSource {
    pub(super) fn new(request: MessageRead, topic: &DatasetTopic) -> Self {
        Self {
            request,
            initial_mode: TopicMode::from(topic),
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum TopicMode {
    Video,
    Fields,
    Messages,
}
impl From<&DatasetTopic> for TopicMode {
    fn from(topic: &DatasetTopic) -> Self {
        if topic
            .schemas()
            .iter()
            .any(|schema| schema == "foxglove.CompressedVideo")
        {
            Self::Video
        } else {
            Self::Fields
        }
    }
}

#[component]
pub(super) fn EpisodeTopic(
    source: TopicSource,
    timeline: EpisodeTimeRange,
    transport: EpisodeTransport,
) -> Element {
    let mut mode = use_signal(|| source.initial_mode);
    let mut fields_visited = use_signal(|| false);
    let panel = dioxus_workbench::PanelContext::current();
    let activity = crate::shell::activity::WorkspaceActivity::current();
    let visible = panel.geometry().visible() && activity.is_active();
    use_effect(move || {
        if panel.geometry().visible() && activity.is_active() && mode() == TopicMode::Fields {
            fields_visited.set(true);
        }
    });
    rsx! { div { class: "episode-fields-panel",
        div { class: "episode-field-toolbar",
            if source.initial_mode == TopicMode::Video { button { class: "data-text-action", "aria-pressed": mode() == TopicMode::Video, onclick: move |_| mode.set(TopicMode::Video), "Video" } }
            button { class: "data-text-action", "aria-pressed": mode() == TopicMode::Fields, onclick: move |_| mode.set(TopicMode::Fields), "Fields & plots" }
            button { class: "data-text-action", "aria-pressed": mode() == TopicMode::Messages, onclick: move |_| mode.set(TopicMode::Messages), "Messages" }
        }
        if visible && mode() == TopicMode::Video { EpisodeCamera { request: source.request.clone(), timeline, transport: transport.clone() } }
        if visible && mode() == TopicMode::Messages { MessageTable { source: source.request.clone(), timeline } }
        if fields_visited() {
            div { class: "episode-fields-mode", hidden: mode() != TopicMode::Fields,
                SchemaFields { source: source.request, timeline, transport }
            }
        }
    } }
}

#[component]
fn SchemaFields(
    source: MessageRead,
    timeline: EpisodeTimeRange,
    transport: EpisodeTransport,
) -> Element {
    let schema_source = source.clone();
    let mut schema = use_resource(move || MessageBuffer::load(schema_source.clone(), false));
    rsx! {
        if *schema.state().read() == UseResourceState::Pending { p { class: "data-empty-copy", role: "status", "Reading message schema…" } }
        else { match &*schema.read() {
            Some(Ok(schema)) => match schema.value_field() {
                Ok(root) => rsx! { FieldPlots { source: FieldPlotSource { request: source, timeline, root }, transport } },
                Err(error) => rsx! { p { class: "data-empty-copy", role: "alert", "{error}" } },
            },
            Some(Err(error)) => rsx! { div { class: "data-empty-copy", role: "alert", p { "{error}" } button { class: "data-text-action", onclick: move |_| schema.restart(), "Retry schema" } } },
            None => rsx! {},
        } }
    }
}

#[derive(Clone, PartialEq)]
struct FieldPlotSource {
    request: MessageRead,
    timeline: EpisodeTimeRange,
    root: MessageField,
}
#[component]
fn FieldPlots(source: FieldPlotSource, transport: EpisodeTransport) -> Element {
    let timeline = source.timeline;
    let root = source.root.clone();
    let mut selected = use_signal(Vec::<MessageField>::new);
    let mut bounds = use_signal(|| (0.0, timeline.duration()));
    let mut schema_open = use_signal(|| true);
    let read = source;
    let mut data = use_resource(move || {
        let selected = selected();
        let bounds = bounds();
        let read = read.clone();
        async move {
            if selected.is_empty() {
                return Ok::<_, String>(None);
            }
            let fields = selected.iter().map(MessageField::projection).collect();
            let start = read
                .timeline
                .start_ns()
                .saturating_add((bounds.0 * 1e9) as u64);
            let end = read
                .timeline
                .start_ns()
                .saturating_add((bounds.1 * 1e9) as u64)
                .saturating_add(1);
            let data = MessageBuffer::load(
                read.request
                    .with_window(MessageWindow::new((start, end), fields)),
                true,
            )
            .await?;
            let lanes = data.lanes(&selected, read.timeline.start_ns())?;
            Ok(Some(Rc::new(PlottedMessages {
                data,
                fields: selected,
                lanes,
            })))
        }
    });
    rsx! {
        div { class: "episode-field-plots-layout",
            div { class: "episode-field-toolbar episode-window-controls",
                button { class: "data-text-action", "aria-expanded": schema_open(), onclick: move |_| schema_open.toggle(), "Schema" }
                label { "From" input { r#type: "number", min: "0", max: "{timeline.duration()}", step: "0.001", value: "{bounds().0:.3}", "aria-label": "Plot window start seconds", onchange: move |event| if let Ok(value) = event.value().parse::<f64>() && value.is_finite() && value >= 0.0 && value < bounds().1 { bounds.write().0 = value; } } }
                label { "To" input { r#type: "number", min: "0", max: "{timeline.duration()}", step: "0.001", value: "{bounds().1:.3}", "aria-label": "Plot window end seconds", onchange: move |event| if let Ok(value) = event.value().parse::<f64>() && value.is_finite() && value > bounds().0 && value <= timeline.duration() { bounds.write().1 = value; } } }
                span { "seconds" }
            }
        div { class: "episode-field-workspace",
            aside { class: "episode-field-browser", hidden: !schema_open(), "aria-label": "Message schema",
                p { class: "episode-field-help", "Select numeric fields to plot. Arrays retain their recorded indices. ? marks nullable fields." }
                SchemaField { field: root, selected: selected(), on_select: move |field: MessageField| {
                    let mut fields = selected.write();
                    if let Some(index) = fields.iter().position(|existing| existing == &field) { fields.remove(index); }
                    else if fields.len() < 16 { fields.push(field); }
                } }
            }
            div { class: "episode-field-plots",
                if *data.state().read() == UseResourceState::Pending { p { class: "data-empty-copy", role: "status", "Loading selected fields…" } }
                else { match &*data.read() {
                    Some(Ok(Some(plot))) => rsx! { MessageTraces { plot: plot.clone(), time: TraceTime { timeline, transport, window: bounds() } } },
                    Some(Ok(None)) => rsx! { p { class: "data-empty-copy", "Select a numeric field from the schema." } },
                    Some(Err(error)) => rsx! { div { class: "data-empty-copy", role: "alert", p { "{error}" } button { class: "data-text-action", onclick: move |_| data.restart(), "Retry fields" } } },
                    None => rsx! {},
                } }
            }
        }
        }
    }
}
#[component]
fn SchemaField(
    field: MessageField,
    selected: Vec<MessageField>,
    on_select: EventHandler<MessageField>,
) -> Element {
    let children = field.children();
    if !children.is_empty() {
        rsx! { details { class: "episode-schema-branch", open: true,
            summary { span { "{field.name()}" } small { "{field.type_label()}" } }
            div { for child in children { SchemaField { key: "{child.label()}", field: child, selected: selected.clone(), on_select } } }
        } }
    } else {
        let checked = selected.contains(&field);
        rsx! { div { class: "episode-schema-field",
            if field.plottable() { label { title: field.label(), input { r#type: "checkbox", checked, disabled: !checked && selected.len() >= 16, "aria-label": "Plot {field.label()}", onchange: { let field = field.clone(); move |_| on_select.call(field.clone()) } } span { "{field.name()}" } } }
            else { span { title: field.label(), "{field.name()}" } }
            small { "{field.type_label()}" }
        } }
    }
}
#[derive(PartialEq)]
struct PlottedMessages {
    data: Rc<MessageBuffer>,
    fields: Vec<MessageField>,
    lanes: Vec<TimelineSignalLane>,
}
#[derive(Clone, PartialEq)]
struct TraceTime {
    timeline: EpisodeTimeRange,
    transport: EpisodeTransport,
    window: (f64, f64),
}
#[component]
fn MessageTraces(plot: Rc<PlottedMessages>, time: TraceTime) -> Element {
    let TraceTime {
        timeline,
        transport,
        window,
    } = time;
    let chart = use_memo(use_reactive((&plot,), move |(plot,)| {
        plot.lanes.as_slice().chart(timeline.duration())
    }));
    let id = use_hook(|| {
        format!(
            "message-traces-{}",
            dioxus::dioxus_core::current_scope_id().0
        )
    });
    let seconds = transport.seconds();
    let absolute = timeline.start_ns().saturating_add((seconds * 1e9) as u64);
    let in_window = seconds >= window.0 && seconds <= window.1;
    let current = if in_window {
        plot.data.values(&plot.fields, absolute).and_then(|values| {
            plot.data
                .sample_time(absolute)
                .map(|sample| (values, sample))
        })
    } else {
        Ok((Default::default(), None))
    };
    let (values, sample) = match current {
        Ok(current) => current,
        Err(error) => return rsx! { p { class: "data-empty-copy", role: "alert", "{error}" } },
    };
    let percent = if timeline.duration() > 0.0 {
        100.0 * seconds / timeline.duration()
    } else {
        0.0
    };
    let height = plot.lanes.len() * 28;
    rsx! {
        if plot.data.rows() == 0 { p { class: "data-empty-copy", "No messages in this window." } }
        div { class: "episode-trace-scroll",
            div { class: "episode-trace-lanes", style: "height: {height}px",
                div { class: "episode-trace-labels", for lane in &plot.lanes { div { title: lane.label(), span { class: "episode-trace-swatch", style: "background-color: {lane.color()}", "aria-hidden": "true" } "{lane.label()}" } } }
                div { class: "episode-trace-chart",
                    CharmingChart { id, chart: chart(), presentation: ChartPresentation::new("episode-trace-chart-canvas", "Selected message fields over episode time") }
                    div { class: "episode-trace-playhead", style: "left: {percent}%", "aria-hidden": "true" }
                }
                div { class: "episode-trace-values", "aria-label": "Field values at playhead", for lane in &plot.lanes { div { title: values.get(lane.label()).cloned().unwrap_or_default(), {values.get(lane.label()).cloned().unwrap_or_else(|| "—".into())} } } }
            }
        }
        div { class: "episode-trace-axis", span { "0 s" } span { "{timeline.duration():.3} s" } }
        div { class: "episode-trace-sample-time",
            if let Some(sample) = sample { {format!("{} messages · sample {:.6} s · age {:.3} s", plot.data.rows(), sample.saturating_sub(timeline.start_ns()) as f64 / 1e9, absolute.saturating_sub(sample) as f64 / 1e9)} }
            else { "No message at this time in the selected window" }
        }
    }
}
#[component]
pub(super) fn MessageTable(source: MessageRead, timeline: EpisodeTimeRange) -> Element {
    let mut data = use_resource(move || {
        let source = source
            .clone()
            .with_window(MessageWindow::new(timeline.bounds(), vec![]).with_limit(100));
        async move { MessageBuffer::load(source, false).await?.snapshot() }
    });
    rsx! {
        if *data.state().read() == UseResourceState::Pending { p { class: "data-empty-copy", "Loading message preview…" } }
        else { match &*data.read() {
            Some(Ok(snapshot)) => rsx! { {QueryExecutionPresentation::new("Native message window".into(), 100, QueryState::Ready(snapshot.clone())).render()} },
            Some(Err(error)) => rsx! { div { class: "data-empty-copy", role: "alert", p { "{error}" } button { class: "data-text-action", onclick: move |_| data.restart(), "Retry messages" } } },
            None => rsx! {},
        } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn topic_rendering_uses_schema_instead_of_topic_name() {
        let video = DatasetTopic::new(
            "/unexpected/name".into(),
            vec!["foxglove.CompressedVideo".into()],
        );
        let metadata = DatasetTopic::new("/camera/metadata".into(), vec!["example.Sensor".into()]);
        assert!(TopicMode::from(&video) == TopicMode::Video);
        assert!(TopicMode::from(&metadata) == TopicMode::Fields);
        assert!(
            TopicMode::from(&DatasetTopic::new("/unknown".into(), vec![])) == TopicMode::Fields
        );
    }
}
