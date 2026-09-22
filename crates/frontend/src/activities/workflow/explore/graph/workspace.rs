//! The workspace beside the graph: what the selected node needs, and nothing
//! else. Measurements and explorations own their own workspaces; this file
//! chooses between them and keeps video and tables out of the canvas.
use dioxus::prelude::*;
use spiral_robotics_ui::EmptyState;

use super::super::document::ExplorationNodeKind;
use super::super::state::ExplorationController;
use super::super::{annotate, similar};
use super::dataset::DatasetWorkspace;

#[component]
pub(super) fn ExploreNodeWorkspace(controller: ExplorationController) -> Element {
    let selected = controller.selected_node();
    let kind = selected.as_ref().and_then(|id| {
        controller.with_payload(|payload| payload.node(id).map(|node| node.kind().clone()))
    });
    let Some((node, kind)) = selected.zip(kind) else {
        return rsx! {
            section { class: "explore-canvas-workspace",
                EmptyState { p { "Select a node in the graph to work on it." } }
            }
        };
    };
    rsx! {
        section { class: "explore-canvas-workspace", "aria-label": "{kind.label()}",
            match kind {
                ExplorationNodeKind::Dataset => rsx! { DatasetWorkspace { controller } },
                ExplorationNodeKind::BreakDownSubtasks(_) => rsx! {
                    annotate::AnnotationWorkspace { key: "{node}", controller, node: node.clone() }
                },
                ExplorationNodeKind::FindSimilarSubtasks(_) => rsx! {
                    similar::SearchWorkspace { key: "{node}", controller, node: node.clone() }
                },
                ExplorationNodeKind::GroupSimilarSubtasks => rsx! {
                    similar::GroupingWorkspace { key: "{node}", controller, node: node.clone() }
                },
            }
        }
    }
}
