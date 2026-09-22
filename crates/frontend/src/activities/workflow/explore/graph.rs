//! The Explore canvas: a Dioxus Flow graph of dataset → measurement →
//! exploration nodes beside the selected node's workspace, plus the document
//! bar wiring that turns the Explore view on.
//!
//! Ranking the cards and parking the viewport are `components::flow_canvas`,
//! shared with Workflow's plan and Curation's pipeline; what is Explore's own
//! is which card a node draws and what its + offers.
mod canvas;
mod dataset;
mod document_bar;
mod node_card;
mod workspace;

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelLayout, PanelWorkspace, SplitAxis};

use super::document::ExplorationNodeKind;
use super::state::ExplorationController;
use canvas::ExploreNodeGlyph;
use node_card::ExploreNodeFace;

use crate::components::icons::WorkflowIcon;

pub(in crate::activities::workflow) use canvas::ExploreGraphCanvas;
pub(in crate::activities::workflow) use document_bar::ExplorationDocumentBar;

/// Which view of the Workflow activity the document bar and the panels show.
/// Agent authors SQL in a conversation; Explore assembles a measurement graph
/// over one dataset. They share the bar and nothing else.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::activities::workflow) enum WorkflowView {
    Agent,
    Explore,
}

impl WorkflowView {
    /// How often the Workflow activity re-reads its runs while this view is
    /// open. A run in flight is worth watching twice a second; a graph with
    /// nothing in flight is worth a glance; and Explore schedules no runs at
    /// all, so it reads none.
    pub(in crate::activities::workflow) const fn run_poll_interval_ms(
        self,
        run_active: bool,
    ) -> Option<u32> {
        match (self, run_active) {
            (Self::Explore, _) => None,
            (Self::Agent, true) => Some(500),
            (Self::Agent, false) => Some(5_000),
        }
    }
}

#[cfg(test)]
mod poll_tests {
    use super::WorkflowView;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn runs_are_watched_closely_only_while_one_is_in_flight_and_never_in_explore() {
        assert_eq!(WorkflowView::Agent.run_poll_interval_ms(true), Some(500));
        assert_eq!(WorkflowView::Agent.run_poll_interval_ms(false), Some(5_000));
        assert_eq!(WorkflowView::Explore.run_poll_interval_ms(true), None);
        assert_eq!(WorkflowView::Explore.run_poll_interval_ms(false), None);
    }
}

const GRAPH_PANEL_ID: &str = "explore-graph";
const WORKSPACE_PANEL_ID: &str = "explore-workspace";
/// Bumped with the default split below, so a layout saved at the old third
/// does not keep a quarter of the window for a three-card breadcrumb.
const EXPLORE_LAYOUT_SCOPE: &str = "workflow-explore-v2";

/// The Explore view: the graph in a resizable left pane, the selected node's
/// workspace in the larger right pane. Video results live in the workspace so
/// the graph's nodes stay small.
#[component]
pub(in crate::activities::workflow) fn ExplorePanels(controller: ExplorationController) -> Element {
    let selected = controller.selected_node();
    let kind = selected.and_then(|id| {
        controller.with_payload(|payload| payload.node(&id).map(|node| node.kind().clone()))
    });
    let panels = vec![
        Panel::new(
            GRAPH_PANEL_ID,
            "Graph",
            "explore-graph",
            rsx! { ExploreGraphCanvas { controller } },
        )
        .with_class("explore-canvas-graph-editor")
        .with_tab_icon(rsx! { WorkflowIcon {} }),
        Panel::new(
            WORKSPACE_PANEL_ID,
            kind.as_ref().map_or("Workspace", ExplorationNodeKind::label),
            "explore-workspace",
            rsx! { workspace::ExploreNodeWorkspace { controller } },
        )
        .with_class("explore-canvas-workspace-editor")
        .with_tab_icon(rsx! {
            ExploreNodeGlyph { face: kind.as_ref().map_or(ExploreNodeFace::Dataset, ExploreNodeFace::of) }
        }),
    ];
    rsx! {
        PanelWorkspace {
            panels,
            initial_layout: ExploreWorkspaceLayout::initial(),
            layout_scope: Some(EXPLORE_LAYOUT_SCOPE.to_owned()),
        }
    }
}

struct ExploreWorkspaceLayout;

impl ExploreWorkspaceLayout {
    /// The graph is a column of 224px cards, so a fifth of a 1440px window
    /// already fits two branches side by side with room to pan. It had a
    /// third, which left 122px of empty canvas either side of a single column
    /// while the workspace under study — episodes, tables, video — went short.
    /// Both tiles stay dockable and resizable, so a wider graph is a drag away.
    fn initial() -> PanelLayout {
        PanelLayout::new(LayoutNode::split(
            "explore-columns",
            SplitAxis::Horizontal,
            0.23,
            LayoutNode::tile("explore-graph", [GRAPH_PANEL_ID]),
            LayoutNode::tile("explore-workspace", [WORKSPACE_PANEL_ID]),
        ))
    }
}
