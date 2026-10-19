//! Datasets and their episodes, the way an operator thinks about recordings:
//! a dataset is a folder of MCAP files, each file one episode, or a LeRobot
//! dataset's episodes. Above them, the views curations published of those
//! episodes.
use super::curated_view::ViewSpans;
use super::dataset_checks::CheckedEpisode;
use crate::activities::curation::{CuratedView, ViewFill};
use crate::activities::episode::{
    Episode, EpisodeFactsPresentation as _, EpisodeListing, EpisodeRows, EpisodeStill,
    EpisodeStills, PlayRequest, RowFaceView, SourceLabel, StillMoment,
};
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::components::page_keys::PageKeys;
use crate::format::GroupedCountPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetSelection, DatasetSummary, EpisodeId,
};
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonSize, ButtonVariant, KindMark, RefreshIcon,
    TextInput, TextInputKind, Tree, TreeBranch, TreeItem,
};

type ListedDatasets = ReadSignal<Option<Result<Vec<DatasetSummary>, String>>>;
pub(super) type CuratedCatalog = ReadSignal<Option<Result<Vec<CuratedView>, String>>>;
const EPISODE_PAGE_SIZE: usize = 50;
const FILTER_ID: &str = "data-rail-filter";

/// The episode the workspace is inspecting, with the label the rail showed,
/// and, opened from a curated view, that view's intervals and the ask to play
/// the one chosen.
#[derive(Clone, PartialEq)]
pub(super) struct OpenedEpisode {
    episode: Episode,
    label: SourceLabel,
    spans: Option<ViewSpans>,
    play: Option<PlayRequest>,
}

impl OpenedEpisode {
    fn new(episode: Episode, label: SourceLabel) -> Self {
        Self {
            episode,
            label,
            spans: None,
            play: None,
        }
    }
    /// `episode` opened from a curated view: its intervals on the timeline,
    /// playing the one chosen.
    pub(super) fn from_view(episode: Episode, label: SourceLabel, spans: ViewSpans) -> Self {
        Self {
            play: spans.chosen().map(PlayRequest::span),
            spans: Some(spans),
            ..Self::new(episode, label)
        }
    }
    /// `checked` opened from the dataset's check report, playing from the
    /// moment its finding names.
    pub(super) fn checked(checked: CheckedEpisode) -> Self {
        Self {
            play: checked.moment().map(PlayRequest::instant),
            ..Self::new(checked.episode().clone(), checked.label().clone())
        }
    }
    pub(super) fn episode(&self) -> &Episode {
        &self.episode
    }
    pub(super) fn label(&self) -> &SourceLabel {
        &self.label
    }
    pub(super) fn spans(&self) -> Option<&ViewSpans> {
        self.spans.as_ref()
    }
    pub(super) fn play(&self) -> Option<PlayRequest> {
        self.play
    }
    /// Take what opening this episode again asks of its tab. A view's
    /// intervals replace the ones shown and play the chosen one, and a
    /// finding plays from its moment, asked anew even when it is the one
    /// already playing; opened again from the rail, the tab stays as it is.
    pub(super) fn reopen(&mut self, again: Self) {
        let Some(spans) = again.spans else {
            if let Some(play) = again.play {
                self.play = Some(play.after(self.play));
            }
            return;
        };
        if let Some(span) = spans.chosen() {
            self.play = Some(PlayRequest::span(span).after(self.play));
        }
        self.spans = Some(spans);
    }
}

/// What the rail reports back to the workspace: a dataset chosen, an episode
/// opened from its row, the next or previous episode stepped to by key, or a
/// curated view chosen.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct RailActions {
    on_select_dataset: EventHandler<DatasetSelection>,
    on_open_episode: EventHandler<OpenedEpisode>,
    on_step_episode: EventHandler<OpenedEpisode>,
    on_import: EventHandler<()>,
    on_open_view: EventHandler<CuratedView>,
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
            on_open_view: EventHandler::new(|_| {}),
        }
    }
    pub(super) fn with_import(mut self, on_import: EventHandler<()>) -> Self {
        self.on_import = on_import;
        self
    }
    pub(super) fn with_curated(mut self, on_open_view: EventHandler<CuratedView>) -> Self {
        self.on_open_view = on_open_view;
        self
    }
}

/// What the workspace currently shows, so the rail can highlight it.
#[derive(Clone, PartialEq)]
pub(super) struct RailSelection {
    dataset: Option<DatasetSelection>,
    episode_id: Option<EpisodeId>,
    inspection: Option<DatasetInspection>,
    view: Option<String>,
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
            view: None,
        }
    }
    /// The curated view whose tab is in front.
    pub(super) fn with_view(mut self, view: Option<String>) -> Self {
        self.view = view;
        self
    }
}

