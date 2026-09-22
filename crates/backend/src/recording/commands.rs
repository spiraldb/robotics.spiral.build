//! Recording application operations.
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};

use crate::projects::context::ProjectContext;
use crate::recording::catalog::{DeleteEpisodeCommand, EpisodeCatalogActions, EpisodeCatalogPort};
use crate::recording::queries::EpisodeQueries;
use crate::recording::take::{TakeOperations, TakeRequest};
use crate::{BackendCommandError, OperationError};
use spiral_robotics_common::recording::{EpisodeInfo, EpisodeInspectionSnapshot};

/// SpiralDB and control-state adapter for episode catalog actions.
struct EpisodeAdapter<'a> {
    state: &'a ProjectContext,
}

impl<'a> EpisodeAdapter<'a> {
    fn new(state: &'a ProjectContext) -> Self {
        Self { state }
    }
}

impl EpisodeCatalogPort for EpisodeAdapter<'_> {
    fn recording_episode(&self) -> Option<String> {
        self.state
            .hardware()
            .state()
            .recording()
            .map(|recording| recording.episode().to_owned())
    }

    async fn delete_episode(&self, episode: String) -> Result<(), OperationError> {
        let summary = self.state.recording_buffer().get(&episode)?;
        self.state
            .storage()
            .remove_mcap_file(summary.dataset_id(), &format!("{}.mcap", summary.id()))
            .await
            .map_err(OperationError::Internal)?;
        self.state.recording_buffer().forget_completed(&episode);
        self.state.recording_session().forget_preview(&episode);
        Ok(())
    }
}

/// Project-bound operations; the context owns backend task lifetime.
#[derive(Clone)]
pub(crate) struct RecordingCommands {
    state: ProjectContext,
}

impl RecordingCommands {
    pub(crate) fn recording_signals(
        &self,
        cursor: RecordingCursor,
    ) -> Result<RecordingWindow, BackendCommandError> {
        Ok(self.state.hardware().recording_signals(cursor))
    }

    /// Begin a take using the configured teleoperation pair.
    pub(super) async fn start_recording(
        &self,
        request: spiral_robotics_common::recording::RecordingStart,
    ) -> Result<EpisodeInfo, BackendCommandError> {
        let (dataset_id, (leader, follower)) = request.into_parts();
        self.state
            .execute(move |state| async move {
                state
                    .storage()
                    .validate_mcap_destination(&dataset_id)
                    .await?;
                let summary = TakeOperations::new(&state)
                    .start(TakeRequest::new(dataset_id, (leader, follower)))
                    .await?;
                let episode = summary.id().to_owned();
                tokio::spawn(async move {
                    TakeOperations::new(&state)
                        .close_when_the_control_loop_ends(&episode)
                        .await;
                });
                Ok(summary)
            })
            .await
    }

    pub(super) async fn stop_recording(
        &self,
        keep: bool,
    ) -> Result<EpisodeInfo, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                TakeOperations::new(&state)
                    .stop(keep)
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn set_recording_paused(
        &self,
        paused: bool,
    ) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                TakeOperations::new(&state)
                    .set_paused(paused)
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn delete_episode(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                EpisodeCatalogActions::new(&EpisodeAdapter::new(&state))
                    .delete(DeleteEpisodeCommand::new(id))
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn inspection(
        &self,
        id: String,
    ) -> Result<EpisodeInspectionSnapshot, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                EpisodeQueries::new(&state)
                    .inspection(id)
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn camera_preview(
        &self,
        id: String,
        request: spiral_robotics_common::recording::RecordingCameraRequest,
    ) -> Result<spiral_robotics_common::datasets::EpisodeVideo, BackendCommandError> {
        spiral_robotics_storage::episodes::EpisodeId::try_from(id.as_str())
            .map_err(|error| OperationError::InvalidInput(error.to_string()))?;
        self.state.recording_session().camera_preview(&id, request.stream())
            .ok_or_else(|| OperationError::NotFound("This take is not available in the current recording session. Open the published dataset in Data.".into()))?
            .map_err(|error| OperationError::InvalidInput(error).into())
    }
}

impl From<crate::projects::context::ProjectContext> for RecordingCommands {
    fn from(state: crate::projects::context::ProjectContext) -> Self {
        Self { state }
    }
}
