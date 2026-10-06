//! The inspected episode's timeline: the rows the surface brings, then every
//! stream the episode recorded, on the episode's one clock.
//!
//! A surface's rows are its evidence — a progress score, a measurement's
//! subtasks — so they lead, and activating one of their intervals plays it.
//! The streams follow as the hub the operator steers the rest from: ticking a
//! camera shows its panel, ticking a stream plots its numeric fields, and a
//! ticked row previews what it shows along its track — a camera's stills, a
//! stream's plotted values — so when something happened reads across the
//! rows. An unticked row stays one quiet line. Under
//! evidence they wait folded behind one row, since the streams say nothing
//! about the measurement being checked.
use super::{
    choices::{PlotState, PlottableLeaves, StreamSchema},
    filmstrip::CameraFilmstrip,
    inspection::LoadedEpisode,
    messages::MessageField,
    reads::StreamRead,
    sparkline::ValuesSparkline,
    streams::{RecordedStream, StreamKind},
};
use crate::components::{
    icons::{CameraStreamIcon, SignalStreamIcon},
    timeline::{
        IntervalTrack, IntervalTrackRow, NumericTrackRow, Timeline, TimelineContent,
        TimelineScrubber, TimelineTrack,
    },
};
use crate::format::GroupedCountPresentation;
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, ChevronIcon, TextInput, TextInputKind,
};
use std::collections::BTreeSet;

/// A row a surface draws above the recorded streams.
#[derive(Clone, PartialEq)]
pub(in crate::activities) enum EvidenceTrack {
    /// A measurement's value over time, such as a progress score.
    Values(TimelineTrack),
    /// Named spans, such as a measurement's subtasks; activating one plays it.
    /// A row with no span in it is left out rather than drawn empty.
    Intervals(IntervalTrack),
}

impl EvidenceTrack {
    /// A value row as a curation draws it, tall enough to read a shape in
    /// (`curation-review.css`).
    const VALUES_ROW: f64 = 168.0;
    /// A row of spans: one line of their names.
    const INTERVALS_ROW: f64 = 44.0;

    /// How tall its row is on the timeline; a row of spans with none in it
    /// is not drawn.
    pub(super) fn row_height(&self) -> f64 {
        match self {
            Self::Values(_) => Self::VALUES_ROW,
            Self::Intervals(track) if track.is_empty() => 0.0,
            Self::Intervals(_) => Self::INTERVALS_ROW,
        }
    }
}

impl From<TimelineTrack> for EvidenceTrack {
    fn from(track: TimelineTrack) -> Self {
        Self::Values(track)
    }
}

impl From<IntervalTrack> for EvidenceTrack {
    fn from(track: IntervalTrack) -> Self {
        Self::Intervals(track)
    }
}

