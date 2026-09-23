//! The episodes one annotation node walks: what the pass needs to sample them
//! and what the player needs to show them.
use crate::activities::curation::state::CurationController;
use crate::activities::data::DatasetClient;
use crate::activities::episode::{Episode, StreamCatalog};
use dioxus::prelude::*;
use serde_json::Value;
use spiral_robotics_common::datasets::{EpisodeId, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_common::sql::SqlLiteral;

/// The topic a dataset records the operator's own words on, when it has one.
const INSTRUCTION_TOPIC: &str = "/instruction";

/// One episode a pass annotates: an episode with a recorded span, since
/// camera packets are read over that span and timed from its start.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct AnnotationEpisode(Episode);

impl AnnotationEpisode {
    fn new(episode: Episode) -> Result<Self, String> {
        match episode.range() {
            Some(_) => Ok(Self(episode)),
            None => Err("This episode has no recorded time range.".into()),
        }
    }

    /// The episodes that reach `node`, in sample order: the curation's
    /// sample, less whatever a Select above the node left out. An episode
    /// without a recorded span cannot be sampled and is not offered.
    pub(super) fn reaching(controller: CurationController, node: &str) -> Vec<Self> {
        let (Some(sample), Some((input, _))) = (controller.sample(), controller.populations(node))
        else {
            return Vec::new();
        };
        input
            .episodes()
            .iter()
            .filter_map(|id| sample.episode(id).cloned())
            .filter_map(|episode| Self::new(episode).ok())
            .collect()
    }

    /// The camera topics the operator may annotate, read from one episode of
    /// the dataset: every episode of one recording session carries the same
    /// cameras, so the first one answers for the rest.
    pub(super) fn use_cameras(first: Option<Self>) -> Vec<String> {
        let cameras = use_resource(use_reactive(&first, move |first| async move {
            let Some(first) = first else {
                return Ok(Vec::new());
            };
            Ok::<_, String>(
                StreamCatalog::load(first.selection())
                    .await?
                    .videos()
                    .map(|stream| stream.read().topic().topic().to_owned())
                    .collect(),
            )
        }));
        let listed = cameras
            .read()
            .as_ref()
            .and_then(|loaded| loaded.as_ref().ok())
            .cloned();
        listed.unwrap_or_default()
    }

    /// What the operator asked the robot to do, when the dataset records it on
    /// an instruction topic. A dataset without one simply has no task text, so
    /// a missing topic or an unreadable row is silence rather than a failure.
    pub(super) async fn task(&self, datasets: &DatasetClient) -> Option<String> {
        let selection = self.selection();
        let statement = format!(
            "SELECT r.value.data AS instruction FROM spql.mcap.read_topic({}, {}) r \
             WHERE r.id = $1",
            SqlLiteral::text(&selection.dataset().sql_name()),
            SqlLiteral::text(INSTRUCTION_TOPIC)
        );
        let snapshot = datasets
            .run_episode_query(&EpisodeQueryRequest::new(selection, statement, 1))
            .await
            .ok()?;
        snapshot
            .rows()
            .first()?
            .get("instruction")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|task| !task.is_empty())
            .map(str::to_owned)
    }

    pub(super) fn id(&self) -> &EpisodeId {
        self.0.id()
    }
    pub(super) fn selection(&self) -> EpisodeSelection {
        self.0.selection()
    }
    /// Seconds from the first recorded message to the last.
    pub(super) fn duration(&self) -> f64 {
        self.0.facts().duration_seconds().unwrap_or_default()
    }
    pub(super) fn episode(&self) -> &Episode {
        &self.0
    }

    /// The fixture episode `ordinal`, of a fixed length, for tests that only
    /// need its identity and duration.
    #[cfg(test)]
    pub(super) fn fixture(ordinal: u32, duration_ns: u64) -> Self {
        Self::new(Episode::fixture(&EpisodeId::fixture(ordinal), duration_ns))
            .expect("a fixture has a recorded span")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_episode_takes_its_identity_and_length_from_the_recorded_bounds() {
        let episode = AnnotationEpisode::fixture(1, 20_500_000_000);
        assert_eq!(episode.id(), &EpisodeId::fixture(1));
        assert!((episode.duration() - 20.5).abs() < 1e-9);
        assert_eq!(episode.episode().name(), "Episode …00000001");
    }

    #[test]
    fn an_episode_without_a_recorded_range_is_not_annotated() {
        let facts =
            serde_json::from_value(serde_json::json!({ "id": EpisodeId::fixture(2) })).unwrap();
        let dataset =
            spiral_robotics_common::datasets::DatasetSelection::new(vec!["folding".into()])
                .unwrap();
        assert!(AnnotationEpisode::new(Episode::new(dataset, facts)).is_err());
    }
}
