//! The workspace beside the graph: what the selected node needs, and nothing
//! else. Measurements and explorations belong to the other Build streams; this
//! file chooses between them and keeps video and tables out of the canvas.
use dioxus::prelude::*;
use spiral_robotics_ui::EmptyState;

use super::super::document::BuildNodeKind;
use super::super::state::BuildController;
use super::super::{annotate, explore};
use super::dataset::DatasetWorkspace;

#[component]
pub(super) fn BuildNodeWorkspace(controller: BuildController) -> Element {
    let selected = controller.selected_node();
    let kind = selected.as_ref().and_then(|id| {
        controller.with_payload(|payload| payload.node(id).map(|node| node.kind().clone()))
    });
    let Some((node, kind)) = selected.zip(kind) else {
        return rsx! {
            section { class: "build-canvas-workspace",
                EmptyState { p { "Select a node in the graph to work on it." } }
            }
        };
    };
    rsx! {
        section { class: "build-canvas-workspace", "aria-label": "{kind.label()}",
            match kind {
                BuildNodeKind::Dataset => rsx! { DatasetWorkspace { controller } },
                BuildNodeKind::BreakDownSubtasks(_) => rsx! {
                    annotate::AnnotationWorkspace { key: "{node}", controller, node: node.clone() }
                },
                BuildNodeKind::FindSimilarSubtasks(_) => rsx! {
                    explore::SearchWorkspace { key: "{node}", controller, node: node.clone() }
                },
                BuildNodeKind::GroupSimilarSubtasks => rsx! {
                    explore::GroupingWorkspace { key: "{node}", controller, node: node.clone() }
                },
                BuildNodeKind::FindUnproductiveMoments => rsx! { PlannedMeasurement {} },
            }
        }
    }
}

/// Curation's measurement is not part of a Build graph yet. The node keeps its
/// place so a graph that plans for it still reads, and says so here.
#[component]
fn PlannedMeasurement() -> Element {
    rsx! {
        EmptyState { title: "Find unproductive moments is planned",
            p { "This measurement is not implemented yet. Its node stays in the graph, dimmed, and runs nothing." }
        }
    }
}
