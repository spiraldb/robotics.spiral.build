//! The episodes one annotation node reads, and what their recordings hold
//! that its step reads: the cameras, and the episode's task.
use crate::activities::curation::state::CurationController;
use crate::activities::episode::{Episode, EpisodeTask, StreamCatalog};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;

/// What one of a dataset's recordings holds that Break down reads: its
/// camera topics, and where it records its task. Every episode of one
/// dataset carries the same topics, so one episode answers for the sample.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RecordedTopics {
    cameras: Vec<String>,
    task: Option<EpisodeTask>,
}

impl RecordedTopics {
    /// What `episode` recorded: `None` until it is read, or when it cannot be.
    pub(super) fn use_topics(episode: Option<Episode>) -> Option<Self> {
        let topics = use_resource(use_reactive(&episode, move |episode| async move {
            let episode = episode?;
            let catalog = StreamCatalog::load(episode.selection()).await.ok()?;
            Some(Self {
                cameras: catalog
                    .videos()
                    .map(|stream| stream.name().to_owned())
                    .collect(),
                task: EpisodeTask::of(episode.listing(), &catalog),
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
    pub(super) fn task(&self) -> Option<EpisodeTask> {
        self.task.clone()
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
    pub(super) fn episode(&self) -> &Episode {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_episode_without_a_recorded_range_is_not_annotated() {
        let facts =
            serde_json::from_value(serde_json::json!({ "id": EpisodeId::fixture(2) })).unwrap();
        let listing = crate::activities::episode::EpisodeListing::mcap_fixture("folding");
        assert!(AnnotationEpisode::new(Episode::new(listing, facts)).is_err());
    }
}
