//! The episodes one annotation node reads, and what their recordings hold
//! that its step reads: the cameras, and the operator's instruction.
use crate::activities::curation::state::CurationController;
use crate::activities::episode::{Episode, StreamCatalog};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{EpisodeId, EpisodeSelection};

/// The topic a dataset records the operator's own words on, when it has one.
const INSTRUCTION_TOPIC: &str = "/instruction";

/// What one of a dataset's recordings holds that Break down reads: its
/// camera topics, and whether it records the operator's instruction. Every
/// episode of one recording session carries the same topics, so one episode
/// answers for the sample.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RecordedTopics {
    cameras: Vec<String>,
    instructed: bool,
}

impl RecordedTopics {
    /// What `episode` recorded: `None` until it is read, or when it cannot be.
    pub(super) fn use_topics(episode: Option<EpisodeSelection>) -> Option<Self> {
        let topics = use_resource(use_reactive(&episode, move |episode| async move {
            let catalog = StreamCatalog::load(episode?).await.ok()?;
            Some(Self {
                cameras: catalog
                    .videos()
                    .map(|stream| stream.name().to_owned())
                    .collect(),
                instructed: catalog
                    .streams()
                    .iter()
                    .any(|stream| stream.name() == INSTRUCTION_TOPIC),
            })
        }));
        topics.read().clone().flatten()
    }
    pub(super) fn cameras(&self) -> Vec<String> {
        self.cameras.clone()
    }
    /// The camera that shows the whole workspace best.
    pub(super) fn overview_camera(&self) -> Option<String> {
        StreamCatalog::overview_camera(&self.cameras).map(str::to_owned)
    }
    /// Where each episode's task is read, when the dataset records one.
    pub(super) fn task_topic(&self) -> Option<String> {
        self.instructed.then(|| INSTRUCTION_TOPIC.to_owned())
    }
}

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

    pub(super) fn id(&self) -> &EpisodeId {
        self.0.id()
    }
    /// Seconds from the first recorded message to the last.
    #[cfg(test)]
    fn duration(&self) -> f64 {
        self.0.facts().duration_seconds().unwrap_or_default()
    }
    pub(super) fn episode(&self) -> &Episode {
        &self.0
    }

    /// The fixture episode `ordinal`, of a fixed length, for tests that only
    /// need its identity and duration.
    #[cfg(test)]
    fn fixture(ordinal: u32, duration_ns: u64) -> Self {
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
