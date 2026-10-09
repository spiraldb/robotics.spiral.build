//! The review of the last episode, paused or stopped: its joint positions and
//! camera frames, read back from its file.

use crate::OperationError;
use crate::context::RobotConnectorContext;
use spiral_robotics_common::datasets::EpisodeVideo;
use spiral_robotics_common::recording::{EpisodeInspectionSnapshot, RecordingCameraRequest};
use spiral_robotics_recording::{EpisodeFileReader, EpisodeId, EpisodeSummary};

pub(crate) struct EpisodeCatalog<'a> {
    state: &'a RobotConnectorContext,
}

impl<'a> EpisodeCatalog<'a> {
    pub(super) fn new(state: &'a RobotConnectorContext) -> Self {
        Self { state }
    }

    pub(super) async fn inspection(
        &self,
        episode: String,
    ) -> Result<EpisodeInspectionSnapshot, OperationError> {
        let (summary, reader) = self.review(&episode)?;
        tokio::task::spawn_blocking(move || reader.inspection(&summary))
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?
            .map_err(OperationError::Internal)
    }

    pub(super) async fn camera_preview(
        &self,
        episode: String,
        request: RecordingCameraRequest,
    ) -> Result<EpisodeVideo, OperationError> {
        let (summary, reader) = self.review(&episode)?;
        tokio::task::spawn_blocking(move || reader.camera(&summary, request.stream()))
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?
            .map_err(OperationError::InvalidInput)
    }

    /// The last episode, paused or stopped, and its file, when it is `episode`.
    fn review(&self, episode: &str) -> Result<(EpisodeSummary, EpisodeFileReader), OperationError> {
        let episode = Self::episode_id(episode)?;
        let unavailable = || {
            OperationError::NotFound(
                "This episode is not available in the current recording session. Open its dataset in Data.".into(),
            )
        };
        let summary = self
            .state
            .recording_buffer()
            .summary(&episode)
            .map_err(|_| unavailable())?;
        let path = self
            .state
            .recording_files()
            .review_path(summary.info().dataset_id(), &episode)
            .ok_or_else(unavailable)?;
        Ok((summary, EpisodeFileReader::new(path)))
    }

    fn episode_id(episode: &str) -> Result<String, OperationError> {
        EpisodeId::try_from(episode).map(String::from).map_err(|_| {
            OperationError::invalid_input(
                "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_requires_exact_episode_identities() {
        assert_eq!(
            EpisodeCatalog::episode_id("episode_000001")
                .unwrap()
                .as_str(),
            "episode_000001"
        );
        assert!(matches!(
            EpisodeCatalog::episode_id(" episode"),
            Err(OperationError::InvalidInput(_))
        ));
        for invalid in ["", "../episode", "episode name", "episode/"] {
            assert!(matches!(
                EpisodeCatalog::episode_id(invalid),
                Err(OperationError::InvalidInput(_))
            ));
        }
    }
}
