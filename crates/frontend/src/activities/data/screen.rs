//! Data: datasets and their episodes in a rail, beside tabs of what is open —
//! the chosen dataset at a glance, and one tab per episode opened from it.
//!
//! The rail is the hierarchy: a dataset holds episodes. The Overview tab shows
//! the dataset last chosen; every episode opened gets a tab of its own, so a
//! few recordings can be compared by switching tabs, and each names the
//! dataset it belongs to as the way back to its overview. A view a curation
//! published opens as a tab too, and its intervals open their episodes.
use super::{
    DatasetClient,
    catalog_rail::{CatalogRail, OpenedEpisode, RailActions, RailSelection},
    curated_view::CuratedViewSubject,
    dataset_overview::{DatasetOverview, DatasetsAtAGlance},
};
use crate::activities::curation::{CuratedFills, CuratedView};
use crate::activities::episode::{
    EpisodeFactsPresentation as _, EpisodeHeading, EpisodeInspection, EpisodeListing,
    HeadingSubject,
};
use crate::components::page_keys::PageKeys;
use crate::format::WallClockPresentation as _;
use crate::shell::{
    activity::{Activity, WorkspaceActivity},
    frame::WorkspaceFrame,
};
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant, KindMark};

const LAYOUT_SCOPE: &str = "spiral_robotics.panel-layout.v18.data";
const CATALOG_PANEL: &str = "catalog";
const OVERVIEW_PANEL: &str = "data-overview";
const IMPORT_PANEL: &str = "data-import";
const SUBJECT_TILE: &str = "subject";
/// How often Data asks how far the runs publishing views have got.
const FILL_POLL_MS: u32 = 2_000;

#[component]
pub(crate) fn DataWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = WorkspaceActivity::provide(Activity::Data, is_active);
    rsx! { WorkspaceFrame { workspace, DataPanels {} } }
}

/// What the operator has open: the dataset the Overview shows, and the
/// episodes and curated views open as tabs, in the order they were opened.
/// Owned by the workspace; the rail and the tabs read it and report back.
#[derive(Clone, Copy, PartialEq)]
struct DataSelection {
    dataset: Signal<Option<DatasetSelection>>,
    episodes: Signal<Vec<OpenedEpisode>>,
    views: Signal<Vec<CuratedView>>,
    importing: Signal<bool>,
    active_panel: Signal<Option<PanelId>>,
}

