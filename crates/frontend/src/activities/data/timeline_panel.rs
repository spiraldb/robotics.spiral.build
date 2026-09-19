//! One clock, one ruler, and one row per recorded stream.
//!
//! The Timeline is where an episode is navigated: the transport drives the
//! shared playhead, the ruler shows the visible span, and every stream row
//! shows where its messages are in time. Ticking a stream shows it — a camera
//! opens its panel, a data stream draws its numeric fields in Plots — and
//! expanding a data stream exposes its fields one by one.
use super::{
    episode_list::OpenedEpisode,
    episode_streams::{
        CoverageState, EpisodeStreams, PlotState, PlottableLeaves, RecordedStream, StreamChoices,
        StreamKind, StreamSchema,
    },
    episode_transport::{EpisodeTransport, KeyboardFocus, PLAYBACK_SPEEDS},
    message_buffer::MessageField,
    timeline_tracks::{NumericTrackRow, TimelineTrack},
};
use crate::components::icons::{
    CameraStreamIcon, FitTimeIcon, PauseIcon, PlayIcon, SignalStreamIcon, SkipToEndIcon,
    SkipToStartIcon, StepBackIcon, StepForwardIcon, ZoomInIcon, ZoomOutIcon,
};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant, ChevronIcon, Toggle};
use crate::format::{GroupedCountPresentation, PreciseClockPresentation};
use dioxus::html::geometry::WheelDelta;
use dioxus::prelude::*;
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
    #[props(default)] subject: Option<TimelineSubject>,
    #[props(default)] tracks: Vec<TimelineTrack>,
) -> Element {
    let mut keyboard = transport.clone();
    rsx! {
        section { class: "data-timeline", "aria-label": "Episode timeline", tabindex: "0",
            onkeydown: move |event: KeyboardEvent| {
                if KeyboardFocus::editing() {
                    return;
                }
                if keyboard.handle_key(&event) {
                    event.prevent_default();
                    event.stop_propagation();
                }
            },
            TransportBar { transport: transport.clone(), identity: subject.as_ref().map(|subject| subject.episode.clone()) }
            if subject.is_none() && transport.duration() <= 0.0 && tracks.is_empty() {
                div { class: "data-timeline-empty",
                    strong { "No episode open" }
                    span { "Choose an episode under Datasets to see its streams on this clock." }
                }
            } else {
                TimelineRows { subject, transport, tracks }
            }
        }
    }
}

trait SpeedPresentation {
    fn speed_label(self) -> String;
}
impl SpeedPresentation for f64 {
    fn speed_label(self) -> String {
        if self.fract() == 0.0 {
            format!("{self:.0}×")
        } else {
            format!("{self}×")
        }
    }
}

