//! The takes this recording session produced: Live review of the recording
//! buffer and their camera previews.

use crate::OperationError;
use crate::projects::context::ProjectContext;
use spiral_robotics_common::datasets::EpisodeVideo;
use spiral_robotics_common::recording::{
    EpisodeInspectionArm, EpisodeInspectionSnapshot, RecordingCameraRequest,
};
use spiral_robotics_storage::episodes::{EpisodeId, EpisodeInspection};

pub(crate) struct EpisodeCatalog<'a> {
    state: &'a ProjectContext,
}

impl<'a> EpisodeCatalog<'a> {
    pub(super) fn new(state: &'a ProjectContext) -> Self {
        Self { state }
    }

    pub(super) fn inspection(
        &self,
        episode: String,
    ) -> Result<EpisodeInspectionSnapshot, OperationError> {
        let episode = Self::episode_id(&episode)?;
        self.state
            .recording_buffer()
            .inspection(episode.as_str())
            .map_err(OperationError::from)
            .map(InspectionProjection::into_snapshot)
    }

    pub(super) fn camera_preview(
        &self,
        episode: String,
        request: RecordingCameraRequest,
    ) -> Result<EpisodeVideo, OperationError> {
        let episode = Self::episode_id(&episode)?;
        self.state
            .recording_session()
            .camera_preview(&episode, request.stream())
            .ok_or_else(|| {
                OperationError::NotFound(
                    "This take is not available in the current recording session. Open the published dataset in Data.".into(),
                )
            })?
            .map_err(OperationError::InvalidInput)
    }

    fn episode_id(episode: &str) -> Result<String, OperationError> {
        EpisodeId::try_from(episode).map(String::from).map_err(|_| {
            OperationError::invalid_input(
                "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
            )
        })
    }
}

/// Projects a stored inspection into the wire snapshot.
///
/// Both types are foreign to this crate, so the row-major to frame-major
/// transposition hangs off the stored value as a local extension trait rather
/// than a `From` impl.
trait InspectionProjection {
    fn into_snapshot(self) -> EpisodeInspectionSnapshot;
}

impl InspectionProjection for EpisodeInspection {
    fn into_snapshot(self) -> EpisodeInspectionSnapshot {
        let inspection = self;
        let (episode, summary) = inspection.into_parts();
        let samples = episode.samples();
        let arms = episode
            .start()
            .arms()
            .iter()
            .enumerate()
            .map(|(arm_index, arm)| {
                let normalized = samples
                    .iter()
                    .flat_map(|sample| sample.normalized()[arm_index].iter().copied())
                    .collect();
                EpisodeInspectionArm::new(arm.id(), arm.joints().to_vec(), normalized)
            })
            .collect();
        EpisodeInspectionSnapshot::new(
            summary,
            samples.iter().map(|sample| sample.seconds()).collect(),
            arms,
        )
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
