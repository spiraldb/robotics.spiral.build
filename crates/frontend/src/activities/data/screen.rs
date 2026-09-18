//! Data: datasets and their episodes in a rail; one open episode across
//! cameras, plots, and messages on one timeline; read-only SQL beside them.
use super::{
    DatasetClient,
    catalog_rail::{CatalogRail, RailActions, RailSelection},
    dataset_overview::{DatasetOverview, OverviewActions},
    episode_list::OpenedEpisode,
    episode_playback::EpisodeCamera,
    episode_streams::{EpisodeStreams, StreamCatalog, StreamChoices},
    episode_transport::{EpisodeTimeRange, EpisodeTransport, KeyboardFocus},
    messages_panel::MessagesPanel,
    plots_panel::PlotsPanel,
    sql_panel::SqlPanel,
    state::QueryController,
    timeline_panel::{TimelinePanel, TimelineSubject},
};
use crate::components::interaction::{Action, ActionPurpose, ActionSize, ActionVariant};
use crate::panels::camera_panel::CameraPanelRows;
use crate::shell::frame::WorkspaceFrame;
use dioxus::prelude::*;
use dioxus_workbench::{
    DockZone, LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis,
};
use spiral_robotics_common::datasets::{DatasetSelection, ProjectResource};

const LAYOUT_SCOPE: &str = "spiral_robotics.panel-layout.v13.data";
const IMPORT_PANEL: &str = "data-import";

#[component]
pub(crate) fn DataWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Data,
        is_active,
    );
    rsx! { WorkspaceFrame { workspace, DataPanels {} } }
}

/// What the operator is looking at. Owned by the workspace; the rail, the
/// overview, and the episode panels all read from here and report back.
#[derive(Clone, Copy, PartialEq)]
struct DataSelection {
    dataset: Signal<Option<DatasetSelection>>,
    episode: Signal<Option<OpenedEpisode>>,
    importing: Signal<bool>,
    active_panel: Signal<Option<PanelId>>,
}

impl DataSelection {
    fn use_selection(initial_episode: Option<OpenedEpisode>) -> Self {
        Self {
            dataset: use_signal(|| {
                initial_episode
                    .as_ref()
                    .map(|episode| episode.dataset().clone())
            }),
            episode: use_signal(|| initial_episode.clone()),
            importing: use_signal(|| false),
            active_panel: use_signal(|| initial_episode.map(|_| PanelId::new("plots"))),
        }
    }
    fn activate(mut self, panel: &str) {
        self.active_panel.set(Some(PanelId::new(panel)));
    }
    /// Choosing a dataset shows its Overview; choosing a different one also
    /// closes the open episode.
    fn select_dataset(mut self, dataset: DatasetSelection) {
        if self.dataset.peek().as_ref() != Some(&dataset) {
            self.episode.set(None);
        }
        self.dataset.set(Some(dataset));
        self.activate("overview");
    }
}

/// Discovery of one episode's streams, tagged with the episode it belongs to
/// so a slow answer for the previous episode is never applied to the next.
#[derive(Clone, PartialEq)]
struct DiscoveredStreams {
    episode_id: String,
    result: Result<StreamCatalog, String>,
}

struct DataLayout;
impl DataLayout {
    fn initial(camera_ids: &[PanelId], browsing: bool) -> PanelLayout {
        let primary_ids = if browsing {
            vec!["overview", "plots", "messages", "sql", IMPORT_PANEL]
        } else {
            vec!["plots", "messages"]
        };
        let primary = LayoutNode::tile("primary", primary_ids.into_iter().map(PanelId::new));
        let editors = match camera_ids.camera_rows() {
            Some(cameras) => LayoutNode::split(
                "data-editors",
                SplitAxis::Horizontal,
                0.64,
                primary,
                cameras,
            ),
            None => primary,
        };
        let visualization = LayoutNode::split(
            "data-rows",
            SplitAxis::Vertical,
            0.6,
            editors,
            LayoutNode::tile("timeline", ["timeline".to_owned()]),
        );
        if !browsing {
            return PanelLayout::new(visualization);
        }
        PanelLayout::new(LayoutNode::split(
            "data-columns",
            SplitAxis::Horizontal,
            0.21,
            LayoutNode::tile("catalog", ["catalog".to_owned()]),
            visualization,
        ))
    }
}

