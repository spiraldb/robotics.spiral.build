//! What each episode was recorded doing, where its dataset records it: the
//! one source of an episode's task, which Break down tells its model.
use super::{EpisodeListing, StreamCatalog};
use spiral_robotics_common::datasets::{DatasetFormat, EpisodeTask};

/// The topic Studio's recordings carry the operator's own words on.
const INSTRUCTION_TOPIC: &str = "/instruction";

impl EpisodeListing {
    /// Where the episodes it lists record their task, judged by `streams`,
    /// the ones one of them records, since every episode of a dataset
    /// records the same; none for a dataset that records none.
    pub(in crate::activities) fn task(&self, streams: &StreamCatalog) -> Option<EpisodeTask> {
        match self.format() {
            DatasetFormat::LeRobot => Some(EpisodeTask::Listed),
            DatasetFormat::Mcap => streams
                .streams()
                .iter()
                .any(|stream| stream.name() == INSTRUCTION_TOPIC)
                .then(|| EpisodeTask::Recorded(INSTRUCTION_TOPIC.to_owned())),
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
            DatasetSelection::new("so101").unwrap(),
            DatasetFormat::LeRobot,
        );
        let silent = StreamCatalog::default();
        assert_eq!(lerobot.task(&silent), Some(EpisodeTask::Listed));
        assert_eq!(EpisodeListing::mcap_fixture("bench").task(&silent), None);
    }
}
