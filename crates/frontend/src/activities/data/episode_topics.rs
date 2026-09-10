//! Discover the selected native episode and retain raw topic inspection.
use super::{DatasetClient, episode_browser::EpisodeContext, episode_workspace::EpisodeWorkspace};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeTopicRequest};

#[component]
pub(super) fn EpisodeInspector(episode: EpisodeContext, on_back: EventHandler) -> Element {
    let request = episode.selection();
    let mut topics = use_resource(use_reactive((&request,), move |(request,)| async move {
        DatasetClient::current()
            .episode_topics(request)
            .await
            .map_err(|error| error.to_string())
    }));
    let pending = *topics.state().read() == UseResourceState::Pending;
    rsx! {
        if pending { p { class: "data-empty-copy", role: "status", "Finding recorded streams…" } }
        else { match &*topics.read() {
            Some(Ok(topics)) => match episode.timeline() {
                Some(timeline) => rsx! { EpisodeWorkspace { episode, topics: topics.clone(), timeline, on_back } },
                None => rsx! { UntimedEpisode { episode, topics: topics.clone(), on_back } },
            },
            Some(Err(error)) => rsx! { div { class: "data-empty-copy", role: "alert",
                p { "Could not load recorded streams: {error}" }
                button { class: "data-text-action", onclick: move |_| topics.restart(), "Retry topics" }
                button { class: "data-text-action", onclick: move |_| on_back.call(()), "Back to episodes" }
            } },
            None => rsx! {},
        } }
    }
}

#[component]
fn UntimedEpisode(
    episode: EpisodeContext,
    topics: Vec<DatasetTopic>,
    on_back: EventHandler,
) -> Element {
    let mut selected = use_signal(|| None::<usize>);
    let selection = episode.selection();
    rsx! {
        div { class: "data-topic-table-tools",
            button { class: "data-text-action", onclick: move |_| on_back.call(()), "Back to episodes" }
            p { "Episode timing is unavailable. Recorded messages are still inspectable." }
            select { "aria-label": "Recorded topic", value: selected().map(|i| i.to_string()).unwrap_or_default(), onchange: move |event| selected.set(event.value().parse().ok()),
                option { value: "", "Choose a topic" }
                for (index, topic) in topics.iter().enumerate() { option { value: "{index}", "{topic.name()}" } }
            }
        }
        if let Some(index) = selected() {
            if let Some(topic) = topics.get(index) {
                if let Some(revision) = topic.revision() {
                    super::episode_topic::MessageTable { key: "{index}", source: spiral_robotics_common::datasets::MessageRead::schema(EpisodeTopicRequest::new(selection.dataset().clone(), topic.name().into(), selection.episode_id().into()), revision.clone()), timeline: super::episode_transport::EpisodeTimeRange::new(0, u64::MAX) }
                }
            }
        }
    }
}
