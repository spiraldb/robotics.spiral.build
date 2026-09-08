//! Dataset navigation and inspection share selection, while each mounted panel owns its reads.
use super::{
    DatasetClient,
    query_result::{QueryExecutionPresentation, QueryState},
};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::project::ProjectSessionSignals;
use dioxus::prelude::*;
use spiral_robotics_common::native_datasets::{NativeDatasetInspection, NativeDatasetSelection};

#[derive(Clone, Copy)]
pub(crate) struct DatasetBrowserSelection {
    selected: Signal<Option<NativeDatasetSelection>>,
    revision: Signal<u64>,
    requested_panel: Signal<Option<dioxus_workbench::PanelId>>,
    navigation: DatasetNavigation,
}
impl DatasetBrowserSelection {
    pub(crate) fn provide() -> Self {
        let selected = use_signal(|| None);
        let revision = use_signal(|| 0);
        let requested_panel = use_signal(|| None);
        let navigation = DatasetNavigation::use_navigation();
        use_context_provider(|| Self {
            selected,
            revision,
            requested_panel,
            navigation,
        })
    }
    pub(crate) fn requested_panel(self) -> Option<dioxus_workbench::PanelId> {
        (self.requested_panel)()
    }
    pub(crate) fn clear_request(mut self) {
        self.requested_panel.set(None);
    }
    fn select(mut self, selection: NativeDatasetSelection) {
        if self.selected.peek().as_ref() != Some(&selection) {
            self.navigation.reset();
        }
        self.selected.set(Some(selection));
        self.requested_panel.set(Some("dataset-inspector".into()));
    }
}