impl DataSelection {
    fn use_selection() -> Self {
        Self {
            dataset: use_signal(|| None),
            episodes: use_signal(Vec::new),
            views: use_signal(Vec::new),
            importing: use_signal(|| false),
            active_panel: use_signal(|| None),
        }
    }
    fn episode_panel(id: &EpisodeId) -> PanelId {
        PanelId::new(format!("episode-{id}"))
    }
    fn view_panel(view: &str) -> PanelId {
        PanelId::new(format!("view-{view}"))
    }
    /// The curated view whose tab is in front, if a view's tab is.
    fn active_view(&self) -> Option<String> {
        let active = self.active_panel.read().clone()?;
        self.views
            .read()
            .iter()
            .find(|view| Self::view_panel(view.view()) == active)
            .map(|view| view.view().to_owned())
    }
    /// A curated view opens in a tab of its own, or brings its tab forward.
    fn open_view(mut self, view: CuratedView) {
        let panel = Self::view_panel(view.view());
        if !self
            .views
            .peek()
            .iter()
            .any(|open| open.view() == view.view())
        {
            self.views.write().push(view);
        }
        self.activate(panel);
    }
    fn activate(mut self, panel: PanelId) {
        self.active_panel.set(Some(panel));
    }
    /// The episode whose tab is in front, if an episode tab is.
    fn active_episode(&self) -> Option<OpenedEpisode> {
        let active = self.active_panel.read().clone()?;
        self.episodes
            .read()
            .iter()
            .find(|opened| Self::episode_panel(opened.episode().id()) == active)
            .cloned()
    }
    /// Choosing a dataset shows it in the Overview tab; open episodes stay.
    fn select_dataset(self, dataset: DatasetSelection) {
        self.show_dataset(dataset);
        self.activate(PanelId::new(OVERVIEW_PANEL));
    }
    /// The Overview shows `dataset`. Showing the one it shows already reads
    /// nothing again: the Overview stays as it is.
    fn show_dataset(mut self, dataset: DatasetSelection) {
        if self.dataset.peek().as_ref() != Some(&dataset) {
            self.dataset.set(Some(dataset));
        }
    }
    /// An episode opens in a tab of its own, or brings its tab forward with
    /// whatever a curated view asks of it.
    fn open_episode(mut self, opened: OpenedEpisode) {
        let panel = Self::episode_panel(opened.episode().id());
        self.show_dataset(opened.episode().dataset().clone());
        let open = self
            .episodes
            .peek()
            .iter()
            .position(|open| open.episode().id() == opened.episode().id());
        match open {
            Some(open) => self.episodes.write()[open].reopen(opened),
            None => self.episodes.write().push(opened),
        }
        self.activate(panel);
    }
    /// Stepping to the next or previous episode moves the tab in front on to
    /// it rather than opening another, so walking a dataset leaves one tab.
    fn step_episode(mut self, opened: OpenedEpisode) {
        let already_open = self
            .episodes
            .peek()
            .iter()
            .any(|open| open.episode().id() == opened.episode().id());
        let current = self
            .active_episode()
            .map(|current| current.episode().id().clone());
        match current.filter(|_| !already_open) {
            Some(current) => {
                let panel = Self::episode_panel(opened.episode().id());
                if let Some(slot) = self
                    .episodes
                    .write()
                    .iter_mut()
                    .find(|open| open.episode().id() == &current)
                {
                    *slot = opened;
                }
                self.activate(panel);
            }
            None => self.open_episode(opened),
        }
    }
    /// A deleted dataset leaves the Overview, and the tabs of its episodes
    /// and of the curated views over it close.
    fn forget_dataset(mut self, dataset: &DatasetSelection) {
        if self.dataset.peek().as_ref() == Some(dataset) {
            self.dataset.set(None);
        }
        self.episodes
            .write()
            .retain(|open| open.episode().dataset() != dataset);
        self.views.write().retain(|view| view.dataset() != dataset);
        self.activate(PanelId::new(OVERVIEW_PANEL));
    }
    fn start_import(mut self) {
        self.importing.set(true);
        self.activate(PanelId::new(IMPORT_PANEL));
    }
    fn close(mut self, panel: &PanelId) {
        if panel.as_str() == IMPORT_PANEL {
            self.importing.set(false);
        }
        self.episodes
            .write()
            .retain(|open| &Self::episode_panel(open.episode().id()) != panel);
        self.views
            .write()
            .retain(|view| &Self::view_panel(view.view()) != panel);
        // Closing the tab in front brings the newest remaining episode
        // forward, or the Overview when none is left.
        if self.active_panel.peek().as_ref() == Some(panel) {
            let next = self
                .episodes
                .peek()
                .last()
                .map(|open| Self::episode_panel(open.episode().id()))
                .unwrap_or_else(|| PanelId::new(OVERVIEW_PANEL));
            self.active_panel.set(Some(next));
        }
    }
    /// The rail beside the tabs; episodes and an import open as tabs beside
    /// the Overview.
    fn layout() -> PanelLayout {
        PanelLayout::new(LayoutNode::split(
            "data-columns",
            SplitAxis::Horizontal,
            0.21,
            LayoutNode::tile(CATALOG_PANEL, [CATALOG_PANEL]),
            LayoutNode::tile(SUBJECT_TILE, [OVERVIEW_PANEL, IMPORT_PANEL]),
        ))
    }
}