#[component]
fn TransportBar(transport: EpisodeTransport, identity: Option<OpenedEpisode>) -> Element {
    let playing = transport.playing();
    let duration = transport.duration();
    let seconds = transport.seconds();
    let clocked = duration > 0.0;
    let view = transport.view();
    let fitted = view.start() <= 0.0 && view.end() >= duration;
    let speed = transport.speed();
    let looping = transport.looping();
    let buffering = transport.buffering();
    let mut start = transport.clone();
    let mut back = transport.clone();
    let mut toggle = transport.clone();
    let mut forward = transport.clone();
    let mut end = transport.clone();
    let mut speed_select = transport.clone();
    let mut loop_toggle = transport.clone();
    let mut zoom_in = transport.clone();
    let mut zoom_out = transport.clone();
    let mut fit = transport.clone();
    rsx! {
        header { class: "data-transport", role: "toolbar", "aria-label": "Playback",
            div { class: "data-transport-identity",
                match identity {
                    Some(episode) => rsx! {
                        strong { "{episode.listed().title()}" }
                        span { class: "truncate", title: "{episode.record().facts_label()}\n{episode.record().source_uri().unwrap_or_default()}", "{episode.label().head()}" }
                    },
                    None => rsx! { span { class: "text-ink-faint", "Timeline" } },
                }
            }
            div { class: "data-transport-group", role: "group", "aria-label": "Transport",
                Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon, disabled: !clocked,
                    title: "Jump to start · Home", aria_label: "Jump to start",
                    onclick: move |_| start.seek(0.0), SkipToStartIcon {} }
                Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon, disabled: !clocked,
                    title: "Back 0.1 s · ← (Shift 1 s, Alt 10 ms)", aria_label: "Step back",
                    onclick: move |_| back.step(-0.1), StepBackIcon {} }
                Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Icon, class: "data-transport-play", disabled: !clocked,
                    title: if playing { "Pause · Space" } else { "Play · Space" },
                    aria_label: if playing { "Pause" } else { "Play" }, aria_pressed: playing,
                    onclick: move |_| toggle.toggle(),
                    if playing { PauseIcon {} } else { PlayIcon {} } }
                Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon, disabled: !clocked,
                    title: "Forward 0.1 s · → (Shift 1 s, Alt 10 ms)", aria_label: "Step forward",
                    onclick: move |_| forward.step(0.1), StepForwardIcon {} }
                Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon, disabled: !clocked,
                    title: "Jump to end · End", aria_label: "Jump to end",
                    onclick: move |_| { let duration = end.duration(); end.seek(duration); }, SkipToEndIcon {} }
            }
            output { class: "data-transport-clock", "aria-live": "off",
                span { "{seconds.precise_clock_label()}" }
                span { class: "data-transport-total", "/ {duration.precise_clock_label()}" }
            }
            label { class: "data-transport-speed", title: "Playback speed",
                span { class: "sr-only", "Speed" }
                select { value: "{speed}", disabled: !clocked, "aria-label": "Playback speed",
                    onchange: move |event| if let Ok(value) = event.value().parse::<f64>() { speed_select.set_speed(value); },
                    for option in PLAYBACK_SPEEDS {
                        option { value: "{option}", selected: option == speed, "{option.speed_label()}" }
                    }
                }
            }
            Toggle { label: "Loop".to_owned(), checked: looping, disabled: !clocked, class: "data-transport-loop".to_owned(),
                title: "Start over when the episode ends".to_owned(), onchange: move |_| loop_toggle.toggle_loop() }
            span { class: "data-transport-note", role: "status", if buffering { "Buffering…" } }
            div { class: "data-transport-group data-transport-zoom", role: "group", "aria-label": "Time zoom",
                Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Icon, disabled: !clocked,
                    title: "Zoom out · −", aria_label: "Zoom out", onclick: move |_| { let anchor = zoom_out.seconds(); zoom_out.zoom(2.0, anchor); }, ZoomOutIcon {} }
                Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Icon, disabled: !clocked,
                    title: "Zoom in around the playhead · + (Ctrl/⌘ + wheel)", aria_label: "Zoom in", onclick: move |_| { let anchor = zoom_in.seconds(); zoom_in.zoom(0.5, anchor); }, ZoomInIcon {} }
                Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Icon, disabled: !clocked || fitted,
                    title: "Fit the whole episode · 0", aria_label: "Fit episode", onclick: move |_| fit.fit(), FitTimeIcon {} }
            }
        }
    }
}

