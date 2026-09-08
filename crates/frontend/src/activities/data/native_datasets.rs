//! Dataset navigation and inspection share selection, while each mounted panel owns its reads.
use super::{
    DatasetClient,
    query_result::{QueryExecutionPresentation, QueryState},
};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::project::ProjectSessionSignals;
use dioxus::prelude::*;
use spiral_robotics_common::native_datasets::{NativeDatasetInspection, NativeDatasetSelection};

#[component]
pub(crate) fn NativeDatasetInspector(
    selection: NativeDatasetSelection,
    topic: Option<String>,
) -> Element {
    let navigation = use_hook(|| DatasetNavigation::new(topic));
    let available = use_memo(move || ProjectSessionSignals::current().storage_available());
    let mut inspection = crate::client::queries::QueryPolicy::on_demand(
        crate::client::queries::QueryDomain::Catalog,
    )
    .use_when(
        move || *available.read(),
        move || DatasetClient::current().inspect_native_dataset(selection.clone()),
    );
    let response = inspection.read();
    let pending = *inspection.state().read() == UseResourceState::Pending;
    rsx! {
        section { class: "data-inspector", "aria-label": "Dataset inspector",
            if pending { p { class: "data-empty-copy", role: "status", "Loading dataset…" } }
            else { match &*response {
                Some(Ok(detail)) => rsx! { DatasetContents { navigation, inspection: detail.clone() } },
                Some(Err(error)) => rsx! { div { class: "data-empty-copy", role: "alert",
                    p { "{error}" }
                    Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Outline, onclick: move |_| inspection.restart(), "Retry dataset" }
                } },
                None => rsx! { p { class: "data-empty-copy", "Connect to SpiralDB to inspect this dataset." } },
            } }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DatasetSection {
    Episodes,
    Topics,
}

#[derive(Clone, Copy, PartialEq)]
struct DatasetNavigation {
    section: Signal<DatasetSection>,
    episodes: super::native_episodes::EpisodeNavigation,
    topic: Signal<Option<String>>,
    message: Signal<usize>,
}
impl DatasetNavigation {
    fn new(topic: Option<String>) -> Self {
        Self {
            section: Signal::new(if topic.is_some() {
                DatasetSection::Topics
            } else {
                DatasetSection::Episodes
            }),
            episodes: super::native_episodes::EpisodeNavigation::new(),
            topic: Signal::new(topic),
            message: Signal::new(0),
        }
    }
}

#[component]
fn DatasetContents(inspection: NativeDatasetInspection, navigation: DatasetNavigation) -> Element {
    let mut section = navigation.section;
    let selection = inspection.dataset().selection();
    rsx! {
        header { class: "data-inspector-heading",
            h2 { "{selection.label()}" }
            p {
                if let Some(count) = inspection.members() { "{count} episodes · " }
                "{inspection.topics().len()} topics"
            }
        }
        nav { class: "data-section-switcher", "aria-label": "Dataset contents",
            button { "aria-pressed": section() == DatasetSection::Episodes, onclick: move |_| section.set(DatasetSection::Episodes), "Episodes" }
            button { "aria-pressed": section() == DatasetSection::Topics, onclick: move |_| section.set(DatasetSection::Topics), "Topics" }
        }
        if !selection.can_preview() {
            p { class: "data-empty-copy", "Preview is unavailable for this dataset name." }
        } else if section() == DatasetSection::Episodes {
            if inspection.members().is_some() {
                super::native_episodes::NativeEpisodeBrowser { inspection: inspection.clone(), navigation: navigation.episodes, on_topics: move |_| section.set(DatasetSection::Topics) }
            } else {
                p { class: "data-empty-copy", "This dataset has no episode list. Open Topics to explore its contents." }
            }
        } else {
            DatasetTopics { navigation, inspection: inspection.clone() }
        }
    }
}

#[component]
fn DatasetTopics(
    inspection: NativeDatasetInspection,
    mut navigation: DatasetNavigation,
) -> Element {
    let mut selected = navigation.topic;
    rsx! {
        if let Some(topic) = selected() {
            TopicMessages {
                key: "{topic}",
                dataset: inspection.dataset().selection().clone(),
                topic,
                navigation,
            }
        } else {
            p { class: "px-5 py-4 text-xs text-ink-soft", "Choose a topic to preview messages across this dataset." }
            div { class: "min-h-0 flex-1 overflow-auto",
                if inspection.topics().is_empty() { p { class: "data-empty-copy", "No topics in this dataset." } }
                table { class: "data-ledger", "aria-label": "Dataset topics",
                    thead { tr { th { "Topic / schema" } th { class: "text-right", "Episodes" } th { class: "text-right", "Messages" } } }
                    tbody {
                        for topic in inspection.topics() {
                            {
                                let name = topic.name().to_owned();
                                rsx! {
                                    tr { key: "{name}",
                                        td { button { class: "data-topic-action", onclick: move |_| { navigation.message.set(0); selected.set(Some(name.clone())); },
                                            span { class: "block break-all font-medium text-accent", "{topic.name()}" }
                                            span { class: "mt-1 block break-all text-[11px] text-ink-soft", {topic.schemas().join(", ")} }
                                        } }
                                        td { class: "text-right tabular-nums", "{topic.members()}" }
                                        td { class: "text-right tabular-nums", {topic.samples().map(|count| count.to_string()).unwrap_or_else(|| "—".into())} }
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

#[component]
fn TopicMessages(
    dataset: NativeDatasetSelection,
    topic: String,
    mut navigation: DatasetNavigation,
) -> Element {
    let selected_message = navigation.message;
    let mut preview = use_resource(use_reactive(
        (&dataset, &topic),
        move |(dataset, topic)| async move {
            let sql = dataset.topic_sql(&topic);
            let state = match DatasetClient::current()
                .run_spiral_query(sql.clone(), 100)
                .await
            {
                Ok(result) => QueryState::Ready(result),
                Err(error) => QueryState::Failed(error.to_string()),
            };
            QueryExecutionPresentation::new(sql, 100, state)
        },
    ));
    let response = preview.read();
    let pending = *preview.state().read() == UseResourceState::Pending;
    rsx! {
        header { class: "data-content-toolbar",
            button { class: "data-text-action", onclick: move |_| navigation.topic.set(None), "Back to topics" }
            button { class: "data-text-action", disabled: pending, onclick: move |_| preview.restart(), "Refresh preview" }
        }
        div { class: "px-5 pb-4",
            h3 { class: "break-all text-sm font-medium", "{topic}" }
            p { class: "mt-1 text-xs text-ink-soft", "Message preview · up to 100 rows across all episodes" }
        }
        div { class: "flex min-h-0 flex-1 flex-col overflow-hidden",
            if pending { p { class: "data-empty-copy", role: "status", "Loading messages…" } }
            else if let Some(model) = &*response { super::native_messages::TopicMessagePreview { model: model.clone(), selected: selected_message } }
        }
    }
}