#[component]
pub(super) fn CatalogRail(
    datasets: ListedDatasets,
    curated: CuratedCatalog,
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
    let result = datasets.read();
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
                    title: "Import a dataset from MCAP recordings or Hugging Face", onclick: move |_| actions.on_import.call(()), "Import" }
            }
            div { class: "data-rail-scroll",
                match &*result {
                    None => rsx! { p { class: "data-rail-note", role: "status", "Loading datasets…" } },
                    Some(Err(error)) => rsx! { p { class: "data-rail-note", role: "alert", "{error} Refresh to retry." } },
                    Some(Ok(entries)) => {
                        let needle = filter().to_lowercase();
                        // A search never hides a dataset: each one stays listed and
                        // searches its own episodes, so a match in a collapsed dataset
                        // is found and the operator's expansion choices survive.
                        // Curated views lead, so an open dataset's episodes
                        // never push them out of sight.
                        let curated = match &*curated.read() {
                            Some(Ok(views)) => Ok(ListedViews::of(views, &needle)),
                            Some(Err(error)) => Err(error.clone()),
                            None => Ok(Vec::new()),
                        };
                        let headed = !matches!(&curated, Ok(views) if views.is_empty());
                        rsx! {
                            match curated {
                                Ok(views) => rsx! {
                                    CuratedViewList { views, selected: selection.view.clone(), on_open: actions.on_open_view }
                                },
                                Err(error) => rsx! {
                                    p { class: "data-rail-note", role: "alert", "Curated views could not be read: {error}" }
                                },
                            }
                            if headed {
                                h3 { class: "data-curated-heading", "Datasets" }
                            }
                            if entries.is_empty() {
                                div { class: "data-rail-note",
                                    strong { "No datasets yet" }
                                    p { "Import MCAP recordings to create the first one." }
                                }
                            }
                            Tree { label: "Datasets", class: "data-dataset-list",
                                for summary in entries.iter() {
                                    DatasetBranch {
                                        key: "{summary.resource_id()}",
                                        summary: summary.clone(),
                                        selected: selection.dataset.as_ref() == Some(summary.selection()),
                                        selected_episode_id: selection.episode_id.clone(),
                                        inspection: selection.inspection.clone().filter(|_| selection.dataset.as_ref() == Some(summary.selection())),
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
    summary: DatasetSummary,
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
    let dataset = summary.selection().clone();
    let name = dataset.display_name().to_owned();
    let label = dataset.label();
    // Every dataset says how many episodes it holds, open or not.
    // Read again when the catalog is refreshed, as the episodes are.
    let count_dataset = dataset.clone();
    let counted = QueryPolicy::on_demand(QueryDomain::Catalog).use_resource(move || {
        let dataset = count_dataset.clone();
        async move { Episode::count(&dataset).await }
    });
    let members = inspection
        .as_ref()
        .map(DatasetInspection::members)
        .or_else(|| {
            counted
                .read()
                .as_ref()
                .and_then(|count| count.as_ref().ok().copied())
        });
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
                span { class: "data-dataset-name", title: "{label}", "{name}" }
            },
            trailing: rsx! {
                span { class: "data-dataset-count", title: "Episodes",
                    if let Some(members) = members { "{members.grouped_label()}" }
                }
            },
            DatasetEpisodes { key: "{summary.resource_id()}", listing: EpisodeListing::of(&summary), selected_episode_id, filter, actions }
        }
    }
}

/// The episodes one page at a time, in recording order. Each row leads with
/// what tells it apart — its label, or its id when the path says nothing —
/// and draws its length against the longest episode listed.
#[component]
fn DatasetEpisodes(
    listing: EpisodeListing,
    selected_episode_id: Option<EpisodeId>,
    filter: String,
    actions: RailActions,
) -> Element {
    let on_open = actions.on_open_episode;
    let mut loaded = use_signal(Vec::<Episode>::new);
    let mut has_more = use_signal(|| false);
    let mut offset = use_signal(|| 0_usize);
    let dataset = listing.dataset().clone();
    // Refresh reads the page again along with the catalog, so the rail's
    // Refresh refreshes what the rail shows, not only its datasets. A page
    // is listed as the dataset's format lists episodes, which the catalog
    // says.
    let mut page = QueryPolicy::on_demand(QueryDomain::Catalog).use_resource(move || {
        let listing = listing.clone();
        let offset = offset();
        async move {
            listing
                .page(offset, EPISODE_PAGE_SIZE)
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
    // Stills of the camera that shows the workspace best, as the Dataset
    // node's are; every episode of a dataset records the same cameras.
    let camera = EpisodeStills::use_overview_camera(Some(dataset.clone()));
    let pending = *page.state().read() == UseResourceState::Pending;
    let episodes = loaded.read();
    let listing = EpisodeRows::of(episodes.iter());
    let rows = episodes
        .iter()
        .filter(|episode| episode.matches(&filter))
        .cloned()
        .collect::<Vec<_>>();
    let unmatched = searching && rows.is_empty() && !episodes.is_empty();
    rsx! {
        Fragment {
            // The row Curation's Dataset node lists it with, inside the tree
            // item that makes it one of the rail's rows.
            for episode in rows {
                TreeItem { key: "{episode.id()}", class: "data-episode",
                    selected: selected_episode_id.as_ref() == Some(episode.id()),
                    id: EpisodeStepper::row_id(episode.id()),
                    title: "{episode.facts().facts_label()}\n{episode.facts().source_uri().unwrap_or_default()}",
                    onselect: {
                        let opened = OpenedEpisode::new(episode.clone(), listing.label(&episode));
                        move |_| on_open.call(opened.clone())
                    },
                    label: rsx! {
                        RowFaceView {
                            face: listing.face(&episode),
                            still: rsx! { EpisodeStill { moment: StillMoment::opening(&episode), camera: camera.clone() } },
                        }
                    },
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

/// The curated views the rail lists: those a saved curation records that
/// are published or that a run is publishing, and whose name the filter
/// matches.
struct ListedViews;

impl ListedViews {
    fn of(views: &[CuratedView], needle: &str) -> Vec<CuratedView> {
        views
            .iter()
            .filter(|view| view.fill().is_some() || view.statement().is_some())
            .filter(|view| view.view().to_lowercase().contains(needle))
            .cloned()
            .collect()
    }
}

/// The views curations published, above the datasets their episodes belong
/// to: each by the name it is read by, over the dataset and the output it
/// comes from. One whose models are still answering says so, and opens once
/// they are done.
#[component]
fn CuratedViewList(
    views: Vec<CuratedView>,
    selected: Option<String>,
    on_open: EventHandler<CuratedView>,
) -> Element {
    if views.is_empty() {
        return rsx! {};
    }
    rsx! {
        h3 { class: "data-curated-heading", "Curated" }
        Tree { label: "Curated views", class: "data-curated-list",
            for view in views {
                TreeItem { key: "{view.view()}", class: "data-curated-item",
                    selected: selected.as_deref() == Some(view.view()),
                    title: match view.fill() {
                        Some(fill) if fill.is_running() => format!("Curation in progress: {}", fill.doing()),
                        _ => format!("{} of the curation “{}”", view.output(), view.curation()),
                    },
                    "aria-disabled": view.fill().is_some_and(ViewFill::is_running).then_some("true"),
                    onselect: (!view.fill().is_some_and(ViewFill::is_running)).then(|| {
                        let view = view.clone();
                        EventHandler::new(move |_| on_open.call(view.clone()))
                    }),
                    label: rsx! {
                        KindMark { letter: 'C', hue: "var(--tone-kind-curated)", label: "Curated view" }
                        span { class: "data-curated-item-words",
                            span { class: "data-curated-item-name", "{view.view()}" }
                            span { class: "data-curated-item-origin", "{view.dataset().display_name()} · {view.output()}" }
                        }
                    },
                    trailing: match view.fill() {
                        Some(ViewFill::Running(_)) => Some(rsx! {
                            span { class: "data-curated-item-filling", role: "status", "In progress" }
                        }),
                        Some(ViewFill::Failed(reason)) => Some(rsx! {
                            span { class: "data-curated-item-stale", title: "Its curation failed: {reason}", "Failed" }
                        }),
                        None => (!view.is_current()).then(|| rsx! {
                            span { class: "data-curated-item-stale", title: "Its curation has changed since it was published", "Changed" }
                        }),
                    },
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
            let opened = OpenedEpisode::new(
                (*next).clone(),
                EpisodeRows::of(episodes.iter()).label(next),
            );
            Self::release_rail_focus();
            BrowserFocus::reveal_after_render(Self::row_id(next.id()));
            on_step.call(opened);
            true
        });
    }
}
