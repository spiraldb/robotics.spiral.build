//! Datasets and their episodes, the way an operator thinks about recordings:
//! a dataset is a folder of MCAP files, and each file is one episode.
use crate::activities::episode::{
    Episode, EpisodeFactsPresentation as _, EpisodeLabels, SourceLabel,
};
use crate::components::icons::DataIcon;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetSelection, EpisodeId, ProjectResource,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, RefreshIcon, TextInput, TextInputKind, Tree,
    TreeBranch, TreeItem,
};

type ResourceCatalog = ReadSignal<Option<Result<Vec<ProjectResource>, String>>>;
const EPISODE_PAGE_SIZE: usize = 50;

/// The episode the workspace is inspecting, with the label the rail showed.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OpenedEpisode {
    episode: Episode,
    label: SourceLabel,
}

impl OpenedEpisode {
    pub(super) fn episode(&self) -> &Episode {
        &self.episode
    }
    pub(super) fn label(&self) -> &SourceLabel {
        &self.label
    }
}

/// What the rail reports back to the workspace.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct RailActions {
    on_select_dataset: EventHandler<DatasetSelection>,
    on_open_episode: EventHandler<OpenedEpisode>,
    on_import: EventHandler<()>,
}

impl RailActions {
    pub(super) fn new(
        on_select_dataset: EventHandler<DatasetSelection>,
        on_open_episode: EventHandler<OpenedEpisode>,
    ) -> Self {
        Self {
            on_select_dataset,
            on_open_episode,
            on_import: EventHandler::new(|_| {}),
        }
    }
    pub(super) fn with_import(mut self, on_import: EventHandler<()>) -> Self {
        self.on_import = on_import;
        self
    }
}

/// What the workspace currently shows, so the rail can highlight it.
#[derive(Clone, PartialEq)]
pub(super) struct RailSelection {
    dataset: Option<DatasetSelection>,
    episode_id: Option<EpisodeId>,
    inspection: Option<DatasetInspection>,
}

impl RailSelection {
    pub(super) fn new(
        dataset: Option<DatasetSelection>,
        episode_id: Option<EpisodeId>,
        inspection: Option<DatasetInspection>,
    ) -> Self {
        Self {
            dataset,
            episode_id,
            inspection,
        }
    }
}

