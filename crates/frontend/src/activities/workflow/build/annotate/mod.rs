//! Break down subtasks: browser-owned annotation of every episode in the
//! dataset, one model call per episode, rows persisted through the
//! controller after each episode. Owned by the Annotation stream.
use super::state::BuildController;
use dioxus::prelude::*;

/// The workspace shown for a selected Break down subtasks node.
#[component]
pub(super) fn AnnotationWorkspace(controller: BuildController, node: String) -> Element {
    let _ = (controller, node);
    rsx! {
        section { class: "build-annotate-workspace", role: "status",
            "Break down subtasks is not implemented yet."
        }
    }
}
