//! The Curation activity: the graph in a narrow left pane, headed by the
//! document bar, and the selected node's workspace in the larger right pane.
//! Video and tables live in the workspace so the graph's nodes stay small.
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelLayout, PanelWorkspace, SplitAxis};

use super::document::CurationNodeKind;
use super::graph::{CurationDocumentBar, CurationGraphCanvas, NodeFace, NodeGlyph, NodeWorkspace};
use super::state::CurationController;
use crate::components::icons::GraphIcon;

const GRAPH_PANEL_ID: &str = "curation-graph";
const WORKSPACE_PANEL_ID: &str = "curation-workspace";
const CURATION_LAYOUT_SCOPE: &str = "curation-graph-v2";

#[component]
fn CurationPanels(controller: CurationController) -> Element {
    let selected = controller.selected_node();
    let kind = selected.and_then(|id| {
        controller.with_payload(|payload| payload.node(&id).map(|node| node.kind().clone()))
    });
    let panels = vec![
        Panel::new(
            GRAPH_PANEL_ID,
            "Graph",
            "curation-graph",
            rsx! {
                div { class: "curation-graph-pane",
                    CurationDocumentBar { controller }
                    CurationGraphCanvas { controller }
                }
            },
        )
        .with_class("curation-canvas-graph-editor")
        .with_tab_icon(rsx! { GraphIcon {} }),
        Panel::new(
            WORKSPACE_PANEL_ID,
            kind.as_ref().map_or("Workspace", CurationNodeKind::label),
            "curation-workspace",
            rsx! { NodeWorkspace { controller } },
        )
        .with_class("curation-canvas-workspace-editor")
        .with_tab_icon(rsx! {
            NodeGlyph { face: kind.as_ref().map_or(NodeFace::Dataset, NodeFace::of) }
        }),
    ];
    rsx! {
        PanelWorkspace {
            panels,
            initial_layout: CurationWorkspaceLayout::initial(),
            layout_scope: Some(CURATION_LAYOUT_SCOPE.to_owned()),
        }
    }
}

struct CurationWorkspaceLayout;

impl CurationWorkspaceLayout {
    /// The graph pane is headed by the document bar, whose name, Discard,
    /// Save, and Publish want about 390px, so the graph takes just over a
    /// quarter of a 1440px window, and the workspace under study — episodes,
    /// tables, video — keeps the rest. Both tiles stay dockable and
    /// resizable, so a wider graph is a drag away.
    fn initial() -> PanelLayout {
        PanelLayout::new(LayoutNode::split(
            "curation-columns",
            SplitAxis::Horizontal,
            0.27,
            LayoutNode::tile("curation-graph", [GRAPH_PANEL_ID]),
            LayoutNode::tile("curation-workspace", [WORKSPACE_PANEL_ID]),
        ))
    }
}

#[component]
pub(crate) fn CurationWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Curation,
        is_active,
    );
    let controller = CurationController::use_controller();
    // Previews live in this tab only, so a reload asks first while they, a
    // run, or unsaved edits would go with it.
    super::leave_guard::LeaveGuard::use_while(move || controller.holds_unkept_work());
    let status = use_memo(move || controller.runs());
    rsx! {
        crate::shell::frame::WorkspaceFrame {
            workspace,
            status: crate::shell::frame::WorkspaceStatus::Curation(status),
            CurationPanels { controller }
        }
    }
}
