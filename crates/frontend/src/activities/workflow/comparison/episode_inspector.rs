//! Reuse the dataset's synchronized inspector for a comparison evidence row.
use crate::activities::data::{OpenedEpisode, screen::DataPanels};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

#[component]
pub(super) fn ComparisonEpisodeInspector(
    dataset: DatasetSelection,
    row: serde_json::Map<String, serde_json::Value>,
    on_back: EventHandler,
) -> Element {
    let episode = ComparisonEpisodeRow(row).open(dataset);
    rsx! {
        Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Link,
            onclick: move |_| on_back.call(()), "Back to comparison" }
        match episode {
            Ok(episode) => rsx! {
                DataPanels {
                    key: "{episode.selection().dataset().label()}:{episode.selection().episode_id()}",
                    initial_episode: episode,
                }
            },
            Err(message) => rsx! { div { class: "data-empty-copy", role: "alert", "{message}" } },
        }
    }
}

/// Comparison row identities are independent of their recorded episode IDs.
struct ComparisonEpisodeRow(serde_json::Map<String, serde_json::Value>);
impl ComparisonEpisodeRow {
    fn open(mut self, dataset: DatasetSelection) -> Result<OpenedEpisode, String> {
        let id = self
            .0
            .remove("episode_id")
            .ok_or("Comparison evidence has no episode identity.")?;
        self.0.insert("id".into(), id);
        OpenedEpisode::from_row(dataset, self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn comparison_evidence_preserves_episode_identity_and_timing() {
        let dataset = DatasetSelection::new(vec!["recordings".into()]).unwrap();
        let row = serde_json::json!({"id":"a", "source_uri":"spfsu://project/take-24.mcap", "message_start_time":1000,"duration_ns":2000}).as_object().unwrap().clone();
        let expected = OpenedEpisode::from_row(dataset.clone(), row.clone()).unwrap();
        let mut evidence = row;
        let id = evidence.remove("id").unwrap();
        evidence.insert("episode_id".into(), id);
        evidence.insert("id".into(), "comparison-row".into());
        let episode = ComparisonEpisodeRow(evidence)
            .open(dataset.clone())
            .unwrap();
        assert_eq!(episode, expected);
        assert_eq!(episode.selection().episode_id(), "a");
        assert_eq!(episode.title(), "a");
        assert!(
            ComparisonEpisodeRow(Default::default())
                .open(dataset)
                .is_err()
        );
    }
}