#[component]
fn TimelineRows(
    subject: Option<TimelineSubject>,
    transport: EpisodeTransport,
    tracks: Vec<TimelineTrack>,
) -> Element {
    let mut filter = use_signal(String::new);
    let expanded = use_signal(BTreeSet::<String>::new);
    let mut quiet_open = use_signal(|| false);
    let view = transport.view();
    let duration = transport.duration();
    let seconds = transport.seconds();
    let playhead = view.percent(seconds);
    let mut seek = transport.clone();
    let mut wheel = transport.clone();
    let catalog = subject.as_ref().and_then(|subject| match &subject.streams {
        EpisodeStreams::Ready(catalog) => Some(catalog),
        _ => None,
    });
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
    let choices = subject.as_ref().map(|subject| subject.choices);
    let (cameras, data): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|stream| stream.kind() == StreamKind::Video);
    let (quiet, loud): (Vec<_>, Vec<_>) = data.into_iter().partition(|stream| {
        !searching && choices.is_some_and(|choices| choices.stream_is_quiet(stream))
    });
    let leading = cameras.into_iter().chain(loud).collect::<Vec<_>>();
    let nothing_listed = leading.is_empty() && quiet.is_empty();
    rsx! {
        div { class: "data-timeline-body", style: "--playhead: {playhead / 100.0:.6}",
            onwheel: move |event: WheelEvent| {
                let modifiers = event.modifiers();
                let (dx, dy) = match event.delta() {
                    WheelDelta::Pixels(delta) => (delta.x, delta.y),
                    WheelDelta::Lines(delta) => (delta.x * 16.0, delta.y * 16.0),
                    WheelDelta::Pages(delta) => (delta.x * 400.0, delta.y * 400.0),
                };
                if modifiers.ctrl() || modifiers.meta() {
                    event.prevent_default();
                    let anchor = wheel.seconds();
                    wheel.zoom(if dy > 0.0 { 1.25 } else { 0.8 }, anchor);
                } else if modifiers.shift() || dx.abs() > dy.abs() {
                    event.prevent_default();
                    let delta = if modifiers.shift() { dy } else { dx };
                    let span = wheel.view().span();
                    wheel.pan(delta / 800.0 * span);
                }
            },
            div { class: "data-timeline-ruler",
                div { class: "data-timeline-corner",
                    if subject.is_some() {
                        input { class: "field data-stream-filter", r#type: "search",
                            placeholder: "Filter {total} streams", "aria-label": "Filter streams",
                            value: filter(), oninput: move |event| filter.set(event.value()) }
                    } else { span { class: "text-ink-soft text-[11px]", "Tracks" } }
                }
                div { class: "data-timeline-scale",
                    for tick in view.ticks() {
                        div { key: "{tick.seconds()}", class: "data-time-tick", style: "left: {tick.position():.4}%",
                            "data-edge": if tick.position() > 90.0 { "end" } else { "start" },
                            span { "{tick.label()}" }
                        }
                    }
                    input { r#type: "range", class: "data-time-scrub",
                        min: "{view.start()}", max: "{view.end()}", step: "0.001", value: "{seconds}", disabled: duration <= 0.0,
                        "aria-label": "Playhead", "aria-valuetext": "{seconds.precise_clock_label()} of {duration.precise_clock_label()}",
                        oninput: move |event| if let Ok(value) = event.value().parse::<f64>() { seek.seek(value); } }
                    div { class: "data-timeline-handle", style: "left: {playhead:.4}%", "aria-hidden": "true" }
                }
            }
            div { class: "data-timeline-rows",
                for track in tracks {
                    NumericTrackRow { key: "{track.id()}", track, transport: transport.clone() }
                }
                if let Some(subject) = subject {
                    match subject.streams {
                        EpisodeStreams::Loading => rsx! { div { class: "data-timeline-none", role: "status", "Finding recorded streams…" } },
                        EpisodeStreams::Failed(error) => rsx! { div { class: "data-timeline-none", role: "alert", "Could not read this episode's streams: {error}" } },
                        EpisodeStreams::Ready(catalog) => rsx! {
                            if nothing_listed {
                                p { class: "data-timeline-none", if catalog.is_empty() { "This episode has no readable streams." } else { "No streams match the filter." } }
                            }
                            StreamRows { streams: leading, transport: transport.clone(), choices: subject.choices, expanded }
                            if !quiet.is_empty() {
                                QuietStreamsRow { count: quiet.len(), open: quiet_open(), on_toggle: move |_| quiet_open.toggle() }
                                if quiet_open() {
                                    StreamRows { streams: quiet, transport: transport.clone(), choices: subject.choices, expanded }
                                }
                            }
                        },
                    }
                }
                div { class: "data-timeline-cursor", "aria-hidden": "true" }
            }
        }
    }
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
/// leads with cameras and measurements. They stay one click away, still
/// selectable for Messages.
#[component]
fn QuietStreamsRow(count: usize, open: bool, on_toggle: EventHandler<()>) -> Element {
    let noun = if count == 1 { "stream" } else { "streams" };
    let verb = if open { "Hide" } else { "Show" };
    rsx! {
        div { class: "data-stream data-stream-fold",
            div { class: "data-stream-name",
                Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Icon, class: "data-stream-expand",
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

/// Stream density and additional numeric rows seek through the same input.
#[component]
pub(super) fn TrackScrubber(mut transport: EpisodeTransport) -> Element {
    let view = transport.view();
    rsx! { input { r#type: "range", class: "data-track-scrub", tabindex: "-1", "aria-hidden": "true",
    min: "{view.start()}", max: "{view.end()}", step: "0.001", value: "{transport.seconds()}",
    oninput: move |event| if let Ok(value) = event.value().parse::<f64>() { transport.seek(value); } } }
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
    let selected = choices.selected().as_deref() == Some(stream.key());
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
    let select_key = key.clone();
    rsx! {
        div { class: "data-stream", "data-selected": selected, "data-kind": if kind == StreamKind::Video { "video" } else { "data" },
            div { class: "data-stream-name",
                if kind == StreamKind::Data {
                    Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Icon, class: "data-stream-expand",
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
                button { r#type: "button", class: "data-stream-label", aria_pressed: selected,
                    title: "{stream.name()} · {schemas}",
                    onclick: move |_| choices.select(if selected { None } else { Some(select_key.clone()) }),
                    span { class: "truncate", "{stream.label()}" }
                }
                span { class: "data-stream-count", title: "Messages in this episode", "{count}" }
            }
            div { class: "data-stream-track",
                DensityTrack { coverage: choices.coverage(stream.key()), transport: transport.clone() }
                TrackScrubber { transport }

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

/// Where a stream's messages are, drawn as one bar per timeline bucket.
#[component]
fn DensityTrack(coverage: Option<CoverageState>, transport: EpisodeTransport) -> Element {
    let view = transport.view();
    let duration = transport.duration();
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
            let buckets = coverage.buckets();
            let count = buckets.len() as f64;
            let scale = if duration > 0.0 {
                count / duration
            } else {
                count
            };
            let x0 = view.start() * scale;
            let width = ((view.end() - view.start()) * scale).max(1e-6);
            let tiers = buckets.density_paths();
            let gaps = buckets.gap_path();
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
            rsx! {
                div { class: "data-track", title,
                    svg { class: "data-density", view_box: "{x0} 0 {width} 1", preserve_aspect_ratio: "none", "aria-hidden": "true",
                        path { class: "data-density-1", d: "{tiers[0]}" }
                        path { class: "data-density-2", d: "{tiers[1]}" }
                        path { class: "data-density-3", d: "{tiers[2]}" }
                        path { class: "data-density-gap", d: "{gaps}" }
                    }
                }
            }
        }
    }
}

/// Runs of equally dense buckets become one bar each, so a steady 30 Hz
/// stream draws as a solid band rather than 240 seams.
trait DensityPaths {
    fn density_paths(&self) -> [String; 3];
    /// Empty buckets between the first and last message of a dense stream:
    /// a dropout worth a glance. A sparse stream has spacing, not gaps, and
    /// draws none.
    fn gap_path(&self) -> String;
}

impl DensityPaths for [u32] {
    fn gap_path(&self) -> String {
        let Some(first) = self.iter().position(|count| *count > 0) else {
            return String::new();
        };
        let last = self.iter().rposition(|count| *count > 0).unwrap_or(first);
        let span = &self[first..=last];
        let occupied = span.iter().filter(|count| **count > 0).count();
        if occupied * 2 < span.len() {
            return String::new();
        }
        let mut path = String::new();
        let mut run: Option<(usize, usize)> = None;
        for (index, count) in span.iter().enumerate() {
            match (run, *count == 0) {
                (Some((start, length)), true) => run = Some((start, length + 1)),
                (Some((start, length)), false) => {
                    path.push_str(&format!("M{} .42h{length}v.16h-{length}z", first + start));
                    run = None;
                }
                (None, true) => run = Some((index, 1)),
                (None, false) => {}
            }
        }
        path
    }
    fn density_paths(&self) -> [String; 3] {
        // Tiers compare against the stream's typical occupied bucket, not its
        // peak: a 30 Hz stream over 240 buckets alternates between 2 and 3
        // messages per bucket (with the odd 4), and must read as one solid
        // band rather than a moiré of alternating tints.
        let occupied = self.iter().filter(|count| **count > 0).count().max(1) as f64;
        let typical = self.iter().map(|count| f64::from(*count)).sum::<f64>() / occupied;
        let tier_of = |count: u32| -> Option<usize> {
            if count == 0 {
                return None;
            }
            let share = f64::from(count) / typical.max(1.0);
            Some(if share >= 0.6 {
                2
            } else if share >= 0.3 {
                1
            } else {
                0
            })
        };
        let mut paths = [String::new(), String::new(), String::new()];
        let mut run: Option<(usize, usize, usize)> = None;
        for (index, count) in self.iter().enumerate() {
            let tier = tier_of(*count);
            match (run, tier) {
                (Some((start, length, current)), Some(tier)) if tier == current => {
                    run = Some((start, length + 1, current));
                }
                _ => {
                    if let Some((start, length, current)) = run.take() {
                        paths[current].push_str(&format!("M{start} .2h{length}v.6h-{length}z"));
                    }
                    run = tier.map(|tier| (index, 1, tier));
                }
            }
        }
        if let Some((start, length, current)) = run {
            paths[current].push_str(&format!("M{start} .2h{length}v.6h-{length}z"));
        }
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn density_runs_merge_into_single_bars_per_tier() {
        // Typical occupied bucket is 7.2: 10s are dense, 4 is medium, 2 is sparse.
        let paths = [10_u32, 10, 2, 0, 0, 4, 10].density_paths();
        assert_eq!(paths[2], "M0 .2h2v.6h-2zM6 .2h1v.6h-1z");
        assert_eq!(paths[0], "M2 .2h1v.6h-1z");
        assert_eq!(paths[1], "M5 .2h1v.6h-1z");
        let steady = [3_u32, 2, 3, 4, 2, 3].density_paths();
        assert_eq!(steady[2], "M0 .2h6v.6h-6z");
        assert_eq!([1_u32].density_paths()[2], "M0 .2h1v.6h-1z");
        assert!([0_u32, 0].density_paths().iter().all(String::is_empty));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn only_dense_streams_report_gaps_and_only_between_their_messages() {
        // A dropout inside a steady stream is a gap; leading and trailing
        // silence is not.
        assert_eq!([0_u32, 3, 3, 0, 0, 3, 3, 0].gap_path(), "M3 .42h2v.16h-2z");
        assert_eq!(
            [3_u32, 0, 3, 3, 0, 3].gap_path(),
            "M1 .42h1v.16h-1zM4 .42h1v.16h-1z"
        );
        // A sparse stream is spacing, not gaps; a single message has neither.
        assert_eq!([1_u32, 0, 0, 0, 0, 0, 0, 1].gap_path(), "");
        assert_eq!([0_u32, 1, 0].gap_path(), "");
        assert_eq!([0_u32, 0].gap_path(), "");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn speeds_read_as_multipliers() {
        assert_eq!(0.25_f64.speed_label(), "0.25×");
        assert_eq!(1.0_f64.speed_label(), "1×");
        assert_eq!(4.0_f64.speed_label(), "4×");
    }
}