/// The runs publishing curated views, as the workspace follows them.
struct ViewFills;

impl ViewFills {
    /// Read the catalog again whenever a run stops filling a view, so a view
    /// it just created is listed and a tab that waited on it reads it.
    fn use_catalog_refresh(fills: Memo<CuratedFills>) {
        let refresh = crate::client::queries::QueryRefresh::current();
        let mut filling = use_signal(std::collections::BTreeSet::<String>::new);
        use_effect(move || {
            let running = fills.read().running();
            let ended = filling.peek().iter().any(|view| !running.contains(view));
            if ended {
                refresh.invalidate(&[crate::client::queries::QueryDomain::Catalog]);
            }
            if *filling.peek() != running {
                filling.set(running);
            }
        });
    }

    /// `opened` as `listed` has it now, or as it was opened while the list
    /// is read again.
    fn current(
        listed: &Option<Result<Vec<CuratedView>, String>>,
        opened: CuratedView,
    ) -> CuratedView {
        listed
            .as_ref()
            .and_then(|read| read.as_ref().ok())
            .and_then(|views| views.iter().find(|view| view.view() == opened.view()))
            .cloned()
            .unwrap_or(opened)
    }
}

#[component]
fn DataPanels() -> Element {
    let project = crate::shell::project::ProjectSelection::current();
    let available = use_memo(move || project.catalog_available());
    let catalog = crate::client::queries::QueryPolicy::on_demand(
        crate::client::queries::QueryDomain::Catalog,
    )
    .use_when(
        move || *available.read(),
        || async {
            DatasetClient::current()
                .datasets()
                .await
                .map_err(|error| error.to_string())
        },
    );
    let datasets = catalog.value();
    // Which of the catalog's views curations published, and from what; read
    // again with the catalog, and once a publication is saved. Beside each,
    // how far the run publishing it has got, read every few seconds.
    let published = crate::client::queries::QueryPolicy::on_demand(
        crate::client::queries::QueryDomain::Catalog,
    )
    .use_when(move || *available.read(), CuratedView::all);
    let fills = crate::client::queries::QueryPolicy::new(
        crate::client::queries::QueryDomain::Catalog,
        FILL_POLL_MS,
    )
    .use_when(move || *available.read(), CuratedFills::read);
    let fills = use_memo(move || {
        fills
            .read()
            .as_ref()
            .and_then(|read| read.as_ref().ok())
            .cloned()
            .unwrap_or_default()
    });
    let curated = use_memo(move || {
        published
            .read()
            .clone()
            .map(|read| read.map(|views| fills.read().applied(&views)))
    });
    ViewFills::use_catalog_refresh(fills);
    let selection = DataSelection::use_selection();
    let inspection = use_resource(move || {
        let dataset = selection.dataset.read().clone();
        async move {
            match dataset {
                Some(dataset) => Some(
                    DatasetClient::current()
                        .inspect_dataset(dataset)
                        .await
                        .map_err(|error| error.to_string()),
                ),
                None => None,
            }
        }
    });
    // While the chosen dataset's definition loads, the previous dataset's
    // streams must not appear under the new name.
    let dataset_inspection = if *inspection.state().read() == UseResourceState::Pending {
        None
    } else {
        inspection.read().clone().flatten()
    };
    let mut imported_dataset = use_signal(|| None::<DatasetSelection>);
    use_effect(move || {
        let Some(imported) = imported_dataset() else {
            return;
        };
        let listed = datasets
            .read()
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|dataset| dataset.selection() == &imported)
            });
        if listed {
            imported_dataset.set(None);
            let mut selection = selection;
            selection.importing.set(false);
            selection.select_dataset(imported);
        }
    });

    // Esc brings the open episode's dataset forward in the Overview tab.
    let activity = WorkspaceActivity::current();
    PageKeys::use_listener(move |event| {
        let back = event.key() == "Escape"
            && !event.default_prevented()
            && activity.is_active()
            && PageKeys::is_shortcut(event);
        match selection.active_episode().filter(|_| back) {
            Some(opened) => {
                selection.select_dataset(opened.episode().dataset().clone());
                true
            }
            None => false,
        }
    });
    let dataset = selection.dataset.read().clone();
    let active_episode = selection.active_episode();
    let rail_actions = RailActions::new(
        EventHandler::new(move |dataset| selection.select_dataset(dataset)),
        EventHandler::new(move |opened| selection.open_episode(opened)),
        EventHandler::new(move |opened| selection.step_episode(opened)),
    )
    .with_import(EventHandler::new(move |()| selection.start_import()))
    .with_curated(EventHandler::new(move |view| selection.open_view(view)));
    // The rail marks what is in front: the episode tab's row, or the
    // Overview's dataset while the Overview is — nothing behind an import.
    let overview_in_front = selection
        .active_panel
        .read()
        .as_ref()
        .is_none_or(|panel| panel.as_str() == OVERVIEW_PANEL);
    let rail_selection = RailSelection::new(
        active_episode
            .as_ref()
            .map(|opened| opened.episode().dataset().clone())
            .or_else(|| dataset.clone().filter(|_| overview_in_front)),
        active_episode
            .as_ref()
            .map(|opened| opened.episode().id().clone()),
        dataset_inspection.clone().and_then(|result| result.ok()),
    )
    .with_view(selection.active_view());

    // The overview lists the dataset's episodes as its format records them,
    // which its inspection says.
    let overview = match dataset {
        Some(_) => match &dataset_inspection {
            Some(Ok(inspection)) => {
                let deleted = inspection.dataset().selection().clone();
                rsx! {
                    DatasetOverview { listing: EpisodeListing::of(inspection.dataset()),
                        on_open: move |checked| selection.open_episode(OpenedEpisode::checked(checked)),
                        on_deleted: move |()| selection.forget_dataset(&deleted) }
                }
            }
            Some(Err(error)) => {
                rsx! { p { class: "data-overview-alert", role: "alert", "{error}" } }
            }
            None => {
                rsx! { p { class: "data-overview-alert", role: "status", "Reading the dataset…" } }
            }
        },
        None => {
            let listings = datasets
                .read()
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .map(|entries| entries.iter().map(EpisodeListing::of).collect::<Vec<_>>())
                .unwrap_or_default();
            rsx! {
                DatasetsAtAGlance { datasets: listings,
                    on_select: move |listing: EpisodeListing| selection.select_dataset(listing.dataset().clone()),
                    on_import: move |()| selection.start_import() }
            }
        }
    };
    let mut panels = vec![
        Panel::new(
            CATALOG_PANEL,
            "Datasets",
            CATALOG_PANEL,
            rsx! {
                CatalogRail { datasets, curated: curated.into(), selection: rail_selection, actions: rail_actions }
            },
        )
        .with_class("data-catalog-panel"),
        Panel::new(OVERVIEW_PANEL, "Overview", SUBJECT_TILE, overview)
            .with_class("data-subject-panel")
            .with_tab_icon(rsx! { KindMark { letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" } }),
    ];
    for opened in selection.episodes.read().iter().cloned() {
        let title = opened
            .label()
            .head()
            .map_or_else(|| opened.episode().name(), str::to_owned);
        let dataset = opened.episode().dataset().clone();
        panels.push(
            Panel::new(
                DataSelection::episode_panel(opened.episode().id()),
                title,
                SUBJECT_TILE,
                rsx! {
                    EpisodeSubject { key: "{opened.episode().id()}", opened, on_dataset: move |_| selection.select_dataset(dataset.clone()) }
                },
            )
            .with_closable(true)
            .with_class("data-subject-panel")
            .with_tab_icon(rsx! { KindMark { letter: 'E', hue: "var(--tone-kind-episode)", label: "Episode" } }),
        );
    }
    for opened in selection.views.read().iter().cloned() {
        // A tab shows its view as the catalog lists it now, its run's
        // progress included.
        let view = ViewFills::current(&curated.read(), opened);
        let dataset = view.dataset().clone();
        let panel = DataSelection::view_panel(view.view());
        panels.push(
            Panel::new(
                panel.clone(),
                view.view().to_owned(),
                SUBJECT_TILE,
                rsx! {
                    CuratedViewSubject { key: "{view.view()}", view,
                        on_dataset: move |_| selection.select_dataset(dataset.clone()),
                        on_open: move |opened| selection.open_episode(opened),
                        on_unpublished: move |_| selection.close(&panel) }
                },
            )
            .with_closable(true)
            .with_class("data-subject-panel")
            .with_tab_icon(rsx! { KindMark { letter: 'C', hue: "var(--tone-kind-curated)", label: "Curated view" } }),
        );
    }
    if (selection.importing)() {
        panels.push(
            Panel::new(IMPORT_PANEL, "Import", SUBJECT_TILE, rsx! {
                super::import_dataset::ImportDataset {
                    on_done: move |dataset| { imported_dataset.set(Some(dataset)); super::relations::CatalogRefresh::request(); },
                }
            })
            .with_closable(true)
            .with_class("data-subject-panel"),
        );
    }
    rsx! {
        div { class: "data-workspace flex-1 min-h-0 min-w-0 overflow-hidden",
            PanelWorkspace {
                panels,
                initial_layout: DataSelection::layout(),
                layout_scope: Some(LAYOUT_SCOPE.to_owned()),
                active_panel: (selection.active_panel)(),
                on_panel_activate: move |id: PanelId| { let mut selection = selection; selection.active_panel.set(Some(id)); },
                on_panel_close: move |id: PanelId| selection.close(&id),
            }
        }
    }
}

/// One episode inspected whole, under the dataset it belongs to.
#[component]
fn EpisodeSubject(opened: OpenedEpisode, on_dataset: EventHandler<()>) -> Element {
    let episode = opened.episode().clone();
    let recorded = episode.facts();
    let mut facts = vec![recorded.duration_label()];
    if recorded
        .message_start_time()
        .is_some_and(|start| start.has_wall_clock())
    {
        facts.push(recorded.recorded_date_label());
    }
    let dataset = episode.dataset().display_name();
    let name = episode.name();
    // Opened from a curated view, the view's intervals ride on the timeline.
    let tracks = opened
        .spans()
        .map(|spans| vec![spans.track().into()])
        .unwrap_or_default();
    rsx! {
        section { class: "data-episode-subject", "aria-label": "{name}",
            EpisodeHeading {
                subject: HeadingSubject::episode(&episode).with_facts(facts),
                lead: rsx! {
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                        class: "data-subject-dataset",
                        title: "Back to {dataset} (Esc)", onclick: move |_| on_dataset.call(()),
                        KindMark { letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" }
                        "{dataset}"
                    }
                    span { class: "data-subject-separator", "aria-hidden": "true", "/" }
                },
                actions: rsx! {
                    p { class: "data-subject-keys", "aria-hidden": "true",
                        kbd { class: "ui-kbd", "J" }
                        kbd { class: "ui-kbd", "K" }
                        span { "step" }
                    }
                },
            }
            EpisodeInspection { episode, tracks, tracks_lead: false, focus: opened.play() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tabs_stand_beside_the_rail_and_an_import_opens_among_them() {
        let layout = DataSelection::layout();
        let subject = layout
            .tile_for_panel(&PanelId::new(OVERVIEW_PANEL))
            .expect("the overview has a tile");
        assert_ne!(
            layout.tile_for_panel(&PanelId::new(CATALOG_PANEL)),
            Some(subject.clone())
        );
        assert_eq!(
            layout.tile_for_panel(&PanelId::new(IMPORT_PANEL)),
            Some(subject)
        );
    }
}
