//! What each episode was recorded doing, where its dataset records it: the
//! one source of an episode's task, which Break down tells its model.
use super::{EpisodeListing, StreamCatalog};
use spiral_robotics_common::datasets::DatasetProducer;

/// The topic Studio's recordings carry the operator's own words on.
const INSTRUCTION_TOPIC: &str = "/instruction";

/// Where a dataset records each episode's task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EpisodeTask {
    /// The first message an episode records on this topic.
    Recorded(String),
    /// The first task its dataset lists beside the episode: a LeRobot
    /// episode's `tasks`.
    Listed,
}

impl EpisodeTask {
    /// Where the episodes `listing` lists record their task, judged by the
    /// streams one of them records, since every episode of a dataset records
    /// the same; none for a dataset that records none.
    pub(in crate::activities) fn of(
        listing: &EpisodeListing,
        streams: &StreamCatalog,
    ) -> Option<Self> {
        match listing.producer() {
            DatasetProducer::LeRobot => Some(Self::Listed),
            DatasetProducer::Mcap => streams
                .streams()
                .iter()
                .any(|stream| stream.name() == INSTRUCTION_TOPIC)
                .then(|| Self::Recorded(INSTRUCTION_TOPIC.to_owned())),
        }
    }
    /// Where it is read from, as a settings note says it.
    pub(in crate::activities) fn note(&self) -> &'static str {
        match self {
            Self::Recorded(_) => "Each episode's task is read from the instruction it recorded.",
            Self::Listed => "Each episode's task is the one its dataset lists.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::DatasetSelection;

    #[test]
    fn a_lerobot_dataset_lists_its_task_and_a_recording_may_speak_it() {
        let lerobot = EpisodeListing::new(
            DatasetSelection::new(vec!["so101".into()]).unwrap(),
            DatasetProducer::LeRobot,
        );
        let silent = StreamCatalog::default();
        assert_eq!(
            EpisodeTask::of(&lerobot, &silent),
            Some(EpisodeTask::Listed)
        );
        assert_eq!(
            EpisodeTask::of(&EpisodeListing::mcap_fixture("bench"), &silent),
            None
        );
    }
}
