//! The workspace beside the graph: what the selected node needs, and nothing
//! else. Every node kind owns its workspace; this file chooses between them
//! and keeps video and tables out of the canvas.
use dioxus::prelude::*;
use spiral_robotics_ui::{EmptyState, Remount};

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
    // Two nodes of one kind are two searches, two rules, two runs: another
    // node mounts its workspace afresh rather than inheriting the last one's.
    rsx! {
        section { class: "curation-canvas-workspace", "aria-label": "{kind.label()}",
            Remount { identity: node.clone(),
                match kind {
                    CurationNodeKind::Dataset => rsx! { DatasetWorkspace { controller } },
                    CurationNodeKind::BreakDownSubtasks(_) => rsx! {
                        annotate::AnnotationWorkspace { controller, node: node.clone() }
                    },
                    CurationNodeKind::FindSimilarSubtasks(_) => rsx! {
                        similar::SearchWorkspace { controller, node: node.clone() }
                    },
                    CurationNodeKind::GroupSimilarSubtasks => rsx! {
                        similar::GroupingWorkspace { controller, node: node.clone() }
                    },
                    CurationNodeKind::IdentifyUnproductiveMoments(_) => rsx! {
                        moments::DetectionWorkspace { controller, node: node.clone() }
                    },
                    CurationNodeKind::Select(_) => rsx! {
                        select::SelectWorkspace { controller, node: node.clone() }
                    },
                }
            }
        }
    }
}
