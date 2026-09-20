//! Data-specific stream selection composed into the shared timeline.
use super::{
    episode_list::OpenedEpisode,
    episode_streams::{
        CoverageState, EpisodeStreams, PlotState, PlottableLeaves, RecordedStream, StreamChoices,
        StreamKind, StreamSchema,
    },
    episode_transport::EpisodeTransport,
    message_buffer::MessageField,
};
use crate::components::{
    icons::{CameraStreamIcon, SignalStreamIcon},
    timeline::{DensityTrack, Timeline, TimelineContent, TimelineScrubber},
};
use crate::format::{GroupedCountPresentation, PreciseClockPresentation};
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, ChevronIcon, TextInput, TextInputKind,
};
use std::collections::BTreeSet;

/// What the Timeline is about: the open episode and its discovered streams.
#[derive(Clone, PartialEq)]
pub(super) struct TimelineSubject {
    episode: OpenedEpisode,
    streams: EpisodeStreams,
    choices: StreamChoices,
}

impl TimelineSubject {
    pub(super) fn new(
        episode: OpenedEpisode,
        streams: EpisodeStreams,
        choices: StreamChoices,
    ) -> Self {
        Self {
            episode,
            streams,
            choices,
        }
    }
}

#[component]
pub(super) fn TimelinePanel(
    transport: EpisodeTransport,
    subject: Option<TimelineSubject>,
) -> Element {
    let Some(subject) = subject else {
        return rsx! { Timeline { interaction: transport.interaction(), content: TimelineContent::new(rsx! {
            div { class: "data-timeline-empty", strong { "No episode open" } span { "Choose an episode under Datasets to see its streams on this clock." } }
        }) } };
    };
    rsx! { StreamTimeline { key: "{subject.episode.record().id()}", transport, subject } }
}

#[component]
fn StreamTimeline(transport: EpisodeTransport, subject: TimelineSubject) -> Element {
    let mut filter = use_signal(String::new);
    let expanded = use_signal(BTreeSet::<String>::new);
    let mut quiet_open = use_signal(|| false);
    let catalog = match &subject.streams {
        EpisodeStreams::Ready(catalog) => Some(catalog),
        _ => None,
    };
    let needle = filter().to_lowercase();
    let searching = !needle.is_empty();
    let rows = catalog
        .into_iter()
        .flat_map(|catalog| catalog.streams())
        .filter(|stream| !searching || stream.label().to_lowercase().contains(&needle))
        .cloned()
        .collect::<Vec<_>>();
    let total = catalog.map_or(0, |catalog| catalog.streams().len());
    // Cameras lead, then streams with something to plot; the rest wait behind
    // one row until asked for. A search shows every match in place.
    let choices = subject.choices;
    let (cameras, data): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|stream| stream.kind() == StreamKind::Video);
    let (quiet, loud): (Vec<_>, Vec<_>) = data
        .into_iter()
        .partition(|stream| !searching && choices.stream_is_quiet(stream));
    let leading = cameras.into_iter().chain(loud).collect::<Vec<_>>();
    let nothing_listed = leading.is_empty() && quiet.is_empty();
    let identity = rsx! {
        strong { "{subject.episode.listed().title()}" }
        span { class: "truncate", title: "{subject.episode.record().facts_label()}\n{subject.episode.record().source_uri().unwrap_or_default()}", "{subject.episode.label().head()}" }
    };
    let corner = rsx! { TextInput { class: "data-stream-filter", kind: TextInputKind::Search,
    placeholder: "Filter {total} streams", "aria-label": "Filter streams",
    value: filter(), oninput: move |value| filter.set(value) } };
    let rows = rsx! {
        match subject.streams {
            EpisodeStreams::Loading => rsx! { div { class: "data-timeline-none", role: "status", "Finding recorded streams…" } },
            EpisodeStreams::Failed(error) => rsx! { div { class: "data-timeline-none", role: "alert", "Could not read this episode's streams: {error}" } },
            EpisodeStreams::Ready(catalog) => rsx! {
                if nothing_listed { p { class: "data-timeline-none", if catalog.is_empty() { "This episode has no readable streams." } else { "No streams match the filter." } } }
                StreamRows { streams: leading, transport: transport.clone(), choices, expanded }
                if !quiet.is_empty() {
                    QuietStreamsRow { count: quiet.len(), open: quiet_open(), on_toggle: move |_| quiet_open.toggle() }
                    if quiet_open() { StreamRows { streams: quiet, transport: transport.clone(), choices, expanded } }
                }
            },
        }
    };
    rsx! { Timeline { interaction: transport.interaction(), content: TimelineContent::new(rows).with_identity(identity).with_corner(corner) } }
}

