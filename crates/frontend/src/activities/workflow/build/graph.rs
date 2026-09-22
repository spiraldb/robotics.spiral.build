//! The Build canvas: a Dioxus Flow graph of dataset → measurement →
//! exploration nodes beside the selected node's workspace, plus the document
//! bar wiring that turns the Build view on. Owned by the Canvas stream.
//!
//! Follow `plan_display.rs` for `compute_layout`, fit-to-view, and popovers,
//! and `curation/pipeline.rs` for typed handles on branching nodes.
mod canvas;
mod dataset;
mod document_bar;
mod node_card;
mod workspace;

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelLayout, PanelWorkspace, SplitAxis};

use super::document::BuildNodeKind;
use super::state::BuildController;
use canvas::BuildNodeGlyph;
use node_card::BuildNodeFace;

use crate::components::icons::WorkflowIcon;

pub(in crate::activities::workflow) use canvas::BuildGraphCanvas;
pub(in crate::activities::workflow) use document_bar::BuildDocumentBar;

/// Which view of the Workflow activity the document bar and the panels show.
/// Agent authors SQL in a conversation; Build assembles a measurement graph
/// over one dataset. They share the bar and nothing else.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::activities::workflow) enum WorkflowView {
    Agent,
    Build,
}

const GRAPH_PANEL_ID: &str = "build-graph";
const WORKSPACE_PANEL_ID: &str = "build-workspace";
const BUILD_LAYOUT_SCOPE: &str = "workflow-build-v1";

/// The Build view: the graph in a resizable left pane, the selected node's
/// workspace in the larger right pane. Video results live in the workspace so
/// the graph's nodes stay small.
#[component]
pub(in crate::activities::workflow) fn BuildPanels(controller: BuildController) -> Element {
    let selected = controller.selected_node();
    let kind = selected.and_then(|id| {
        controller.with_payload(|payload| payload.node(&id).map(|node| node.kind().clone()))
    });
    let panels = vec![
        Panel::new(
            GRAPH_PANEL_ID,
            "Graph",
            "build-graph",
            rsx! { BuildGraphCanvas { controller } },
        )
        .with_class("build-canvas-graph-editor")
        .with_tab_icon(rsx! { WorkflowIcon {} }),
        Panel::new(
            WORKSPACE_PANEL_ID,
            kind.as_ref().map_or("Workspace", BuildNodeKind::label),
            "build-workspace",
            rsx! { workspace::BuildNodeWorkspace { controller } },
        )
        .with_class("build-canvas-workspace-editor")
        .with_tab_icon(rsx! {
            BuildNodeGlyph { face: kind.as_ref().map_or(BuildNodeFace::Dataset, BuildNodeFace::of) }
        }),
    ];
    rsx! {
        PanelWorkspace {
            panels,
            initial_layout: BuildWorkspaceLayout::initial(),
            layout_scope: Some(BUILD_LAYOUT_SCOPE.to_owned()),
        }
    }
}

struct BuildWorkspaceLayout;

impl BuildWorkspaceLayout {
    /// The graph is a column of 224px cards: a third of the width holds two
    /// branches side by side, and the workspace keeps the rest for episodes,
    /// tables, and video. Both tiles stay dockable and resizable.
    fn initial() -> PanelLayout {
        PanelLayout::new(LayoutNode::split(
            "build-columns",
            SplitAxis::Horizontal,
            0.33,
            LayoutNode::tile("build-graph", [GRAPH_PANEL_ID]),
            LayoutNode::tile("build-workspace", [WORKSPACE_PANEL_ID]),
        ))
    }
}
