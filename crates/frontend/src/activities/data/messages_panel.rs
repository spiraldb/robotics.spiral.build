//! Raw messages of the selected stream, read forward from the playhead.
use super::{
    episode_streams::{EpisodeStreams, RecordedStream, StreamChoices, StreamKind},
    episode_transport::EpisodeTransport,
    message_buffer::MessageBuffer,
    query_result::{QueryExecutionPresentation, QueryState},
};
use crate::components::icons::{CameraStreamIcon, SignalStreamIcon};
use crate::format::PreciseClockPresentation;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{MessageCursor, MessageWindow};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant};

const MESSAGE_PAGE_ROWS: usize = 100;

#[component]
pub(super) fn MessagesPanel(
    streams: Option<EpisodeStreams>,
    transport: EpisodeTransport,
    choices: StreamChoices,
) -> Element {
    let Some(EpisodeStreams::Ready(catalog)) = streams else {
        return rsx! {
            div { class: "data-messages-empty",
                strong { "No episode open" }
                p { "Messages of a selected stream read here, starting at the playhead." }
            }
        };
    };
    let selected = choices
        .selected()
        .and_then(|key| catalog.get(&key).cloned());
    match selected {
        Some(stream) => rsx! { MessageReader { key: "{stream.key()}", stream, transport } },
        None => rsx! {
            div { class: "data-messages-empty",
                strong { "No stream selected" }
                p { "Click a stream name in the Timeline to read its messages from the playhead." }
            }
        },
    }
}

#[component]
fn MessageReader(stream: RecordedStream, transport: EpisodeTransport) -> Element {
    let range = transport.range();
    let mut anchor_ns = use_signal(|| range.ns_at(transport.seconds()));
    let mut cursor = use_signal(|| None::<MessageCursor>);
    let read = stream.read().clone();
    let mut page = use_resource(move || {
        let read = read.clone();
        let start = anchor_ns();
        let after = cursor();
        async move {
            let window = MessageWindow::new((start, range.bounds().1.max(start + 1)), vec![])
                .with_limit(MESSAGE_PAGE_ROWS)
                .with_cursor(after);
            let buffer = MessageBuffer::load(read.with_window(window), false).await?;
            let snapshot = buffer.snapshot_on_clock(range.start_ns())?;
            Ok::<_, String>((snapshot, buffer.next_cursor().cloned()))
        }
    });
    let pending = *page.state().read() == UseResourceState::Pending;
    let next = match &*page.read() {
        Some(Ok((_, next))) => next.clone(),
        _ => None,
    };
    let at_playhead = transport.clone();
    let has_next = next.is_some();
    rsx! {
        section { class: "data-messages", "aria-label": "{stream.label()} messages",
            header { class: "data-messages-head",
                if stream.kind() == StreamKind::Video { CameraStreamIcon {} } else { SignalStreamIcon {} }
                strong { class: "truncate", title: stream.name(), "{stream.label()}" }
                span { class: "data-messages-from",
                    "from {range.seconds_of(anchor_ns()).precise_clock_label()}"
                    if cursor().is_some() { " · continued" }
                }
                Button { purpose: ActionPurpose::Refresh, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                    disabled: pending, title: "Read the next {MESSAGE_PAGE_ROWS} messages at or after the playhead",
                    onclick: move |_| { anchor_ns.set(at_playhead.absolute_ns()); cursor.set(None); },
                    "From playhead" }
                Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                    disabled: pending || !has_next, title: "Continue after the last row shown",
                    onclick: move |_| cursor.set(next.clone()),
                    "Next {MESSAGE_PAGE_ROWS}" }
            }
            match &*page.read() {
                Some(Ok((snapshot, _))) if !pending => rsx! {
                    {QueryExecutionPresentation::new(format!("{} from playhead", stream.name()), MESSAGE_PAGE_ROWS, QueryState::Ready(snapshot.clone())).render()}
                },
                Some(Err(error)) if !pending => rsx! {
                    div { class: "data-messages-error", role: "alert",
                        p { "{error}" }
                        Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, size: ButtonSize::Compact, onclick: move |_| page.restart(), "Retry" }
                    }
                },
                _ => rsx! {
                    {QueryExecutionPresentation::new(stream.name().to_owned(), MESSAGE_PAGE_ROWS, QueryState::Running).render()}
                },
            }
        }
    }
}