#[component]
pub(super) fn EpisodeTimeline(loaded: LoadedEpisode, evidence: Vec<EvidenceTrack>) -> Element {
    let mut filter = use_signal(String::new);
    let expanded = use_signal(BTreeSet::<String>::new);
    let mut quiet_open = use_signal(|| false);
    let mut streams_open = use_signal(|| false);
    let transport = loaded.transport().clone();
    let choices = loaded.choices();
    let catalog = loaded.catalog();
    let needle = filter().to_lowercase();
    let searching = !needle.is_empty();
    let rows = catalog
        .streams()
        .iter()
        .filter(|stream| !searching || stream.label().to_lowercase().contains(&needle))
        .cloned()
        .collect::<Vec<_>>();
    let total = catalog.streams().len();
    // Cameras lead, then streams with something to plot; the rest wait behind
    // one row until asked for. A search shows every match in place.
    let (cameras, data): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|stream| stream.kind() == StreamKind::Video);
    let (quiet, loud): (Vec<_>, Vec<_>) = data
        .into_iter()
        .partition(|stream| !searching && choices.stream_is_quiet(stream));
    let leading = cameras.into_iter().chain(loud).collect::<Vec<_>>();
    let nothing_listed = leading.is_empty() && quiet.is_empty();
    // A search shows its matches in place, folded or not.
    let folded = !evidence.is_empty() && !searching;
    let interaction = transport.interaction();
    let corner = rsx! { TextInput { class: "data-stream-filter", kind: TextInputKind::Search,
    placeholder: "Filter {total} streams", "aria-label": "Filter streams",
    value: filter(), oninput: move |value| filter.set(value) } };
    let rows = rsx! {
        for track in evidence {
            match track {
                EvidenceTrack::Values(track) => rsx! { NumericTrackRow { key: "{track.id()}", track, interaction: interaction.clone() } },
                EvidenceTrack::Intervals(track) if !track.is_empty() => rsx! { IntervalTrackRow { key: "{track.id()}", track, interaction: interaction.clone() } },
                EvidenceTrack::Intervals(_) => rsx! {},
            }
        }
        if folded {
            StreamFoldRow { label: Counted(total, "stream").to_string(), open: streams_open(),
                on_toggle: move |_| streams_open.toggle() }
        }
        if !folded || streams_open() {
            if nothing_listed { p { class: "data-timeline-none", if catalog.is_empty() { "This episode has no readable streams." } else { "No streams match the filter." } } }
            StreamRows { streams: leading, loaded: loaded.clone(), expanded }
            if !quiet.is_empty() {
                StreamFoldRow { label: format!("{} with nothing to plot", Counted(quiet.len(), "stream")), open: quiet_open(),
                    on_toggle: move |_| quiet_open.toggle() }
                if quiet_open() { StreamRows { streams: quiet, loaded: loaded.clone(), expanded } }
            }
        }
    };
    // Every surface names the episode above its inspection, so the transport
    // leads with its controls rather than repeating the name.
    let content = TimelineContent::new(rows)
        .with_identity(rsx! {})
        .with_corner(corner);
    rsx! { Timeline { interaction, content } }
}

