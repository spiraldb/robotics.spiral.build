//! Datasets and their episodes, the way an operator thinks about recordings:
//! a dataset is a folder of MCAP files, and each file is one episode.
use super::{
    DatasetClient,
    episode_list::{
        EPISODE_PAGE_SIZE, EpisodePage, ListedEpisode, OpenedEpisode, SharedSourcePrefix,
    },
};
use crate::components::icons::DataIcon;
use crate::components::interaction::{
    Action, ActionPurpose, ActionSize, ActionVariant, ChevronIcon, RefreshIcon,
};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetInspection, DatasetSelection, ProjectResource};
use std::collections::BTreeMap;

type ResourceCatalog = ReadSignal<Option<Result<Vec<ProjectResource>, String>>>;

/// What the rail reports back to the workspace.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct RailActions {
    on_select_dataset: EventHandler<DatasetSelection>,
    on_open_episode: EventHandler<OpenedEpisode>,
    on_open_table: EventHandler<ProjectResource>,
    on_import: EventHandler<()>,
}

impl RailActions {
    pub(super) fn new(
        on_select_dataset: EventHandler<DatasetSelection>,
        on_open_episode: EventHandler<OpenedEpisode>,
        on_open_table: EventHandler<ProjectResource>,
    ) -> Self {
        Self {
            on_select_dataset,
            on_open_episode,
            on_open_table,
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
    episode_id: Option<String>,
    inspection: Option<DatasetInspection>,
}

impl RailSelection {
    pub(super) fn new(
        dataset: Option<DatasetSelection>,
        episode_id: Option<String>,
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
                input { class: "field data-rail-search", r#type: "search", placeholder: "Filter datasets and episodes",
                    "aria-label": "Filter datasets and episodes", value: filter(), oninput: move |event| filter.set(event.value()) }
                Action { purpose: ActionPurpose::Refresh, variant: ActionVariant::Icon, aria_label: "Refresh catalog", title: "Refresh catalog",
                    onclick: move |_| super::relations::CatalogRefresh::request(), RefreshIcon {} }
                Action { purpose: ActionPurpose::Import, variant: ActionVariant::Outline, size: ActionSize::Compact,
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
                        let others = entries
                            .iter()
                            .filter(|resource| resource.kind() != "dataset")
                            .cloned()
                            .collect::<Vec<_>>();
                        rsx! {
                            if datasets.is_empty() {
                                div { class: "data-rail-note",
                                    strong { "No datasets yet" }
                                    p { "Import MCAP recordings to create the first one." }
                                }
                            }
                            ul { class: "data-dataset-list",
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
                            TableBranches { resources: others, filter: needle, on_open: actions.on_open_table }
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
    selected_episode_id: Option<String>,
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
    rsx! {
        li { class: "data-dataset", "data-selected": selected,
            div { class: "data-dataset-row",
                Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Icon, class: "data-dataset-expand",
                    aria_expanded: open, aria_label: if open { format!("Collapse {name}") } else { format!("Expand {name}") },
                    onclick: move |_| expanded.toggle(), ChevronIcon { open } }
                button { r#type: "button", class: "data-dataset-name", aria_pressed: selected, title: "{name}",
                    onclick: move |_| { actions.on_select_dataset.call(select.clone()); expanded.set(true); },
                    DataIcon {}
                    span { class: "truncate", "{name}" }
                }
                span { class: "data-dataset-count", title: "Episodes",
                    if let Some(members) = members { "{members}" }
                }
            }
            if open {
                DatasetEpisodes { key: "{resource.resource_id()}", dataset: resource.selection().clone(), selected_episode_id, filter, on_open: actions.on_open_episode }
            }
        }
    }
}

#[component]
fn DatasetEpisodes(
    dataset: DatasetSelection,
    selected_episode_id: Option<String>,
    filter: String,
    on_open: EventHandler<OpenedEpisode>,
) -> Element {
    let mut loaded = use_signal(Vec::<ListedEpisode>::new);
    let mut has_more = use_signal(|| false);
    let mut offset = use_signal(|| 0_usize);
    let query_dataset = dataset.clone();
    let mut page = use_resource(move || {
        let dataset = query_dataset.clone();
        let offset = offset();
        async move {
            let snapshot = DatasetClient::current()
                .run_spiral_query(EpisodePage::sql(&dataset, offset), EPISODE_PAGE_SIZE + 1)
                .await
                .map_err(|error| error.to_string())?;
            EpisodePage::from_snapshot(snapshot, offset)
        }
    });
    use_effect(move || {
        if let Some(Ok(page)) = &*page.read() {
            let first = page
                .episodes()
                .first()
                .map_or(usize::MAX, ListedEpisode::ordinal);
            let mut all = loaded.write();
            all.retain(|episode| episode.ordinal() < first);
            all.extend(page.episodes().iter().cloned());
            has_more.set(page.has_more());
        }
    });
    // A search reads every page, so a match on page three is found without a
    // "Load more" click; the pages stay loaded once the field clears.
    let searching = !filter.is_empty();
    use_effect(use_reactive((&searching,), move |(searching,)| {
        let pending = *page.state().read() == UseResourceState::Pending;
        let more = matches!(&*page.read(), Some(Ok(page)) if page.has_more());
        if searching && more && !pending {
            let next = *offset.peek() + EPISODE_PAGE_SIZE;
            offset.set(next);
        }
    }));
    let pending = *page.state().read() == UseResourceState::Pending;
    let episodes = loaded.read();
    let prefix = episodes.shared_source_prefix();
    let rows = episodes
        .iter()
        .filter(|episode| episode.matches(&filter))
        .cloned()
        .collect::<Vec<_>>();
    let unmatched = searching && rows.is_empty() && !episodes.is_empty();
    rsx! {
        ul { class: "data-episode-list", "aria-label": "Episodes of {dataset.label()}",
            for episode in rows {
                {
                    let label = episode.record().source_label(prefix);
                    let selected = selected_episode_id.as_deref() == Some(episode.record().id());
                    let open_dataset = dataset.clone();
                    let open_label = label.clone();
                    let open_episode = episode.clone();
                    rsx! {
                        li { key: "{episode.record().id()}",
                            button { r#type: "button", class: "data-episode-row", aria_pressed: selected,
                                title: "{episode.record().facts_label()}\n{episode.record().source_uri().unwrap_or_default()}",
                                onclick: move |_| on_open.call(OpenedEpisode::new(open_dataset.clone(), open_episode.clone(), open_label.clone())),
                                span { class: "data-episode-ordinal", "{episode.title()}" }
                                span { class: "data-episode-head", "{label.head()}" }
                                span { class: "data-episode-duration", "{episode.record().duration_label()}" }
                                span { class: "data-episode-sub",
                                    if let Some(tail) = label.tail() { span { class: "data-episode-tail", "{tail}" } }
                                    span { "{episode.record().recorded_date_label()}" }
                                    span { "{episode.record().message_count_label()} msgs" }
                                }
                            }
                        }
                    }
                }
            }
            if pending {
                li { class: "data-rail-note", role: "status", if searching { "Searching…" } else { "Loading episodes…" } }
            } else if let Some(Err(error)) = &*page.read() {
                li { class: "data-rail-note", role: "alert",
                    p { "{error}" }
                    Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Link, size: ActionSize::Compact, onclick: move |_| page.restart(), "Retry" }
                }
            } else if episodes.is_empty() {
                li { class: "data-rail-note", "No episodes in this dataset." }
            } else if unmatched && !has_more() {
                li { class: "data-rail-note", "No episodes match." }
            } else if has_more() && !searching {
                li { class: "data-rail-more",
                    Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Link, size: ActionSize::Compact,
                        onclick: move |_| offset.set(offset() + EPISODE_PAGE_SIZE), "Load {EPISODE_PAGE_SIZE} more" }
                }
            }
        }
    }
}

#[component]
fn TableBranches(
    resources: Vec<ProjectResource>,
    filter: String,
    on_open: EventHandler<ProjectResource>,
) -> Element {
    let matching = resources
        .into_iter()
        .filter(|resource| {
            filter.is_empty()
                || resource
                    .selection()
                    .label()
                    .to_lowercase()
                    .contains(&filter)
        })
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return rsx! {};
    }
    // Relations Spiral scopes under a dataset (its topic signals and cohorts)
    // fold under that dataset's name; only project-level tables and views
    // stay at the top, so the list reads as a handful, not a wall.
    let (scoped, top_level): (Vec<_>, Vec<_>) = matching
        .into_iter()
        .partition(|resource| resource.selection().path().len() > 1);
    let mut groups = BTreeMap::<String, Vec<ProjectResource>>::new();
    for resource in scoped {
        groups
            .entry(resource.selection().path()[0].clone())
            .or_default()
            .push(resource);
    }
    let total = top_level.len() + groups.values().map(Vec::len).sum::<usize>();
    rsx! {
        details { class: "data-rail-tables", open: !filter.is_empty(),
            summary { "Tables & views" span { class: "data-rail-tables-count", "{total}" } }
            ul { class: "data-table-list",
                for resource in top_level { TableRow { key: "{resource.resource_id()}", resource, on_open } }
            }
            for (dataset, relations) in groups {
                details { key: "{dataset}", class: "data-rail-tables data-rail-tables-group", open: !filter.is_empty(),
                    summary { "Relations of {dataset}" span { class: "data-rail-tables-count", "{relations.len()}" } }
                    ul { class: "data-table-list",
                        for resource in relations { TableRow { key: "{resource.resource_id()}", resource, on_open } }
                    }
                }
            }
        }
    }
}

#[component]
fn TableRow(resource: ProjectResource, on_open: EventHandler<ProjectResource>) -> Element {
    let label = resource.selection().label();
    // Inside a dataset's group the dataset is already named; the row shows
    // the relation's own name and keeps the full name for SQL in its tooltip.
    let shown = resource
        .selection()
        .path()
        .last()
        .filter(|_| resource.selection().path().len() > 1)
        .cloned()
        .unwrap_or_else(|| label.clone());
    let kind = resource.kind().to_owned();
    let open = resource.clone();
    rsx! {
        li {
            button { r#type: "button", class: "data-table-row", title: "{label} · {kind} · open in SQL",
                onclick: move |_| on_open.call(open.clone()),
                span { class: "truncate", "{shown}" }
                small { "{kind}" }
            }
        }
    }
}
