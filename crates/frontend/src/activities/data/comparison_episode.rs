//! Reuse the dataset's synchronized inspector for a comparison evidence row.
use super::{episode_list::OpenedEpisode, screen::DataPanels};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSelection;

#[component]
pub(crate) fn ComparisonEpisodeInspector(
    dataset: DatasetSelection,
    row: serde_json::Map<String, serde_json::Value>,
    on_back: EventHandler,
) -> Element {
    let episode = OpenedEpisode::from_comparison(dataset, row);
    rsx! {
        Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Link,
            onclick: move |_| on_back.call(()), "Back to comparison" }
        match episode {
            Ok(episode) => rsx! {
                DataPanels {
                    key: "{episode.dataset().label()}:{episode.record().id()}",
                    initial_episode: episode,
                }
            },
            Err(message) => rsx! { div { class: "data-empty-copy", role: "alert", "{message}" } },
        }
    }
}
