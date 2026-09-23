//! Data: datasets and their episodes in a rail, beside tabs of what is open —
//! the chosen dataset at a glance, and one tab per episode opened from it.
//!
//! The rail is the hierarchy: a dataset holds episodes. The Overview tab shows
//! the dataset last chosen; every episode opened gets a tab of its own, so a
//! few recordings can be compared by switching tabs, and each names the
//! dataset it belongs to as the way back to its overview.
use super::{
    DatasetClient,
    catalog_rail::{CatalogRail, OpenedEpisode, RailActions, RailSelection},
    dataset_overview::{DatasetOverview, DatasetsAtAGlance},
};
use crate::activities::episode::{EpisodeFactsPresentation as _, EpisodeInspection};
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

#[component]
pub(crate) fn DataWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = WorkspaceActivity::provide(Activity::Data, is_active);
    rsx! { WorkspaceFrame { workspace, DataPanels {} } }
}

/// What the operator has open: the dataset the Overview shows, and the
/// episodes open as tabs, in the order they were opened. Owned by the
/// workspace; the rail and the tabs read it and report back.
#[derive(Clone, Copy, PartialEq)]
struct DataSelection {
    dataset: Signal<Option<DatasetSelection>>,
    episodes: Signal<Vec<OpenedEpisode>>,
    importing: Signal<bool>,
    active_panel: Signal<Option<PanelId>>,
}

impl DataSelection {
    fn use_selection() -> Self {
        Self {
            dataset: use_signal(|| None),
            episodes: use_signal(Vec::new),
            importing: use_signal(|| false),
            active_panel: use_signal(|| None),
        }
    }
    fn episode_panel(id: &EpisodeId) -> PanelId {
        PanelId::new(format!("episode-{id}"))
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
    fn select_dataset(mut self, dataset: DatasetSelection) {
        self.dataset.set(Some(dataset));
        self.activate(PanelId::new(OVERVIEW_PANEL));
    }
    /// An episode opens in a tab of its own, or brings its tab forward.
    fn open_episode(mut self, opened: OpenedEpisode) {
        let panel = Self::episode_panel(opened.episode().id());
        self.dataset.set(Some(opened.episode().dataset().clone()));
        if !self
            .episodes
            .peek()
            .iter()
            .any(|open| open.episode().id() == opened.episode().id())
        {
            self.episodes.write().push(opened);
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
                .project_resources()
                .await
                .map_err(|error| error.to_string())
        },
    );
    let resources = catalog.value();
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
    let mut imported_dataset = use_signal(|| None::<String>);
    use_effect(move || {
        let Some(name) = imported_dataset() else {
            return;
        };
        let resource = resources
            .read()
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|resource| resource.selection().label() == name)
            })
            .cloned();
        if let Some(resource) = resource {
            imported_dataset.set(None);
            let mut selection = selection;
            selection.importing.set(false);
            selection.select_dataset(resource.selection().clone());
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
    .with_import(EventHandler::new(move |()| selection.start_import()));
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
    );

    let overview = match dataset {
        Some(dataset) => rsx! { DatasetOverview { dataset, inspection: dataset_inspection } },
        None => {
            let datasets = resources
                .read()
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|resource| resource.kind() == "dataset")
                        .map(|resource| resource.selection().clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            rsx! {
                DatasetsAtAGlance { datasets,
                    on_select: move |dataset| selection.select_dataset(dataset),
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
                CatalogRail { resources, selection: rail_selection, actions: rail_actions }
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
    if (selection.importing)() {
        panels.push(
            Panel::new(IMPORT_PANEL, "Import", SUBJECT_TILE, rsx! {
                super::import_dataset::ImportDataset {
                    on_done: move |name| { imported_dataset.set(Some(name)); super::relations::CatalogRefresh::request(); },
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
    let facts = episode.facts().clone();
    let dataset = episode.dataset().display_name();
    let name = episode.name();
    rsx! {
        section { class: "data-episode-subject", "aria-label": "{name}",
            header { class: "data-subject-head",
                nav { class: "data-subject-path", "aria-label": "Location",
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                        class: "data-subject-dataset",
                        title: "Back to {dataset} (Esc)", onclick: move |_| on_dataset.call(()),
                        KindMark { letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" }
                        "{dataset}"
                    }
                    span { class: "data-subject-separator", "aria-hidden": "true", "/" }
                    h2 { "{name}" }
                }
                p { class: "data-subject-facts", title: "{facts.facts_label()}\n{facts.source_uri().unwrap_or_default()}",
                    span { "{facts.duration_label()}" }
                    if facts.message_start_time().is_some_and(|start| start.has_wall_clock()) {
                        span { "{facts.recorded_date_label()}" }
                    }
                }
                p { class: "data-subject-keys", "aria-hidden": "true",
                    kbd { class: "ui-kbd", "J" }
                    kbd { class: "ui-kbd", "K" }
                    span { "step" }
                }
            }
            EpisodeInspection { episode }
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
