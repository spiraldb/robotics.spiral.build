//! Explorations over a saved subtask table: rank rows against a query with
//! jev, and group rows into LLM-proposed categories assigned by jev. Owned
//! by the Explore stream.
use super::state::BuildController;
use dioxus::prelude::*;

/// The workspace shown for a selected Find similar subtasks node.
#[component]
pub(super) fn SearchWorkspace(controller: BuildController, node: String) -> Element {
    let _ = (controller, node);
    rsx! {
        section { class: "build-explore-workspace", role: "status",
            "Find similar subtasks is not implemented yet."
        }
    }
}

/// The workspace shown for a selected Group similar subtasks node.
#[component]
pub(super) fn GroupingWorkspace(controller: BuildController, node: String) -> Element {
    let _ = (controller, node);
    rsx! {
        section { class: "build-explore-workspace", role: "status",
            "Group similar subtasks is not implemented yet."
        }
    }
}
