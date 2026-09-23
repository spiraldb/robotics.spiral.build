//! Data: datasets and their episodes in a rail, beside the one subject open —
//! a dataset at a glance, or one of its episodes inspected whole.
//!
//! The rail is the hierarchy: a dataset holds episodes. The subject panel
//! shows exactly one level of it at a time, and an open episode names the
//! dataset it belongs to as the way back.
use super::{
    DatasetClient,
    catalog_rail::{CatalogRail, OpenedEpisode, RailActions, RailSelection},
    dataset_overview::DatasetOverview,
};
use crate::activities::episode::{EpisodeFactsPresentation as _, EpisodeInspection};
use crate::shell::{
    activity::{Activity, WorkspaceActivity},
    frame::WorkspaceFrame,
};
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant};

const LAYOUT_SCOPE: &str = "spiral_robotics.panel-layout.v17.data";
const CATALOG_PANEL: &str = "catalog";
const SUBJECT_PANEL: &str = "data-subject";
const IMPORT_PANEL: &str = "data-import";

#[component]
pub(crate) fn DataWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = WorkspaceActivity::provide(Activity::Data, is_active);
    rsx! { WorkspaceFrame { workspace, DataPanels {} } }
}

/// What the operator is looking at: a dataset, and at most one of its
/// episodes. Owned by the workspace; the rail and the subject read it and
/// report back.
#[derive(Clone, Copy, PartialEq)]
struct DataSelection {
    dataset: Signal<Option<DatasetSelection>>,
    episode: Signal<Option<OpenedEpisode>>,
    importing: Signal<bool>,
    active_panel: Signal<Option<PanelId>>,
}

impl DataSelection {
    fn use_selection() -> Self {
        Self {
            dataset: use_signal(|| None),
            episode: use_signal(|| None),
            importing: use_signal(|| false),
            active_panel: use_signal(|| None),
        }
    }
    fn activate(mut self, panel: &str) {
        self.active_panel.set(Some(PanelId::new(panel)));
    }
    /// Choosing a dataset shows it at a glance, closing any episode open.
    fn select_dataset(mut self, dataset: DatasetSelection) {
        self.episode.set(None);
        self.dataset.set(Some(dataset));
        self.activate(SUBJECT_PANEL);
    }
    fn open_episode(mut self, opened: OpenedEpisode) {
        self.dataset.set(Some(opened.episode().dataset().clone()));
        self.episode.set(Some(opened));
        self.activate(SUBJECT_PANEL);
    }
    fn start_import(mut self) {
        self.importing.set(true);
        self.activate(IMPORT_PANEL);
    }
    fn close(mut self, panel: &PanelId) {
        if panel.as_str() == IMPORT_PANEL {
            self.importing.set(false);
        }
        if self.active_panel.peek().as_ref() == Some(panel) {
            self.active_panel.set(None);
        }
    }
    /// The rail beside the subject; an import opens as a tab beside it.
    fn layout() -> PanelLayout {
        PanelLayout::new(LayoutNode::split(
            "data-columns",
            SplitAxis::Horizontal,
            0.21,
            LayoutNode::tile("catalog", [CATALOG_PANEL]),
            LayoutNode::tile("subject", [SUBJECT_PANEL, IMPORT_PANEL]),
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

    let episode = selection.episode.read().clone();
    let dataset = selection.dataset.read().clone();
    let rail_actions = RailActions::new(
        EventHandler::new(move |dataset| selection.select_dataset(dataset)),
        EventHandler::new(move |opened| selection.open_episode(opened)),
    )
    .with_import(EventHandler::new(move |()| selection.start_import()));
    let rail_selection = RailSelection::new(
        dataset.clone(),
        episode
            .as_ref()
            .map(|episode| episode.episode().id().clone()),
        dataset_inspection.clone().and_then(|result| result.ok()),
    );

    let subject_title = if episode.is_some() {
        "Episode"
    } else {
        "Overview"
    };
    let subject = match (episode, dataset) {
        (Some(opened), _) => {
            let dataset = opened.episode().dataset().clone();
            rsx! {
                EpisodeSubject { opened, on_dataset: move |_| selection.select_dataset(dataset.clone()) }
            }
        }
        (None, Some(dataset)) => rsx! {
            DatasetOverview { dataset, inspection: dataset_inspection }
        },
        (None, None) => rsx! {
            WelcomePanel { on_import: move |()| selection.start_import() }
        },
    };
    let mut panels = vec![
        Panel::new(
            CATALOG_PANEL,
            "Datasets",
            "catalog",
            rsx! {
                CatalogRail { resources, selection: rail_selection, actions: rail_actions }
            },
        )
        .with_class("data-catalog-panel"),
        Panel::new(SUBJECT_PANEL, subject_title, "subject", subject)
            .with_class("data-subject-panel"),
    ];
    if (selection.importing)() {
        panels.push(
            Panel::new(IMPORT_PANEL, "Import", "subject", rsx! {
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
    let dataset = episode.dataset().label();
    let name = episode.name();
    rsx! {
        section { class: "data-episode-subject", "aria-label": "{name}",
            header { class: "data-subject-head",
                nav { class: "data-subject-path", "aria-label": "Location",
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                        title: "Show {dataset} at a glance", onclick: move |_| on_dataset.call(()), "{dataset}" }
                    span { class: "data-subject-separator", "aria-hidden": "true", "/" }
                    h2 { "{name}" }
                }
                p { class: "data-subject-facts", title: "{facts.facts_label()}\n{facts.source_uri().unwrap_or_default()}",
                    span { class: "data-subject-label", "{opened.label().head()}" }
                    span { "{facts.duration_label()}" }
                    span { "{facts.recorded_date_label()}" }
                    span { "{facts.message_count_label()} messages" }
                }
            }
            EpisodeInspection { episode }
        }
    }
}

#[component]
fn WelcomePanel(on_import: EventHandler<()>) -> Element {
    rsx! {
        div { class: "data-welcome",
            h2 { "Open your data" }
            p { "A dataset is a collection of MCAP recordings; each file is one episode. Choose a dataset on the left to see its episodes, or import recordings to create a new one." }
            Button { purpose: ActionPurpose::Import, variant: ButtonVariant::Outline, size: ButtonSize::Standard,
                onclick: move |_| on_import.call(()), "Import MCAP recordings" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subject_stands_beside_the_rail_and_an_import_opens_beside_it() {
        let layout = DataSelection::layout();
        let subject = layout
            .tile_for_panel(&PanelId::new(SUBJECT_PANEL))
            .expect("the subject has a tile");
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