#[component]
pub(super) fn DataPanels(#[props(default)] initial_episode: Option<OpenedEpisode>) -> Element {
    let browsing = initial_episode.is_none();
    let studio = crate::shell::state::StudioProjectState::current();
    let sessions = crate::shell::project::ProjectSessionSignals::current();
    let available = use_memo(move || sessions.catalog_available());
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
    let selection = DataSelection::use_selection(initial_episode.clone());
    let mut transport = EpisodeTransport::use_transport();
    use_hook(|| {
        if let Some(episode) = initial_episode {
            transport.load(
                episode
                    .record()
                    .time_range()
                    .unwrap_or_else(|| EpisodeTimeRange::new(0, 0)),
            );
        }
    });
    let mut choices = StreamChoices::use_choices();
    let controller = QueryController::use_controller(String::new());

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
    let discovery = use_resource(move || {
        let episode = selection.episode.read().clone();
        async move {
            let episode = episode?;
            let result = DatasetClient::current()
                .episode_topics(episode.selection())
                .await
                .map(|topics| StreamCatalog::new(topics, &episode.selection()))
                .map_err(|error| error.to_string());
            Some(DiscoveredStreams {
                episode_id: episode.record().id().to_owned(),
                result,
            })
        }
    });
    // Discovered streams become the episode's choices exactly once.
    let mut applied = use_signal(|| None::<String>);
    use_effect(move || {
        let episode = selection.episode.read().clone();
        let discovered = discovery.read().clone().flatten();
        let (Some(episode), Some(discovered)) = (episode, discovered) else {
            return;
        };
        if discovered.episode_id != episode.record().id()
            || applied.peek().as_deref() == Some(discovered.episode_id.as_str())
        {
            return;
        }
        applied.set(Some(discovered.episode_id.clone()));
        if let Ok(streams) = &discovered.result {
            choices.reset(streams);
            if let Some(range) = episode.record().time_range() {
                choices.load_coverage(streams, range);
            }
        }
    });

    let episode = selection.episode.read().clone();
    let dataset = selection.dataset.read().clone();
    let streams = episode.as_ref().map(|episode| {
        let pending = *discovery.state().read() == UseResourceState::Pending;
        match discovery.read().as_ref().and_then(Option::as_ref) {
            Some(discovered) if !pending && discovered.episode_id == episode.record().id() => {
                match &discovered.result {
                    Ok(streams) => EpisodeStreams::Ready(streams.clone()),
                    Err(error) => EpisodeStreams::Failed(error.clone()),
                }
            }
            _ => EpisodeStreams::Loading,
        }
    });
    let dataset_inspection = inspection.read().clone().flatten();

    let mut opening_transport = transport.clone();
    let open_episode = EventHandler::new(move |opened: OpenedEpisode| {
        let range = opened
            .record()
            .time_range()
            .unwrap_or_else(|| EpisodeTimeRange::new(0, 0));
        opening_transport.load(range);
        choices.reset(&StreamCatalog::default());
        let mut selection = selection;
        selection.dataset.set(Some(opened.dataset().clone()));
        selection.episode.set(Some(opened));
        selection.activate("plots");
    });
    let open_table = move |resource: ProjectResource| {
        controller.set_sql(resource.table_sql());
        controller.run_query(studio, *available.peek());
        selection.activate("sql");
    };
    let run_sql = move |sql: String| {
        controller.set_sql(sql);
        controller.run_query(studio, *available.peek());
        selection.activate("sql");
    };
    let start_import = move |(): ()| {
        let mut selection = selection;
        selection.importing.set(true);
        selection.activate(IMPORT_PANEL);
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

    let mut clearing_transport = transport.clone();
    let select_dataset = move |dataset: DatasetSelection| {
        selection.select_dataset(dataset);
        if selection.episode.peek().is_none() {
            clearing_transport.load(EpisodeTimeRange::new(0, 0));
            choices.reset(&StreamCatalog::default());
        }
    };
    // Naming a stream in the Timeline is a request to read it.
    use_effect(move || {
        if choices.selected().is_some() {
            selection.activate("messages");
        }
    });
    let rail_actions = RailActions::new(
        EventHandler::new(select_dataset),
        open_episode,
        EventHandler::new(open_table),
    )
    .with_import(EventHandler::new(start_import));
    let rail_selection = RailSelection::new(
        dataset.clone(),
        episode
            .as_ref()
            .map(|episode| episode.record().id().to_owned()),
        dataset_inspection.clone().and_then(|result| result.ok()),
    );

    // The primary tile keeps a stable tab order; content follows the selection.
    let mut panels =
        vec![
        Panel::new("catalog", "Datasets", "catalog", rsx! {
            CatalogRail { resources, selection: rail_selection, actions: rail_actions }
        })
        .with_class("data-catalog-panel"),
        Panel::new("overview", "Overview", "primary", rsx! {
            match &dataset {
                Some(dataset) => rsx! {
                    DatasetOverview {
                        dataset: dataset.clone(),
                        inspection: dataset_inspection.clone(),
                        actions: OverviewActions::new(open_episode, EventHandler::new(run_sql)),
                    }
                },
                None => rsx! { WelcomePanel { on_import: start_import } },
            }
        })
        .with_class("data-primary-panel"),
        Panel::new("plots", "Plots", "primary", rsx! {
            PlotsPanel { streams: streams.clone(), transport: transport.clone(), choices }
        })
        .with_class("data-primary-panel"),
        Panel::new("messages", "Messages", "primary", rsx! {
            MessagesPanel { streams: streams.clone(), transport: transport.clone(), choices }
        })
        .with_class("data-primary-panel"),
        Panel::new("sql", "SQL", "primary", rsx! { SqlPanel { studio, controller } })
            .with_class("data-primary-panel"),
    ];
    if (selection.importing)() {
        panels.push(
            Panel::new(IMPORT_PANEL, "Import", "primary", rsx! {
                super::import_dataset::ImportDataset {
                    on_done: move |name| { imported_dataset.set(Some(name)); super::relations::CatalogRefresh::request(); },
                }
            })
            .with_closable(true)
            .with_class("data-primary-panel"),
        );
    }

    let mut camera_ids = Vec::new();
    if let Some(EpisodeStreams::Ready(streams)) = &streams {
        for stream in streams.videos() {
            if !choices.camera_open(stream.key()) {
                continue;
            }
            camera_ids.push(stream.panel_id());
            panels.push(
                Panel::new(
                    stream.panel_id(),
                    stream.label().trim_start_matches('/').to_owned(),
                    "cameras",
                    rsx! { EpisodeCamera { request: stream.read().clone(), timeline: transport.range(), transport: transport.clone() } },
                )
                .with_closable(true)
                .with_home_zone(DockZone::Bottom)
                .with_class("camera-editor data-camera-panel"),
            );
        }
    }
    if camera_ids.is_empty() {
        camera_ids.push(PanelId::new("camera-empty"));
        panels.push(
            Panel::new(
                "camera-empty",
                "Cameras",
                "cameras",
                rsx! {
                    CameraPlaceholder { streams: streams.clone() }
                },
            )
            .with_class("camera-editor data-camera-panel"),
        );
    }
    let subject = match (&episode, &streams) {
        (Some(episode), Some(streams)) => Some(TimelineSubject::new(
            episode.clone(),
            streams.clone(),
            choices,
        )),
        _ => None,
    };
    panels.push(
        Panel::new(
            "timeline",
            "Timeline",
            "timeline",
            rsx! {
                TimelinePanel { subject, transport: transport.clone() }
            },
        )
        .with_class("timeline-editor data-timeline-panel"),
    );

    if !browsing {
        panels.retain(|panel| !matches!(panel.id.as_str(), "catalog" | "overview" | "sql"));
    }
    let initial_layout = DataLayout::initial(&camera_ids, browsing);
    let episode_open = episode.is_some();
    let mut keyboard = transport.clone();
    rsx! {
        div { class: "data-workspace flex-1 min-h-0 min-w-0 overflow-hidden", tabindex: "-1",
            onkeydown: move |event: KeyboardEvent| {
                if !episode_open || KeyboardFocus::editing() {
                    return;
                }
                if keyboard.handle_key(&event) {
                    event.prevent_default();
                }
            },
            PanelWorkspace {
                panels,
                initial_layout,
                active_panel: (selection.active_panel)(),
                layout_scope: Some(if browsing { LAYOUT_SCOPE } else { "spiral_robotics.panel-layout.v1.comparison-episode" }.to_owned()),
                on_panel_activate: move |id: PanelId| { let mut selection = selection; selection.active_panel.set(Some(id)); },
                on_panel_close: move |id: PanelId| {
                    if id.as_str() == IMPORT_PANEL {
                        let mut selection = selection;
                        selection.importing.set(false);
                    } else if let Some(key) = id.as_str().strip_prefix("camera:") {
                        choices.set_camera(key, false);
                    }
                    let mut selection = selection;
                    if selection.active_panel.peek().as_ref() == Some(&id) {
                        selection.active_panel.set(None);
                    }
                },
            }
        }
    }
}

#[component]
fn WelcomePanel(on_import: EventHandler<()>) -> Element {
    rsx! {
        div { class: "data-welcome",
            h2 { "Open your data" }
            p { "A dataset is a collection of MCAP recordings; each file is one episode. Choose a dataset on the left to see its episodes, or import recordings to create a new one." }
            Action { purpose: ActionPurpose::Import, variant: ActionVariant::Outline, size: ActionSize::Standard,
                onclick: move |_| on_import.call(()), "Import MCAP recordings" }
        }
    }
}

#[component]
fn CameraPlaceholder(streams: Option<EpisodeStreams>) -> Element {
    let (title, detail) = match streams {
        None => (
            "Cameras",
            "Recorded cameras of the open episode appear here, one panel each.",
        ),
        Some(EpisodeStreams::Loading) => ("Cameras", "Finding recorded streams…"),
        Some(EpisodeStreams::Failed(_)) => (
            "Cameras unavailable",
            "The episode's streams could not be read.",
        ),
        Some(EpisodeStreams::Ready(streams)) if streams.videos().next().is_none() => {
            ("No recorded cameras", "This episode has no video streams.")
        }
        Some(EpisodeStreams::Ready(_)) => (
            "Cameras hidden",
            "Tick a camera in the Timeline to show it again.",
        ),
    };
    rsx! {
        div { class: "editor-empty",
            strong { "{title}" }
            span { "{detail}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn data_layout_keeps_named_homes_for_cameras_and_the_timeline() {
        let cameras = [
            PanelId::new("camera:/top#a"),
            PanelId::new("camera:/wrist#b"),
        ];
        let layout = DataLayout::initial(&cameras, true);
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::new("catalog"))
                .unwrap()
                .as_str(),
            "catalog"
        );
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::new("plots"))
                .unwrap()
                .as_str(),
            "primary"
        );
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::new("sql"))
                .unwrap()
                .as_str(),
            "primary"
        );
        assert_eq!(
            layout.tile_for_panel(&cameras[0]).unwrap().as_str(),
            "cameras"
        );
        assert_eq!(
            layout.tile_for_panel(&cameras[1]).unwrap().as_str(),
            "cameras-2"
        );
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::new("timeline"))
                .unwrap()
                .as_str(),
            "timeline"
        );
        let without = DataLayout::initial(&[PanelId::new("camera-empty")], true);
        assert_eq!(
            without
                .tile_for_panel(&PanelId::new("camera-empty"))
                .unwrap()
                .as_str(),
            "cameras"
        );
        assert!(without.valid());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn comparison_layout_keeps_episode_panels_without_dataset_navigation() {
        let cameras = [PanelId::new("camera:/top#a")];
        let layout = DataLayout::initial(&cameras, false);
        assert!(layout.valid());
        for panel in ["plots", "messages", "timeline", "camera:/top#a"] {
            assert!(layout.tile_for_panel(&PanelId::new(panel)).is_some());
        }
        for panel in ["catalog", "overview", "sql", IMPORT_PANEL] {
            assert!(layout.tile_for_panel(&PanelId::new(panel)).is_none());
        }
    }
}