#[component]
fn StreamRows(
    streams: Vec<RecordedStream>,
    transport: EpisodeTransport,
    choices: StreamChoices,
    expanded: Signal<BTreeSet<String>>,
) -> Element {
    rsx! {
        for stream in streams {
            StreamRow { key: "{stream.key()}", stream: stream.clone(), transport: transport.clone(), choices,
                expanded: expanded.read().contains(stream.key()),
                on_expand: move |open: bool| {
                    let mut expanded = expanded;
                    if open { expanded.write().insert(stream.key().to_owned()); let mut choices = choices; choices.ensure_schema(&stream); }
                    else { expanded.write().remove(stream.key()); }
                },
            }
        }
    }
}

/// One row standing in for the streams with nothing to plot, so the hub
/// leads with cameras and measurements. They stay one click away.
#[component]
fn QuietStreamsRow(count: usize, open: bool, on_toggle: EventHandler<()>) -> Element {
    let noun = if count == 1 { "stream" } else { "streams" };
    let verb = if open { "Hide" } else { "Show" };
    rsx! {
        div { class: "data-stream data-stream-fold",
            div { class: "data-stream-name",
                Button { purpose: ActionPurpose::Toggle, variant: ButtonVariant::Icon, class: "data-stream-expand",
                    aria_expanded: open, aria_label: "{verb} the {count} {noun} with nothing to plot",
                    onclick: move |_| on_toggle.call(()), ChevronIcon { open } }
                span { class: "data-stream-expand-space" }
                span { class: "data-stream-kind", "aria-hidden": "true" }
                span { class: "data-stream-label", "{count} {noun} with nothing to plot" }
            }
            div { class: "data-stream-track" }
        }
    }
}

#[component]
fn StreamRow(
    stream: RecordedStream,
    transport: EpisodeTransport,
    mut choices: StreamChoices,
    expanded: bool,
    on_expand: EventHandler<bool>,
) -> Element {
    let key = stream.key().to_owned();
    let kind = stream.kind();
    let schema = choices.schema(&key);
    let no_numeric_fields =
        matches!(&schema, Some(StreamSchema::Ready(root)) if root.plottable_leaves().is_empty());
    let (state, check_label) = match kind {
        StreamKind::Video => (
            if choices.camera_open(&key) {
                PlotState::All
            } else {
                PlotState::None
            },
            format!("Show camera {}", stream.label()),
        ),
        StreamKind::Data => (choices.plot_state(&key), format!("Plot {}", stream.label())),
    };
    let count = stream
        .samples()
        .map(GroupedCountPresentation::grouped_label)
        .unwrap_or_else(|| "—".into());
    let schemas = stream.schemas().join(", ");
    let toggle_stream = stream.clone();
    rsx! {
        div { class: "data-stream", "data-kind": if kind == StreamKind::Video { "video" } else { "data" },
            div { class: "data-stream-name",
                if kind == StreamKind::Data {
                    Button { purpose: ActionPurpose::Toggle, variant: ButtonVariant::Icon, class: "data-stream-expand",
                        aria_expanded: expanded, aria_label: if expanded { "Hide fields" } else { "Show fields" },
                        onclick: move |_| on_expand.call(!expanded), ChevronIcon { open: expanded } }
                } else {
                    span { class: "data-stream-expand-space" }
                }
                ShowCheck { state, label: check_label, disabled: no_numeric_fields,
                    onclick: move |_| match kind {
                        StreamKind::Video => choices.set_camera(&key, state == PlotState::None),
                        StreamKind::Data => choices.set_stream_plotted(&toggle_stream, state == PlotState::None),
                    } }
                span { class: "data-stream-kind", "aria-hidden": "true",
                    if kind == StreamKind::Video { CameraStreamIcon {} } else { SignalStreamIcon {} }
                }
                span { class: "data-stream-label", title: "{stream.name()} · {schemas}",
                    span { class: "truncate", "{stream.label()}" }
                }
                span { class: "data-stream-count", title: "Messages in this episode", "{count}" }
            }
            div { class: "data-stream-track",
                StreamDensity { coverage: choices.coverage(stream.key()), transport: transport.clone() }
                TimelineScrubber { interaction: transport.interaction() }

            }
        }
        if expanded {
            match schema {
                None | Some(StreamSchema::Loading) => rsx! { FieldNote { note: "Reading message schema…".to_owned(), alert: false } },
                Some(StreamSchema::Failed(error)) => rsx! { FieldNote { note: error, alert: true } },
                Some(StreamSchema::Ready(root)) => {
                    let children = root.children();
                    if children.is_empty() {
                        rsx! { FieldRows { stream_key: stream.key().to_owned(), field: root, depth: 0, choices } }
                    } else {
                        rsx! { for child in children { FieldRows { key: "{child.label()}", stream_key: stream.key().to_owned(), field: child, depth: 0, choices } } }
                    }
                }
            }
        }
    }
}

