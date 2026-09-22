//! Data: datasets and their episodes in a rail; one open episode across
//! cameras and plots on one timeline.
use super::{
    DatasetClient,
    catalog_rail::{CatalogRail, RailActions, RailSelection},
    dataset_overview::DatasetOverview,
    episode_list::OpenedEpisode,
    episode_playback::EpisodeCamera,
    episode_streams::{EpisodeStreams, RecordedStream, StreamCatalog, StreamChoices},
    episode_transport::{EpisodeTimeRange, EpisodeTransport, KeyboardFocus},
    plots_panel::PlotsPanel,
    timeline_panel::{TimelinePanel, TimelineSubject},
};
use crate::shell::{
    activity::{Activity, WorkspaceActivity},
    frame::WorkspaceFrame,
};
use dioxus::prelude::*;
use dioxus_workbench::{
    DockZone, LayoutNode, Panel, PanelId, PanelLayout, PanelPlacement, PanelWorkspace, SplitAxis,
};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant};
use std::collections::BTreeSet;
use wasm_bindgen::JsCast as _;

const LAYOUT_SCOPE: &str = "spiral_robotics.panel-layout.v16.data";
const IMPORT_PANEL: &str = "data-import";
/// The split between the primary tile and the camera column.
const EDITORS_SPLIT: &str = "data-editors";

#[component]
pub(crate) fn DataWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = WorkspaceActivity::provide(Activity::Data, is_active);
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

#[derive(Clone, Copy)]
struct DataLayout {
    layout: Signal<PanelLayout>,
    customized: Signal<bool>,
}

impl DataLayout {
    fn use_layout(canonical: PanelLayout) -> Self {
        let mut owner = Self {
            layout: use_signal(|| canonical.clone()),
            customized: use_signal(|| false),
        };
        // Cameras arrive asynchronously. Follow the default until an operator
        // resizes or docks a panel; after that they own the geometry, but the
        // camera column still follows the episode, so a newly opened
        // episode's cameras take the tiles the grid gives them rather than
        // whatever the previous episode left behind.
        use_effect(use_reactive((&canonical,), move |(mut canonical,)| {
            let current = owner.layout.peek().clone();
            let next = if *owner.customized.peek() {
                let Some(next) = Self::with_cameras_of(&current, &canonical) else {
                    return;
                };
                next
            } else {
                Self::restore_active(&current.root, &mut canonical);
                canonical
            };
            if current != next {
                owner.layout.set(next);
            }
        }));
        owner
    }

    /// The current layout with its camera column replaced by the canonical
    /// one when the set of camera panels changed; `None` when nothing needs
    /// to change. Without a camera column to rebuild, the grid opens beside
    /// the primary tile; without a primary tile, only the canonical tree
    /// knows where cameras go.
    fn with_cameras_of(current: &PanelLayout, canonical: &PanelLayout) -> Option<PanelLayout> {
        if Self::camera_panels(&current.root) == Self::camera_panels(&canonical.root) {
            return None;
        }
        let grid = Self::editors(&canonical.root).map(|(_, cameras)| cameras.clone());
        let mut next = current.clone();
        // The stage follows the episode alongside its camera tiles.
        next.close_panel(&PanelId::new("stage"));
        match (Self::editors_mut(&mut next.root), grid) {
            (Some(editors), Some(grid)) => {
                if let LayoutNode::Split { second, .. } = editors {
                    **second = grid;
                }
            }
            (Some(editors), None) => {
                let primary = match &*editors {
                    LayoutNode::Split { first, .. } => (**first).clone(),
                    tile => tile.clone(),
                };
                *editors = primary;
            }
            (None, Some(grid)) => {
                let Some(primary) = Self::tile_node_mut(&mut next.root, "primary") else {
                    return Some(canonical.clone());
                };
                let primary_tile = primary.clone();
                *primary = LayoutNode::split(
                    EDITORS_SPLIT,
                    SplitAxis::Horizontal,
                    0.46,
                    primary_tile,
                    grid,
                );
            }
            (None, None) => return None,
        }
        Some(next)
    }

