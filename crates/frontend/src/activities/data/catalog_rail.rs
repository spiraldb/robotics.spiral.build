//! Datasets and their episodes, the way an operator thinks about recordings:
//! a dataset is a folder of MCAP files, and each file is one episode.
use crate::activities::episode::{
    Episode, EpisodeFactsPresentation as _, EpisodeLabels, SourceLabel,
};
use crate::components::page_keys::PageKeys;
use crate::format::{GroupedCountPresentation as _, WallClockPresentation as _};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetSelection, EpisodeId, ProjectResource,
};
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonSize, ButtonVariant, KindMark, RefreshIcon,
    TextInput, TextInputKind, Tree, TreeBranch, TreeItem,
};

type ResourceCatalog = ReadSignal<Option<Result<Vec<ProjectResource>, String>>>;
const EPISODE_PAGE_SIZE: usize = 50;
const FILTER_ID: &str = "data-rail-filter";

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

/// What the rail reports back to the workspace: a dataset chosen, an episode
/// opened from its row, or the next or previous episode stepped to by key.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct RailActions {
    on_select_dataset: EventHandler<DatasetSelection>,
    on_open_episode: EventHandler<OpenedEpisode>,
    on_step_episode: EventHandler<OpenedEpisode>,
    on_import: EventHandler<()>,
}

