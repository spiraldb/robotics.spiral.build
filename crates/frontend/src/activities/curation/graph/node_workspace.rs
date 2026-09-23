//! The workspace beside the graph: what the selected node needs, and nothing
//! else. Every node kind owns its workspace; this file chooses between them
//! and keeps video and tables out of the canvas.
use dioxus::prelude::*;
use spiral_robotics_ui::EmptyState;

use super::super::document::CurationNodeKind;
use super::super::state::CurationController;
use super::super::{annotate, moments, select, similar};
use super::dataset::DatasetWorkspace;

#[component]
pub(in crate::activities::curation) fn NodeWorkspace(controller: CurationController) -> Element {
    let selected = controller.selected_node();
    let kind = selected.as_ref().and_then(|id| {
        controller.with_payload(|payload| payload.node(id).map(|node| node.kind().clone()))
    });
    let Some((node, kind)) = selected.zip(kind) else {
        return rsx! {
            section { class: "curation-canvas-workspace",
                EmptyState { p { "Select a node in the graph to work on it." } }
            }
        };
    };
    rsx! {
        section { class: "curation-canvas-workspace", "aria-label": "{kind.label()}",
            match kind {
                CurationNodeKind::Dataset => rsx! { DatasetWorkspace { controller } },
                CurationNodeKind::BreakDownSubtasks(_) => rsx! {
                    annotate::AnnotationWorkspace { key: "{node}", controller, node: node.clone() }
                },
                CurationNodeKind::FindSimilarSubtasks(_) => rsx! {
                    similar::SearchWorkspace { key: "{node}", controller, node: node.clone() }
                },
                CurationNodeKind::GroupSimilarSubtasks => rsx! {
                    similar::GroupingWorkspace { key: "{node}", controller, node: node.clone() }
                },
                CurationNodeKind::IdentifyUnproductiveMoments(_) => rsx! {
                    moments::DetectionWorkspace { key: "{node}", controller, node: node.clone() }
                },
                CurationNodeKind::Select(_) => rsx! {
                    select::SelectWorkspace { key: "{node}", controller, node: node.clone() }
                },
            }
        }
    }
}