    fn tile_node_mut<'a>(node: &'a mut LayoutNode, id: &str) -> Option<&'a mut LayoutNode> {
        let is_tile = matches!(node, LayoutNode::Tile(tile) if tile.id.as_str() == id);
        if is_tile {
            return Some(node);
        }
        match node {
            LayoutNode::Tile(_) => None,
            LayoutNode::Split { first, second, .. } => {
                Self::tile_node_mut(first, id).or_else(|| Self::tile_node_mut(second, id))
            }
        }
    }

    fn is_camera(panel: &PanelId) -> bool {
        panel.as_str().starts_with("camera:") || panel.as_str() == "camera-empty"
    }

    fn camera_panels(node: &LayoutNode) -> BTreeSet<String> {
        match node {
            LayoutNode::Tile(tile) => tile
                .panels
                .iter()
                .filter(|panel| Self::is_camera(panel))
                .map(|panel| panel.as_str().to_owned())
                .collect(),
            LayoutNode::Split { first, second, .. } => {
                let mut all = Self::camera_panels(first);
                all.extend(Self::camera_panels(second));
                all
            }
        }
    }

    /// The primary tile and the camera column of the editors split.
    fn editors(node: &LayoutNode) -> Option<(&LayoutNode, &LayoutNode)> {
        match node {
            LayoutNode::Tile(_) => None,
            LayoutNode::Split {
                id, first, second, ..
            } if id.as_str() == EDITORS_SPLIT => Some((first, second)),
            LayoutNode::Split { first, second, .. } => {
                Self::editors(first).or_else(|| Self::editors(second))
            }
        }
    }

    fn editors_mut(node: &mut LayoutNode) -> Option<&mut LayoutNode> {
        let is_editors =
            matches!(node, LayoutNode::Split { id, .. } if id.as_str() == EDITORS_SPLIT);
        if is_editors {
            return Some(node);
        }
        match node {
            LayoutNode::Tile(_) => None,
            LayoutNode::Split { first, second, .. } => {
                Self::editors_mut(first).or_else(|| Self::editors_mut(second))
            }
        }
    }

    fn observe(mut self, actual: &PanelLayout, canonical: &PanelLayout) {
        if !Self::same_arrangement(&actual.root, &canonical.root) {
            self.customized.set(true);
        }
    }

    fn same_arrangement(left: &LayoutNode, right: &LayoutNode) -> bool {
        match (left, right) {
            (LayoutNode::Tile(left), LayoutNode::Tile(right)) => {
                left.id == right.id && left.panels == right.panels
            }
            (
                LayoutNode::Split {
                    id,
                    axis,
                    ratio,
                    first,
                    second,
                },
                LayoutNode::Split {
                    id: other_id,
                    axis: other_axis,
                    ratio: other_ratio,
                    first: other_first,
                    second: other_second,
                },
            ) => {
                id == other_id
                    && axis == other_axis
                    && ratio == other_ratio
                    && Self::same_arrangement(first, other_first)
                    && Self::same_arrangement(second, other_second)
            }
            _ => false,
        }
    }

    fn restore_active(node: &LayoutNode, next: &mut PanelLayout) {
        match node {
            LayoutNode::Tile(tile) => {
                if let Some(active) = &tile.active {
                    next.activate(active);
                }
            }
            LayoutNode::Split { first, second, .. } => {
                Self::restore_active(first, next);
                Self::restore_active(second, next);
            }
        }
    }

    /// Prefer an explicitly named top view, then retain topic order.
    fn cameras(catalog: &StreamCatalog) -> Vec<&RecordedStream> {
        let mut cameras = catalog.videos().collect::<Vec<_>>();
        cameras.sort_by_key(|stream| {
            !stream
                .name()
                .split(|character: char| !character.is_alphanumeric())
                .any(|part| part.eq_ignore_ascii_case("top"))
        });
        cameras
    }

    /// Two review panels per row, with the stage below the first camera pair.
    fn review_grid(tiles: &[LayoutNode], offset: usize) -> LayoutNode {
        let first = tiles[0].clone();
        let row = match tiles.get(1) {
            Some(second) => LayoutNode::split(
                format!("camera-columns-{offset}"),
                SplitAxis::Horizontal,
                0.5,
                first,
                second.clone(),
            ),
            None => first,
        };
        if tiles.len() <= 2 {
            return row;
        }
        LayoutNode::split(
            format!("camera-grid-rows-{offset}"),
            SplitAxis::Vertical,
            1.0 / tiles.len().div_ceil(2) as f64,
            row,
            Self::review_grid(&tiles[2..], offset + 2),
        )
    }

    fn initial(camera_ids: &[PanelId]) -> PanelLayout {
        let primary_ids = ["overview", "plots", IMPORT_PANEL];
        let primary = LayoutNode::tile("primary", primary_ids.into_iter().map(PanelId::new));
        let mut review_tiles = camera_ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                let home = if index == 0 {
                    "cameras".to_owned()
                } else {
                    format!("cameras-{}", index + 1)
                };
                LayoutNode::tile(home, [id.clone()])
            })
            .collect::<Vec<_>>();
        review_tiles.insert(
            camera_ids.len().min(2),
            LayoutNode::tile("stage", ["stage"]),
        );
        let editors = LayoutNode::split(
            EDITORS_SPLIT,
            SplitAxis::Horizontal,
            0.46,
            primary,
            Self::review_grid(&review_tiles, 0),
        );
        let visualization = LayoutNode::split(
            "data-rows",
            SplitAxis::Vertical,
            0.6,
            editors,
            LayoutNode::tile("timeline", ["timeline".to_owned()]),
        );
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
fn DataPanels() -> Element {
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
    let selection = DataSelection::use_selection();
    let transport = EpisodeTransport::use_transport();
    // Space, the arrows and Home/End also work while focus rests on the page
    // itself, not only inside the workspace. A key pressed inside it is
    // handled there and never reaches this listener as a page key.
    let activity = WorkspaceActivity::current();
    let page_keys = use_hook(|| {
        let mut transport = transport.clone();
        let listener = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                let on_page = event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|element| matches!(element.tag_name().as_str(), "BODY" | "HTML"));
                if !on_page
                    || !activity.is_active()
                    || selection.episode.peek().is_none()
                    || KeyboardFocus::editing()
                {
                    return;
                }
                if transport.handle_key(&event) {
                    event.prevent_default();
                }
            },
        );
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            let _ = document
                .add_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref());
        }
        std::rc::Rc::new(listener)
    });
    let page_keys_to_drop = page_keys.clone();
    use_drop(move || {
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            let _ = document.remove_event_listener_with_callback(
                "keydown",
                page_keys_to_drop.as_ref().as_ref().unchecked_ref(),
            );
        }
    });
    let mut choices = StreamChoices::use_choices();
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
            choices.plot_default_stream(streams);
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
    // While the chosen dataset's definition loads, the previous dataset's
    // streams must not appear under the new name.
    let dataset_inspection = if *inspection.state().read() == UseResourceState::Pending {
        None
    } else {
        inspection.read().clone().flatten()
    };

    let mut opening_transport = transport.clone();
    let open_episode = EventHandler::new(move |opened: OpenedEpisode| {
        let range = opened
            .record()
            .time_range()
            .unwrap_or_else(|| EpisodeTimeRange::new(0, 0));
        opening_transport.load(range);
        if selection.episode.peek().as_ref() != Some(&opened) {
            choices.reset(&StreamCatalog::default());
            applied.set(None);
        }
        let mut selection = selection;
        selection.dataset.set(Some(opened.dataset().clone()));
        selection.episode.set(Some(opened));
        selection.activate("plots");
    });
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
    let rail_actions = RailActions::new(EventHandler::new(select_dataset), open_episode)
        .with_import(EventHandler::new(start_import));
    let rail_selection = RailSelection::new(
        dataset.clone(),
        episode
            .as_ref()
            .map(|episode| episode.record().id().to_owned()),
        dataset_inspection.clone().and_then(|result| result.ok()),
    );

    // The primary tile keeps a stable tab order; content follows the selection.
    let mut panels = vec![
        Panel::new("stage", "3D Stage", "stage", rsx! {
            super::robot_stage::RobotStagePanel { episode: episode.clone(), streams: streams.clone(), transport: transport.clone() }
        }).with_class("data-primary-panel"),
        Panel::new(
            "catalog",
            "Datasets",
            "catalog",
            rsx! {
                CatalogRail { resources, selection: rail_selection, actions: rail_actions }
            },
        )
        .with_class("data-catalog-panel"),
        Panel::new(
            "overview",
            "Overview",
            "primary",
            rsx! {
                match &dataset {
                    Some(dataset) => rsx! {
                        DatasetOverview {
                            dataset: dataset.clone(),
                            inspection: dataset_inspection.clone(),
                        }
                    },
                    None => rsx! { WelcomePanel { on_import: start_import } },
                }
            },
        )
        .with_class("data-primary-panel"),
        Panel::new(
            "plots",
            "Plots",
            "primary",
            rsx! {
                PlotsPanel { streams: streams.clone(), transport: transport.clone(), choices }
            },
        )
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
        for stream in DataLayout::cameras(streams) {
            if !choices.camera_open(stream.key()) {
                continue;
            }
            camera_ids.push(stream.panel_id());
            panels.push(
                Panel::new(
                    stream.panel_id(),
                    format!("Camera · {}", streams.camera_title(stream)),
                    "cameras",
                    rsx! { EpisodeCamera { request: stream.read().clone(), timeline: transport.range(), transport: transport.clone() } },
                )
                .with_closable(true)
                .with_home_zone(DockZone::Bottom)
                .with_class("camera-editor data-camera-panel"),
            );
        }
    }
    // The camera placeholder appears with the episode and gives way to its
    // recorded cameras as they are found, alongside the stage.
    if camera_ids.is_empty() && episode.is_some() {
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

    let initial_layout = DataLayout::initial(&camera_ids);
    let placements = panels
        .iter()
        .map(|panel| {
            PanelPlacement::new(panel.id.clone(), panel.home.clone()).with_zone(panel.home_zone)
        })
        .collect::<Vec<_>>();
    let canonical = initial_layout.reconciled(&placements);
    let layout = DataLayout::use_layout(canonical.clone());
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
                layout: layout.layout,
                on_layout_change: move |actual| layout.observe(&actual, &canonical),
                active_panel: (selection.active_panel)(),
                layout_scope: Some(LAYOUT_SCOPE.to_owned()),
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
            Button { purpose: ActionPurpose::Import, variant: ButtonVariant::Outline, size: ButtonSize::Standard,
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

    #[test]
    fn data_default_layout_pairs_cameras_and_stage_above_shared_timeline() {
        let cameras = [
            PanelId::new("top"),
            PanelId::new("left"),
            PanelId::new("right"),
        ];
        let layout = DataLayout::initial(&cameras);
        let LayoutNode::Split {
            ratio,
            first,
            second,
            ..
        } = &layout.root
        else {
            panic!("dataset rail")
        };
        assert_eq!(*ratio, 0.21);
        assert!(matches!(first.as_ref(), LayoutNode::Tile(tile) if tile.id.as_str() == "catalog"));
        let LayoutNode::Split {
            axis,
            ratio,
            first,
            second,
            ..
        } = second.as_ref()
        else {
            panic!("timeline row")
        };
        assert_eq!(*axis, SplitAxis::Vertical);
        assert_eq!(*ratio, 0.6);
        assert!(
            matches!(second.as_ref(), LayoutNode::Tile(tile) if tile.id.as_str() == "timeline")
        );
        let LayoutNode::Split { ratio, second, .. } = first.as_ref() else {
            panic!("plots and cameras")
        };
        assert_eq!(*ratio, 0.46);
        let LayoutNode::Split {
            axis,
            ratio,
            first,
            second,
            ..
        } = second.as_ref()
        else {
            panic!("camera rows")
        };
        assert_eq!(*axis, SplitAxis::Vertical);
        assert_eq!(*ratio, 0.5);
        assert!(matches!(
            first.as_ref(),
            LayoutNode::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.5,
                ..
            }
        ));
        let LayoutNode::Split { first, second, .. } = second.as_ref() else {
            panic!("stage and third camera")
        };
        assert!(
            matches!(first.as_ref(), LayoutNode::Tile(tile) if tile.panels == [PanelId::new("stage")])
        );
        assert!(
            matches!(second.as_ref(), LayoutNode::Tile(tile) if tile.panels == [cameras[2].clone()])
        );
        for count in 0..=6 {
            let ids = (0..count)
                .map(|index| PanelId::new(format!("camera-{index}")))
                .collect::<Vec<_>>();
            assert!(DataLayout::initial(&ids).valid());
        }
    }

    #[test]
    fn data_default_layout_distinguishes_user_geometry_from_tab_activation() {
        let original = DataLayout::initial(&[PanelId::new("camera-empty")]);
        let mut activated = original.clone();
        activated.activate(&PanelId::new("plots"));
        assert!(DataLayout::same_arrangement(
            &original.root,
            &activated.root
        ));
        let mut discovered = DataLayout::initial(&[PanelId::new("top"), PanelId::new("left")]);
        DataLayout::restore_active(&activated.root, &mut discovered);
        assert_eq!(
            discovered.tile(&"primary".into()).unwrap().active,
            Some(PanelId::new("plots"))
        );
        let mut resized = activated.clone();
        if let LayoutNode::Split { ratio, .. } = &mut resized.root {
            *ratio = 0.3;
        }
        assert!(!DataLayout::same_arrangement(&original.root, &resized.root));
        let mut docked = original.clone();
        docked.dock_panel(&PanelId::new("stage"), &"timeline".into(), DockZone::Center);
        assert_eq!(
            docked
                .tile_for_panel(&PanelId::new("stage"))
                .unwrap()
                .as_str(),
            "timeline"
        );
        assert!(!DataLayout::same_arrangement(&original.root, &docked.root));
    }

    #[test]
    fn cameras_follow_the_episode_inside_the_operators_arrangement() {
        let three = [
            PanelId::new("camera:/top#a"),
            PanelId::new("camera:/left#b"),
            PanelId::new("camera:/right#c"),
        ];
        let two = [
            PanelId::new("camera:/top#d"),
            PanelId::new("camera:/gripper#e"),
        ];
        let mut arranged = DataLayout::initial(&three);
        if let LayoutNode::Split { ratio, .. } = &mut arranged.root {
            *ratio = 0.3;
        }
        let canonical = DataLayout::initial(&two);
        let next = DataLayout::with_cameras_of(&arranged, &canonical).unwrap();
        assert!(next.valid());
        assert!(matches!(&next.root, LayoutNode::Split { ratio, .. } if *ratio == 0.3));
        assert_eq!(next.tile_for_panel(&two[0]).unwrap().as_str(), "cameras");
        assert_eq!(next.tile_for_panel(&two[1]).unwrap().as_str(), "cameras-2");
        assert!(next.tile_for_panel(&three[2]).is_none());
        assert!(next.tile(&"cameras-3".into()).is_none());
        // The same cameras again change nothing.
        assert!(DataLayout::with_cameras_of(&next, &canonical).is_none());
        // Closing the episode removes cameras, retaining the stage and ratio.
        let browsing = DataLayout::initial(&[]);
        let closed = DataLayout::with_cameras_of(&next, &browsing).unwrap();
        assert!(closed.valid());
        assert!(closed.tile(&"cameras".into()).is_none());
        assert_eq!(
            closed
                .tile_for_panel(&PanelId::new("plots"))
                .unwrap()
                .as_str(),
            "primary"
        );
        assert!(matches!(&closed.root, LayoutNode::Split { ratio, .. } if *ratio == 0.3));
        // Reopening from the browsing arrangement grows the camera column
        // beside the primary tile and still keeps the operator's ratio.
        let reopened = DataLayout::with_cameras_of(&closed, &canonical).unwrap();
        assert!(reopened.valid());
        assert!(matches!(&reopened.root, LayoutNode::Split { ratio, .. } if *ratio == 0.3));
        assert_eq!(
            reopened.tile_for_panel(&two[0]).unwrap().as_str(),
            "cameras"
        );
        assert_eq!(
            reopened
                .tile_for_panel(&PanelId::new("plots"))
                .unwrap()
                .as_str(),
            "primary"
        );
        assert!(DataLayout::editors(&reopened.root).is_some());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn data_layout_keeps_named_homes_for_cameras_and_the_timeline() {
        let cameras = [
            PanelId::new("camera:/top#a"),
            PanelId::new("camera:/wrist#b"),
        ];
        let layout = DataLayout::initial(&cameras);
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
        let without = DataLayout::initial(&[PanelId::new("camera-empty")]);
        assert_eq!(
            without
                .tile_for_panel(&PanelId::new("camera-empty"))
                .unwrap()
                .as_str(),
            "cameras"
        );
        assert!(without.valid());
    }
}