#[component]
fn StreamRows(
    streams: Vec<RecordedStream>,
    loaded: LoadedEpisode,
    expanded: Signal<BTreeSet<String>>,
) -> Element {
    let choices = loaded.choices();
    rsx! {
        for stream in streams {
            StreamRow { key: "{stream.key()}", stream: stream.clone(), loaded: loaded.clone(),
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

/// A count and the singular noun it counts, said the way a person would.
struct Counted(usize, &'static str);

impl std::fmt::Display for Counted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(count, noun) = self;
        write!(
            formatter,
            "{count} {noun}{}",
            if *count == 1 { "" } else { "s" }
        )
    }
}

/// One row standing in for streams the hub does not lead with — every
/// recorded stream under a surface's evidence, or those with nothing to plot —
/// so they stay one click away without taking the rows.
#[component]
fn StreamFoldRow(label: String, open: bool, on_toggle: EventHandler<()>) -> Element {
    let verb = if open { "Hide" } else { "Show" };
    rsx! {
        div { class: "data-stream data-stream-fold",
            div { class: "data-stream-name",
                Button { purpose: ActionPurpose::Toggle, variant: ButtonVariant::Icon, class: "data-stream-expand",
                    aria_expanded: open, aria_label: "{verb} the {label}",
                    onclick: move |_| on_toggle.call(()), ChevronIcon { open } }
                span { class: "data-stream-label", "{label}" }
            }
            div { class: "data-stream-track" }
        }
    }
}

/// What a shown stream's row previews along its track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowPreview {
    /// A camera's stills.
    Filmstrip,
    /// A stream's plotted values.
    Sparkline,
}

impl RowPreview {
    /// What a row previews: what its stream shows, and nothing while it is
    /// hidden, so a hidden row stays quiet and reads nothing.
    fn of(kind: StreamKind, state: PlotState) -> Option<Self> {
        match (kind, state) {
            (_, PlotState::None) => None,
            (StreamKind::Video, _) => Some(Self::Filmstrip),
            (StreamKind::Data, _) => Some(Self::Sparkline),
        }
    }
    fn attribute(self) -> &'static str {
        match self {
            Self::Filmstrip => "filmstrip",
            Self::Sparkline => "sparkline",
        }
    }
}

#[component]
fn StreamRow(
    stream: RecordedStream,
    loaded: LoadedEpisode,
    expanded: bool,
    on_expand: EventHandler<bool>,
) -> Element {
    let transport = loaded.transport().clone();
    let mut choices = loaded.choices();
    let reads = loaded.reads();
    let key = stream.key().to_owned();
    let kind = stream.kind();
    let schema = choices.schema(&key);
    let no_numeric_fields =
        matches!(&schema, Some(StreamSchema::Ready(root)) if root.plottable_leaves().is_empty());
    // A camera is called what its picture is captioned: the part of its name
    // that tells it from the episode's other cameras.
    let label = match kind {
        StreamKind::Video => loaded.catalog().camera_title(&stream),
        StreamKind::Data => stream.label().to_owned(),
    };
    let (state, check_label) = match kind {
        StreamKind::Video => (
            if choices.camera_open(&key) {
                PlotState::All
            } else {
                PlotState::None
            },
            format!("Show camera {label}"),
        ),
        StreamKind::Data => (choices.plot_state(&key), format!("Plot {label}")),
    };
    let count = stream
        .samples()
        .map(GroupedCountPresentation::grouped_label)
        .unwrap_or_else(|| "—".into());
    let about = format!(
        "{} · {} · {count} messages",
        stream.name(),
        stream.schemas().join(", ")
    );
    let preview = RowPreview::of(kind, state);
    let fields = choices.plotted_fields(&key);
    let mut wanting = reads;
    use_effect(use_reactive(
        (&stream, &preview, &fields),
        move |(stream, preview, fields)| match preview {
            Some(RowPreview::Filmstrip) => wanting.want_camera(&stream),
            Some(RowPreview::Sparkline) => wanting.want_values(&stream, fields),
            None => {}
        },
    ));
    let view = transport.view();
    let hidden = choices.hidden_traces(&key);
    let toggle_stream = stream.clone();
    rsx! {
        div { class: "data-stream", "data-kind": if kind == StreamKind::Video { "video" } else { "data" },
            "data-shown": state != PlotState::None, "data-preview": preview.map(RowPreview::attribute),
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
                span { class: "data-stream-label", title: "{about}",
                    span { class: "truncate", "{label}" }
                }
            }
            div { class: "data-stream-track",
                match preview {
                    Some(RowPreview::Filmstrip) => match reads.camera(&stream) {
                        Some(StreamRead::Ready(camera)) => rsx! { CameraFilmstrip { camera, view } },
                        Some(StreamRead::Failed(error)) => rsx! {
                            span { class: "episode-filmstrip-note", title: "{error}", "Stills unavailable" }
                        },
                        _ => rsx! { div { class: "episode-filmstrip", "data-reading": "true", "aria-hidden": "true" } },
                    },
                    Some(RowPreview::Sparkline) => match reads.values(&stream).as_ref().and_then(StreamRead::latest) {
                        Some(plotted) => rsx! {
                            ValuesSparkline { plotted: plotted.clone(), lane: None, hidden, view }
                        },
                        None => rsx! {},
                    },
                    None => rsx! {},
                }
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
                        rsx! { FieldRows { stream: stream.clone(), field: root, depth: 0, loaded: loaded.clone() } }
                    } else {
                        rsx! { for child in children { FieldRows { key: "{child.label()}", stream: stream.clone(), field: child, depth: 0, loaded: loaded.clone() } } }
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
    stream: RecordedStream,
    field: MessageField,
    depth: usize,
    loaded: LoadedEpisode,
) -> Element {
    let mut choices = loaded.choices();
    let stream_key = stream.key().to_owned();
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
            // A plotted field previews its own lane, read with its stream's.
            let values = loaded.reads().values(&stream);
            let sparkline = values
                .as_ref()
                .and_then(StreamRead::latest)
                .filter(|_| plotted);
            let view = loaded.transport().view();
            let hidden = choices.hidden_traces(&stream_key);
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
                    div { class: "data-field-track",
                        if let Some(plotted) = sparkline {
                            ValuesSparkline { plotted: plotted.clone(), lane: Some(leaf.label()), hidden, view }
                        }
                    }
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
                FieldRows { key: "{child.label()}", stream: stream.clone(), field: child, depth: depth + 1, loaded: loaded.clone() }
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