#[component]
pub(super) fn CatalogRail(
    resources: ResourceCatalog,
    selection: RailSelection,
    actions: RailActions,
) -> Element {
    let mut filter = use_signal(String::new);
    let result = resources.read();
    rsx! {
        section { class: "data-rail", "aria-label": "Datasets",
            header { class: "data-rail-head",
                TextInput { class: "data-rail-search", kind: TextInputKind::Search, placeholder: "Filter datasets and episodes",
                    aria_label: "Filter datasets and episodes", value: filter(), oninput: move |value| filter.set(value) }
                Button { purpose: ActionPurpose::Refresh, variant: ButtonVariant::Icon, aria_label: "Refresh catalog", title: "Refresh catalog",
                    onclick: move |_| super::relations::CatalogRefresh::request(), RefreshIcon {} }
                Button { purpose: ActionPurpose::Import, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                    title: "Import MCAP recordings as a new dataset", onclick: move |_| actions.on_import.call(()), "Import" }
            }
            div { class: "data-rail-scroll",
                match &*result {
                    None => rsx! { p { class: "data-rail-note", role: "status", "Reading SpiralDB catalog…" } },
                    Some(Err(error)) => rsx! { p { class: "data-rail-note", role: "alert", "{error} Refresh to retry." } },
                    Some(Ok(entries)) => {
                        let needle = filter().to_lowercase();
                        // A search never hides a dataset: each one stays listed and
                        // searches its own episodes, so a match in a collapsed dataset
                        // is found and the operator's expansion choices survive.
                        let datasets = entries.iter().filter(|resource| resource.kind() == "dataset").cloned().collect::<Vec<_>>();
                        rsx! {
                            if datasets.is_empty() {
                                div { class: "data-rail-note",
                                    strong { "No datasets yet" }
                                    p { "Import MCAP recordings to create the first one." }
                                }
                            }
                            Tree { label: "Datasets", class: "data-dataset-list",
                                for resource in datasets.iter() {
                                    DatasetBranch {
                                        key: "{resource.resource_id()}",
                                        resource: resource.clone(),
                                        selected: selection.dataset.as_ref() == Some(resource.selection()),
                                        selected_episode_id: selection.episode_id.clone(),
                                        inspection: selection.inspection.clone().filter(|_| selection.dataset.as_ref() == Some(resource.selection())),
                                        filter: needle.clone(),
                                        actions,
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
fn DatasetBranch(
    resource: ProjectResource,
    selected: bool,
    selected_episode_id: Option<EpisodeId>,
    inspection: Option<DatasetInspection>,
    filter: String,
    actions: RailActions,
) -> Element {
    let mut expanded = use_signal(|| false);
    use_effect(move || {
        if selected {
            expanded.set(true);
        }
    });
    let name = resource.selection().label();
    let members = inspection.as_ref().and_then(DatasetInspection::members);
    let select = resource.selection().clone();
    // A search opens every dataset so its matches show; the operator's own
    // expansion choice returns when the field clears.
    let open = expanded() || !filter.is_empty();
    let branch = TreeBranch::new(name.clone(), open, move |next| expanded.set(next));
    rsx! {
        TreeItem { class: "data-dataset", branch, selected,
            onselect: move |_| { actions.on_select_dataset.call(select.clone()); expanded.set(true); },
            label: rsx! {
                DataIcon {}
                span { class: "truncate", title: "{name}", "{name}" }
            },
            trailing: rsx! {
                span { class: "data-dataset-count", title: "Episodes",
                    if let Some(members) = members { "{members}" }
                }
            },
            DatasetEpisodes { key: "{resource.resource_id()}", dataset: resource.selection().clone(), selected_episode_id, filter, on_open: actions.on_open_episode }
        }
    }
}

#[component]
fn DatasetEpisodes(
    dataset: DatasetSelection,
    selected_episode_id: Option<EpisodeId>,
    filter: String,
    on_open: EventHandler<OpenedEpisode>,
) -> Element {
    let mut loaded = use_signal(Vec::<Episode>::new);
    let mut has_more = use_signal(|| false);
    let mut offset = use_signal(|| 0_usize);
    let query_dataset = dataset.clone();
    let mut page = use_resource(move || {
        let dataset = query_dataset.clone();
        let offset = offset();
        async move {
            Episode::page(&dataset, offset, EPISODE_PAGE_SIZE)
                .await
                .map(|page| (offset, page))
        }
    });
    // A page replaces whatever was loaded from its offset on.
    use_effect(move || {
        if let Some(Ok((offset, page))) = &*page.read() {
            let mut all = loaded.write();
            all.truncate(*offset);
            all.extend(page.episodes().iter().cloned());
            has_more.set(page.has_more());
        }
    });
    // A search reads every page, so a match on page three is found without a
    // "Load more" click; the pages stay loaded once the field clears.
    let searching = !filter.is_empty();
    use_effect(use_reactive((&searching,), move |(searching,)| {
        let pending = *page.state().read() == UseResourceState::Pending;
        let more = matches!(&*page.read(), Some(Ok((_, page))) if page.has_more());
        if searching && more && !pending {
            let next = *offset.peek() + EPISODE_PAGE_SIZE;
            offset.set(next);
        }
    }));
    let pending = *page.state().read() == UseResourceState::Pending;
    let episodes = loaded.read();
    let labels = EpisodeLabels::of(episodes.iter().map(Episode::facts));
    let rows = episodes
        .iter()
        .filter(|episode| episode.matches(&filter))
        .cloned()
        .collect::<Vec<_>>();
    let unmatched = searching && rows.is_empty() && !episodes.is_empty();
    rsx! {
        Fragment {
            for episode in rows {
                {
                    let label = labels.label(episode.facts());
                    let selected = selected_episode_id.as_ref() == Some(episode.id());
                    let facts = episode.facts().clone();
                    let opened = OpenedEpisode { episode: episode.clone(), label: label.clone() };
                    rsx! {
                        TreeItem { key: "{episode.id()}", class: "data-episode", selected,
                            title: "{facts.facts_label()}\n{facts.source_uri().unwrap_or_default()}",
                            onselect: move |_| on_open.call(opened.clone()),
                            label: rsx! {
                                span { class: "data-episode-id", title: "{episode.id()}", "…{episode.id().short()}" }
                                span { class: "data-episode-head", "{label.head()}" }
                                span { class: "data-episode-duration", "{facts.duration_label()}" }
                                span { class: "data-episode-sub",
                                    if let Some(tail) = label.tail() { span { class: "data-episode-tail", "{tail}" } }
                                    span { "{facts.recorded_date_label()}" }
                                    span { "{facts.message_count_label()} msgs" }
                                }
                            },
                        }
                    }
                }
            }
            if pending {
                li { class: "data-rail-note", role: "status", if searching { "Searching…" } else { "Loading episodes…" } }
            } else if let Some(Err(error)) = &*page.read() {
                li { class: "data-rail-note", role: "alert",
                    p { "{error}" }
                    Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Link, size: ButtonSize::Compact, onclick: move |_| page.restart(), "Retry" }
                }
            } else if episodes.is_empty() {
                li { class: "data-rail-note", "No episodes in this dataset." }
            } else if unmatched && !has_more() {
                li { class: "data-rail-note", "No episodes match." }
            } else if has_more() && !searching {
                li { class: "data-rail-more",
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                        onclick: move |_| offset.set(offset() + EPISODE_PAGE_SIZE), "Load {EPISODE_PAGE_SIZE} more" }
                }
            }
        }
    }
}
