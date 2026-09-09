//! Episode-first navigation: select a topic, then render its native media or table.
use super::query_result::{QueryExecutionPresentation, QueryState};
use super::{DatasetClient, episode_browser::EpisodeContext, episode_playback::EpisodePlayback};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeTopicRequest};

#[derive(Clone, Copy, PartialEq)]
enum TopicPresentation {
    Video,
    Table,
}
impl From<&DatasetTopic> for TopicPresentation {
    fn from(topic: &DatasetTopic) -> Self {
        if topic
            .schemas()
            .iter()
            .any(|schema| schema == "foxglove.CompressedVideo")
        {
            Self::Video
        } else {
            Self::Table
        }
    }
}
impl TopicPresentation {
    fn label(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Table => "Table",
        }
    }
}

#[component]
pub(super) fn EpisodeInspector(episode: EpisodeContext, on_back: EventHandler) -> Element {
    let request = episode.selection();
    let mut topics = use_resource(use_reactive((&request,), move |(request,)| async move {
        DatasetClient::current()
            .episode_topics(request)
            .await
            .map_err(|error| error.to_string())
    }));
    let mut selected = use_signal(|| None::<DatasetTopic>);
    let heading_id = use_hook(|| {
        format!(
            "episode-heading-{}",
            dioxus::dioxus_core::current_scope_id().0
        )
    });
    let focus_id = heading_id.clone();
    use_effect(move || {
        let _ = selected();
        crate::components::interaction::BrowserFocus::after_render(focus_id.clone());
    });
    let pending = *topics.state().read() == UseResourceState::Pending;
    let topic = selected();
    rsx! {
        nav { class: "data-breadcrumbs", "aria-label": "Dataset location",
            ol {
                li { button { class: "data-text-action", onclick: move |_| on_back.call(()), title: episode.dataset().label(), "{episode.dataset().label()}" } }
                li {
                    if topic.is_some() { button { class: "data-text-action", onclick: move |_| selected.set(None), "{episode.title()}" } }
                    else { span { "aria-current": "page", "{episode.title()}" } }
                }
                if let Some(topic) = &topic { li { span { "aria-current": "page", title: topic.name(), "{topic.name()}" } } }
            }
        }
        if let Some(topic) = topic {
            header { class: "data-topic-heading",
                h2 { id: "{heading_id}", tabindex: "-1", "{topic.name()}" }
                p { "{TopicPresentation::from(&topic).label()}" span { "{topic.schemas().join(\", \")}" } }
            }
            {
                let selection = episode.selection();
                let request = EpisodeTopicRequest::new(selection.dataset().clone(), topic.name().to_owned(), selection.episode_id().to_owned());
                match TopicPresentation::from(&topic) {
                    TopicPresentation::Video => match episode.timeline() {
                        Some(timeline) => rsx! { div { key: "{topic.name()}", class: "data-topic-video", EpisodePlayback { request, timeline } } },
                        None => rsx! { p { class: "data-empty-copy", "Episode timing is unavailable for video playback." } },
                    },
                    TopicPresentation::Table => rsx! { TopicTable { key: "{topic.name()}", request } },
                }
            }
        } else {
            header { class: "data-episode-heading",
                h2 { id: "{heading_id}", tabindex: "-1", "{episode.title()}" }
                {episode.render_summary()}
                {episode.render_details()}
            }
            header { class: "data-content-toolbar data-topic-list-heading",
                h3 { if let Some(Ok(topics)) = &*topics.read() { "Topics · {topics.len()}" } else { "Topics" } }
                button { class: "data-text-action", disabled: pending, onclick: move |_| topics.restart(), "Refresh topics" }
            }
            if pending { p { class: "data-empty-copy", role: "status", "Finding topics in this episode…" } }
            else { match &*topics.read() {
                Some(Err(error)) => rsx! { div { class: "data-empty-copy", role: "alert",
                    p { "Could not load this episode’s topics: {error}" }
                    button { class: "data-text-action", onclick: move |_| topics.restart(), "Retry topics" }
                } },
                Some(Ok(topics)) if topics.is_empty() => rsx! { p { class: "data-empty-copy", "No topics with messages in this episode." } },
                Some(Ok(topics)) => rsx! { div { class: "data-topic-list",
                    table { class: "data-ledger", "aria-label": "Episode topics",
                        thead { tr { th { "Topic" } th { "View" } } }
                        tbody { for topic in topics {
                            tr { key: "{topic.name()}",
                                td { button { class: "data-topic-action", onclick: { let topic = topic.clone(); move |_| selected.set(Some(topic.clone())) },
                                    span { class: "data-topic-name", "{topic.name()}" }
                                    span { class: "data-topic-schema", "{topic.schemas().join(\", \")}" }
                                } }
                                td { span { class: "data-topic-kind", "{TopicPresentation::from(topic).label()}" } }
                            }
                        } }
                    }
                } },
                None => rsx! {},
            } }
        }
    }
}

#[component]
fn TopicTable(request: EpisodeTopicRequest) -> Element {
    let query = request.clone();
    let mut preview = use_resource(move || {
        let query = query.clone();
        async move { DatasetClient::current().preview_episode_topic(query).await }
    });
    let pending = *preview.state().read() == UseResourceState::Pending;
    rsx! {
        div { class: "data-topic-table-tools",
            span { role: "status",
                if pending { "Loading messages…" }
                else if let Some(Ok(result)) = &*preview.read() {
                    "{result.rows().len()} messages"
                    if result.truncated() { " · showing the first 100" }
                }
            }
            button { class: "data-text-action", disabled: pending, onclick: move |_| preview.restart(), "Refresh preview" }
            details { class: "data-topic-query",
                summary { "Query details" }
                p { "$1 · UUID · {request.episode_id()}" }
                pre { "{request.sql()}" }
            }
        }
        if !pending { match &*preview.read() {
            Some(Ok(result)) => rsx! { {QueryExecutionPresentation::new(request.sql(), 100, QueryState::Ready(result.clone())).render()} },
            Some(Err(error)) => rsx! { div { class: "data-empty-copy", role: "alert",
                p { "Could not load topic messages: {error}" }
                button { class: "data-text-action", onclick: move |_| preview.restart(), "Retry preview" }
            } },
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
        let joints = DatasetTopic::new(
            "/camera/metadata".into(),
            vec!["studio.recording.Stream".into()],
        );
        assert!(TopicPresentation::from(&video) == TopicPresentation::Video);
        assert!(TopicPresentation::from(&joints) == TopicPresentation::Table);
        assert!(
            TopicPresentation::from(&DatasetTopic::new("/unknown".into(), vec![]))
                == TopicPresentation::Table
        );
    }
}