impl RailActions {
    pub(super) fn new(
        on_select_dataset: EventHandler<DatasetSelection>,
        on_open_episode: EventHandler<OpenedEpisode>,
        on_step_episode: EventHandler<OpenedEpisode>,
    ) -> Self {
        Self {
            on_select_dataset,
            on_open_episode,
            on_step_episode,
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
    let activity = crate::shell::activity::WorkspaceActivity::current();
    PageKeys::use_listener(move |event| {
        let claimed = activity.is_active() && PageKeys::is_shortcut(event) && event.key() == "/";
        if claimed {
            BrowserFocus::select_after_render(FILTER_ID.to_owned());
        }
        claimed
    });
    let result = resources.read();
    rsx! {
        section { class: "data-rail", "aria-label": "Datasets",
            header { class: "data-rail-head",
                div { class: "data-rail-search",
                    TextInput { id: FILTER_ID, kind: TextInputKind::Search, placeholder: "Filter",
                        aria_label: "Filter datasets and episodes", value: filter(), oninput: move |value| filter.set(value),
                        onkeydown: move |event: KeyboardEvent| if event.key() == Key::Escape { filter.set(String::new()) } }
                    if filter().is_empty() { kbd { class: "ui-kbd", "aria-hidden": "true", "/" } }
                }
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

/// A dataset row. It is the selected row only while the dataset itself is the
/// subject; with one of its episodes open it steps back to a bold name, so
/// exactly one row in the rail wears the lime.
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
    // Chosen from anywhere — its row, the datasets ledger, Esc from an
    // episode — the dataset opens to show its episodes.
    use_effect(use_reactive((&selected,), move |(selected,)| {
        if selected {
            expanded.set(true);
        }
    }));
    let dataset = resource.selection().clone();
    let name = dataset.display_name();
    let sql_name = dataset.sql_name();
    // Every dataset says how many episodes it holds, open or not.
    let counted = use_resource(use_reactive((&dataset,), |(dataset,)| async move {
        Episode::count(&dataset).await.ok()
    }));
    let members = inspection
        .as_ref()
        .and_then(DatasetInspection::members)
        .or_else(|| counted.read().flatten());
    let select = dataset.clone();
    // A search opens every dataset so its matches show; the operator's own
    // expansion choice returns when the field clears.
    let open = expanded() || !filter.is_empty();
    let branch = TreeBranch::new(name.clone(), open, move |next| expanded.set(next));
    let holds_open_episode = selected && selected_episode_id.is_some();
    rsx! {
        TreeItem { class: "data-dataset", branch, selected: selected && !holds_open_episode,
            "data-current": holds_open_episode,
            onselect: move |_| { actions.on_select_dataset.call(select.clone()); expanded.set(true); },
            label: rsx! {
                KindMark { letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" }
                span { class: "data-dataset-name", title: "{sql_name}", "{name}" }
            },
            trailing: rsx! {
                span { class: "data-dataset-count", title: "Episodes",
                    if let Some(members) = members { "{members.grouped_label()}" }
                }
            },
            DatasetEpisodes { key: "{resource.resource_id()}", dataset, selected_episode_id, filter, actions }
        }
    }
}

/// The episodes one page at a time, in recording order. Each row leads with
/// what tells it apart — its label, or its id when the path says nothing —
/// and draws its length against the longest episode listed.
#[component]
fn DatasetEpisodes(
    dataset: DatasetSelection,
    selected_episode_id: Option<EpisodeId>,
    filter: String,
    actions: RailActions,
) -> Element {
    let on_open = actions.on_open_episode;
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
    EpisodeStepper::use_keys(
        loaded,
        selected_episode_id.clone(),
        filter.clone(),
        actions.on_step_episode,
    );
    let pending = *page.state().read() == UseResourceState::Pending;
    let episodes = loaded.read();
    let labels = EpisodeLabels::of(episodes.iter().map(Episode::facts));
    let longest = episodes
        .iter()
        .filter_map(|episode| episode.facts().duration_seconds())
        .fold(0.0_f64, f64::max);
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
                    // On a square-root scale, so one runaway take does not
                    // flatten every other bar into a dot.
                    let share = facts
                        .duration_seconds()
                        .filter(|_| longest > 0.0)
                        .map_or(0.0, |seconds| (seconds / longest).sqrt() * 100.0);
                    let opened = OpenedEpisode { episode: episode.clone(), label: label.clone() };
                    rsx! {
                        TreeItem { key: "{episode.id()}", class: "data-episode", selected,
                            id: EpisodeStepper::row_id(episode.id()),
                            "data-labelled": label.head().is_some(),
                            title: "{facts.facts_label()}\n{facts.source_uri().unwrap_or_default()}",
                            onselect: move |_| on_open.call(opened.clone()),
                            // A label gets the whole first line; the id, date,
                            // length, and duration share the second. A row
                            // without a label is one line led by its id.
                            label: rsx! {
                                span { class: "data-episode-body",
                                    match label.head() {
                                        Some(head) => rsx! {
                                            span { class: "data-episode-head", "{head}" }
                                            span { class: "data-episode-sub",
                                                span { class: "data-episode-id", title: "{episode.id()}", "{Episode::listed_name_of(episode.id())}" }
                                                if facts.message_start_time().is_some_and(|start| start.has_wall_clock()) {
                                                    span { "{facts.recorded_date_label()}" }
                                                }
                                            }
                                        },
                                        None => rsx! {
                                            span { class: "data-episode-head data-episode-id", title: "{episode.id()}", "{Episode::listed_name_of(episode.id())}" }
                                        },
                                    }
                                    span { class: "data-episode-length", "aria-hidden": "true",
                                        i { style: "width: {share:.1}%" }
                                    }
                                    span { class: "data-episode-duration", "{facts.duration_label()}" }
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

/// J and K step to the next and previous episode of the dataset whose
/// episode is in front, among the ones the filter lets through.
struct EpisodeStepper;

impl EpisodeStepper {
    fn row_id(id: &EpisodeId) -> String {
        format!("data-episode-{id}")
    }

    /// A row the pointer last pressed keeps focus, and its ring would stay
    /// behind on the old episode; stepping hands focus back to the page, so
    /// Space and the arrows drive the newly opened episode's clock.
    fn release_rail_focus() {
        use wasm_bindgen::JsCast as _;
        let focused = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.active_element())
            .filter(|element| element.closest(".data-rail").ok().flatten().is_some())
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok());
        if let Some(element) = focused {
            let _ = element.blur();
        }
    }

    fn use_keys(
        loaded: Signal<Vec<Episode>>,
        selected: Option<EpisodeId>,
        filter: String,
        on_step: EventHandler<OpenedEpisode>,
    ) {
        let activity = crate::shell::activity::WorkspaceActivity::current();
        let mut current = use_signal(|| (selected.clone(), filter.clone()));
        use_effect(use_reactive(
            (&selected, &filter),
            move |(selected, filter)| {
                current.set((selected, filter));
            },
        ));
        PageKeys::use_listener(move |event| {
            let step = match event.key().as_str() {
                "j" | "J" => 1,
                "k" | "K" => -1,
                _ => return false,
            };
            if !activity.is_active() || !PageKeys::is_shortcut(event) {
                return false;
            }
            let (Some(selected), filter) = current.peek().clone() else {
                return false;
            };
            let episodes = loaded.peek();
            let rows = episodes
                .iter()
                .filter(|episode| episode.matches(&filter))
                .collect::<Vec<_>>();
            let Some(position) = rows.iter().position(|episode| episode.id() == &selected) else {
                return false;
            };
            let Some(next) = position
                .checked_add_signed(step)
                .and_then(|index| rows.get(index))
            else {
                return true;
            };
            let labels = EpisodeLabels::of(episodes.iter().map(Episode::facts));
            let opened = OpenedEpisode {
                episode: (*next).clone(),
                label: labels.label(next.facts()),
            };
            Self::release_rail_focus();
            BrowserFocus::reveal_after_render(Self::row_id(next.id()));
            on_step.call(opened);
            true
        });
    }
}