#[component]
fn FieldNote(note: String, alert: bool) -> Element {
    rsx! {
        div { class: "data-field data-field-note", role: if alert { "alert" } else { "status" },
            div { class: "data-field-name", span { class: "data-field-check-space" } span { class: "data-field-label", "{note}" } }
            div { class: "data-field-track" }
        }
    }
}

#[component]
fn FieldRows(
    stream_key: String,
    field: MessageField,
    depth: usize,
    mut choices: StreamChoices,
) -> Element {
    let children = field.children();
    let is_list = field.type_label().starts_with("list");
    let leaf = if children.is_empty() {
        Some(field.clone())
    } else if is_list && children.len() == 1 && children[0].children().is_empty() {
        Some(children[0].clone())
    } else {
        None
    };
    match leaf {
        Some(leaf) => {
            let plottable = leaf.plottable();
            let plotted = choices.is_field_plotted(&stream_key, &leaf);
            let name = if is_list {
                format!("{}[]", field.name())
            } else {
                field.name().to_owned()
            };
            let kind = if is_list {
                format!("{} · {}", field.type_label(), leaf.type_label())
            } else {
                leaf.type_label()
            };
            let toggle_key = stream_key.clone();
            let toggle_field = leaf.clone();
            rsx! {
                div { class: "data-field", style: "--depth: {depth}",
                    div { class: "data-field-name",
                        if plottable {
                            ShowCheck { state: if plotted { PlotState::All } else { PlotState::None }, label: format!("Plot {}", leaf.label()),
                                onclick: move |_| choices.toggle_field(&toggle_key, toggle_field.clone()) }
                        } else {
                            span { class: "data-field-check-space" }
                        }
                        span { class: "data-field-label", title: leaf.label(), "{name}" }
                        span { class: "data-field-type", "{kind}" }
                    }
                    div { class: "data-field-track" }
                }
            }
        }
        None => rsx! {
            div { class: "data-field data-field-group", style: "--depth: {depth}",
                div { class: "data-field-name",
                    span { class: "data-field-check-space" }
                    span { class: "data-field-label", title: field.label(), "{field.name()}" }
                    span { class: "data-field-type", "{field.type_label()}" }
                }
                div { class: "data-field-track" }
            }
            for child in children {
                FieldRows { key: "{child.label()}", stream_key: stream_key.clone(), field: child, depth: depth + 1, choices }
            }
        },
    }
}

/// A checkbox that can also say "some": every stream row and field row uses it.
#[component]
fn ShowCheck(
    state: PlotState,
    label: String,
    #[props(default)] disabled: bool,
    onclick: EventHandler<()>,
) -> Element {
    let checked = match state {
        PlotState::All => "true",
        PlotState::Some => "mixed",
        PlotState::None => "false",
    };
    rsx! {
        button { r#type: "button", class: "data-show-check", role: "checkbox", "aria-checked": checked,
            "aria-label": label.clone(), title: if disabled { "No numeric fields to plot".to_owned() } else { label },
            disabled, onclick: move |_| onclick.call(()),
            svg { view_box: "0 0 12 12", "aria-hidden": "true",
                rect { x: "1", y: "1", width: "10", height: "10", rx: "2" }
                match state {
                    PlotState::All => rsx! { path { d: "m3 6.2 2 2 4-4.4" } },
                    PlotState::Some => rsx! { path { d: "M3.5 6h5" } },
                    PlotState::None => rsx! {},
                }
            }
        }
    }
}

#[component]
fn StreamDensity(coverage: Option<CoverageState>, transport: EpisodeTransport) -> Element {
    let range = transport.range();
    match coverage {
        None => rsx! { div { class: "data-track data-track-empty" } },
        Some(CoverageState::Loading) => {
            rsx! { div { class: "data-track data-track-loading", role: "status", "aria-label": "Counting messages" } }
        }
        Some(CoverageState::Failed(error)) => {
            rsx! { div { class: "data-track data-track-failed", title: "{error}", span { "Density unavailable" } } }
        }
        Some(CoverageState::Ready(coverage)) => {
            let moment = |nanos: Option<u64>| {
                nanos
                    .map(|nanos| range.seconds_of(nanos).precise_clock_label())
                    .unwrap_or_else(|| "—".into())
            };
            let title = format!(
                "{} messages · first {} · last {}",
                coverage.messages().grouped_label(),
                moment(coverage.first_ns()),
                moment(coverage.last_ns())
            );
            rsx! { DensityTrack { buckets: coverage.buckets().to_vec(), state: transport.timeline_state(), title } }
        }
    }
}
