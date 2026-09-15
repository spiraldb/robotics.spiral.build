//! Reuse the dataset's synchronized inspector for a comparison evidence row.
use super::episode_browser::EpisodeContext;
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelLayout, PanelWorkspace};
use spiral_robotics_common::datasets::DatasetSelection;

#[component]
pub(crate) fn ComparisonEpisodeInspector(
    dataset: DatasetSelection,
    row: serde_json::Map<String, serde_json::Value>,
    on_back: EventHandler,
) -> Element {
    match EpisodeContext::from_comparison(dataset, row) {
        Ok(episode) => {
            let panel = Panel::new(
                "comparison-recording",
                "Recorded episode",
                "comparison-recording",
                rsx! { super::episode_topics::EpisodeInspector { episode, on_back } },
            );
            // The shared player observes its owning panel's visibility to pause
            // playback. Mount through the same native host as the Data workspace.
            rsx! {
                PanelWorkspace {
                    panels: vec![panel],
                    initial_layout: PanelLayout::new(LayoutNode::tile("comparison-recording", ["comparison-recording"])),
                }
            }
        }
        Err(message) => {
            rsx! { div { class: "data-empty-copy", role: "alert", p { "{message}" } button { class: "data-text-action", onclick: move |_| on_back.call(()), "Back to comparison" } } }
        }
    }
}