#[component]
pub(crate) fn NativeDatasetsPanel() -> Element {
    let mut browser = use_context::<DatasetBrowserSelection>();
    let sessions = ProjectSessionSignals::current();
    let mut search = use_signal(String::new);
    let mut catalog = use_resource(move || async move {
        if !sessions.storage_available() {
            return Ok(Vec::new());
        }
        DatasetClient::current().native_datasets().await
    });
    let catalog_response = catalog.read();
    let pending = *catalog.state().read() == UseResourceState::Pending;
    rsx! {
        section { class: "data-catalog", "aria-label": "Spiral datasets",
            header { class: "data-catalog-toolbar",
                input { class: "field min-w-0 flex-1", r#type: "search", placeholder: "Find a dataset…", "aria-label": "Find a dataset", value: search(), oninput: move |event| search.set(event.value()) }
                Action { purpose: ActionPurpose::Refresh, variant: ActionVariant::Outline, disabled: pending || !sessions.storage_available(), onclick: move |_| { catalog.restart(); browser.revision += 1; }, "Refresh" }
            }
            div { class: "min-h-0 flex-1 overflow-auto",
                if !sessions.storage_available() {
                    p { class: "data-empty-copy", "Connect to SpiralDB in Settings to browse datasets." }
                } else if pending {
                    p { class: "data-empty-copy", role: "status", "Loading datasets…" }
                } else {
                    match &*catalog_response {
                        Some(Err(error)) => rsx! { p { class: "data-empty-copy text-danger", role: "alert", "{error} Use Refresh to retry." } },
                        Some(Ok(datasets)) => {
                            let filter = search().to_lowercase();
                            let matches = datasets.iter().filter(|dataset| dataset.selection().label().to_lowercase().contains(&filter)).collect::<Vec<_>>();
                            rsx! {
                                if datasets.is_empty() { p { class: "data-empty-copy", "No datasets in this project yet." } }
                                else if matches.is_empty() { p { class: "data-empty-copy", "No datasets match your search." } }
                                ul { "aria-label": "Available datasets",
                                    for dataset in matches {
                                        {
                                            let selection = dataset.selection().clone();
                                            let label = selection.label();
                                            let active = browser.selected.read().as_ref() == Some(&selection);
                                            let name = selection.path().last().cloned().unwrap_or_default();
                                            let namespace = selection.path()[..selection.path().len()-1].join(".");
                                            rsx! {
                                                li { key: "{dataset.resource_id()}",
                                                    button { class: "data-dataset-row", "aria-pressed": active, title: "{label}", onclick: move |_| browser.select(selection.clone()),
                                                        span { class: "min-w-0 flex-1",
                                                            span { class: "block break-words font-medium", "{name}" }
                                                            if !namespace.is_empty() { span { class: "mt-1 block text-[11px] text-ink-soft", "{namespace}" } }
                                                        }
                                                        svg { class: "ui-icon shrink-0", view_box: "0 0 16 16", "aria-hidden": "true", path { d: "m6 4 4 4-4 4" } }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        None => rsx! {},
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn NativeDatasetInspector() -> Element {
    let browser = use_context::<DatasetBrowserSelection>();
    let mut inspection = use_resource(move || {
        let selection = (browser.selected)();
        let _revision = (browser.revision)();
        async move {
            match selection {
                Some(selection) => DatasetClient::current()
                    .inspect_native_dataset(selection)
                    .await
                    .map(Some),
                None => Ok(None),
            }
        }
    });
    let inspection_response = inspection.read();
    let pending = *inspection.state().read() == UseResourceState::Pending;
    rsx! {
        section { class: "data-inspector", "aria-label": "Dataset inspector",
            if browser.selected.read().is_none() {
                div { class: "data-inspector-welcome",
                    h2 { "Explore your recordings" }
                    p { "Choose a dataset on the left to browse its episodes and preview topic messages." }
                    p { class: "text-[11px]", "A dataset groups recordings. Each episode is one recording; topics group the messages inside them." }
                }
            } else if pending {
                p { class: "data-empty-copy", role: "status", "Loading dataset…" }
            } else {
                match &*inspection_response {
                    Some(Ok(Some(detail))) => rsx! { DatasetContents { key: "{detail.dataset().resource_id()}-{detail.dataset().version_id()}", inspection: detail.clone() } },
                    Some(Err(error)) => rsx! {
                        div { class: "data-empty-copy", role: "alert",
                            p { class: "mb-3 text-danger", "{error}" }
                            Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Outline, onclick: move |_| inspection.restart(), "Retry dataset" }
                        }
                    },
                    _ => rsx! {},
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DatasetSection {
    Episodes,
    Topics,
}

#[derive(Clone, Copy)]
struct DatasetNavigation {
    section: Signal<DatasetSection>,
    episodes: super::native_episodes::EpisodeNavigation,
    topic: Signal<Option<String>>,
    message: Signal<usize>,
}
impl DatasetNavigation {
    fn use_navigation() -> Self {
        Self {
            section: use_signal(|| DatasetSection::Episodes),
            episodes: super::native_episodes::EpisodeNavigation::use_navigation(),
            topic: use_signal(|| None),
            message: use_signal(|| 0),
        }
    }
    fn reset(mut self) {
        self.section.set(DatasetSection::Episodes);
        self.episodes.reset();
        self.topic.set(None);
        self.message.set(0);
    }
}

#[component]
fn DatasetContents(inspection: NativeDatasetInspection) -> Element {
    let navigation = use_context::<DatasetBrowserSelection>().navigation;
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
            DatasetTopics { inspection: inspection.clone() }
        }
    }
}

#[component]
fn DatasetTopics(inspection: NativeDatasetInspection) -> Element {
    let mut navigation = use_context::<DatasetBrowserSelection>().navigation;
    let mut selected = navigation.topic;
    rsx! {
        if let Some(topic) = selected() {
            TopicMessages {
                key: "{topic}",
                dataset: inspection.dataset().selection().clone(),
                topic,
                on_back: move |_| selected.set(None),
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
fn TopicMessages(dataset: NativeDatasetSelection, topic: String, on_back: EventHandler) -> Element {
    let selected_message = use_context::<DatasetBrowserSelection>().navigation.message;
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
            button { class: "data-text-action", onclick: move |_| on_back.call(()), "Back to topics" }
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

#[cfg(test)]
mod navigation_tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct NavigationHarness(Rc<RefCell<Option<DatasetBrowserSelection>>>);
    impl NavigationHarness {
        fn render(self) -> Element {
            *self.0.borrow_mut() = Some(DatasetBrowserSelection::provide());
            rsx! {}
        }
    }

    #[test]
    fn revisiting_a_dataset_preserves_location_but_switching_datasets_resets_it() {
        let harness = NavigationHarness::default();
        let mut dom = VirtualDom::new_with_props(NavigationHarness::render, harness.clone());
        dom.rebuild_in_place();
        let browser = harness.0.borrow().unwrap();
        dom.in_runtime(|| {
            let dataset = NativeDatasetSelection::new(vec!["recordings".into()]).unwrap();
            browser.select(dataset.clone());
            let mut navigation = browser.navigation;
            navigation.section.set(DatasetSection::Topics);
            navigation.topic.set(Some("/joints".into()));
            navigation.message.set(7);
            browser.clear_request();
            browser.select(dataset);
            assert!(matches!((navigation.section)(), DatasetSection::Topics));
            assert_eq!((navigation.topic)().as_deref(), Some("/joints"));
            assert_eq!((navigation.message)(), 7);
            browser.select(NativeDatasetSelection::new(vec!["other".into()]).unwrap());
            assert!(matches!((navigation.section)(), DatasetSection::Episodes));
            assert!((navigation.topic)().is_none());
            assert_eq!((navigation.message)(), 0);
        });
    }
}
