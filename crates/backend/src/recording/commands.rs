//! Recording and replay application operations.
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};

use crate::projects::context::ProjectContext;
use crate::recording::catalog::{
    DeleteEpisodeCommand, EpisodeActivity, EpisodeCatalogActions, EpisodeCatalogPort,
};
use crate::recording::queries::EpisodeQueries;
use crate::recording::replay::StartReplayCommand;
use crate::recording::seek::{
    ActiveRecording, PhysicalSeekActions, PhysicalSeekPlan, PhysicalSeekPort, RecordedSample,
    SampledArm, SeekActivity, SeekControlMode, SeekEpisodeCommand, SeekTarget, SeekableEpisode,
};
use crate::recording::take::{TakeOperations, TakeRequest};
use crate::{BackendCommandError, OperationError};
use spiral_robotics_common::recording::{EpisodeInfo, EpisodeInspectionSnapshot};
use spiral_robotics_hardware::ModeSelection as HardwareModeSelection;

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
    fn activity(&self) -> EpisodeActivity {
        let hardware = self.state.hardware().state();
        EpisodeActivity::new(
            hardware
                .recording()
                .as_ref()
                .map(|recording| recording.episode().to_owned()),
            match hardware.mode() {
                HardwareModeSelection::Replay { episode, .. } => Some(episode.clone()),
                _ => None,
            },
        )
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

/// Adapter for recorded physical seeks. The current-ticket check and motor
/// handoff share the store's serialization gate, so a coalesced result proves
/// no stale pose reached hardware.
struct SeekAdapter<'a> {
    state: &'a ProjectContext,
}

impl PhysicalSeekPort for SeekAdapter<'_> {
    fn activity(&self) -> SeekActivity {
        let hardware = self.state.hardware().state();
        SeekActivity::new(
            hardware
                .recording()
                .as_ref()
                .map(|recording| ActiveRecording::new(recording.episode(), recording.paused())),
            match hardware.mode() {
                HardwareModeSelection::Idle => SeekControlMode::Idle,
                HardwareModeSelection::Replay { .. } => SeekControlMode::Replay,
                _ => SeekControlMode::Other,
            },
        )
    }

    async fn target(&self, robot: &str) -> Result<Option<SeekTarget>, OperationError> {
        let record = match self.state.workspace().get(robot).await {
            Ok(record) => record,
            Err(crate::robots::workspace::WorkspaceError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let connection = self
            .state
            .hardware()
            .state()
            .connections()
            .get(robot)
            .cloned()
            .unwrap_or_default();
        Ok(Some(
            SeekTarget::new(
                record.id(),
                record.config().robot_type().map(str::to_owned),
                connection.connected(),
            )
            .with_calibration(record.config().calibrated()),
        ))
    }

    async fn episode(&self, episode: &str) -> Result<SeekableEpisode, OperationError> {
        self.state
            .recording_buffer()
            .get(episode)
            .map(|summary| {
                SeekableEpisode::new(
                    summary.id(),
                    summary
                        .arms()
                        .iter()
                        .map(|arm| arm.id().to_owned())
                        .collect(),
                    summary.sample_hz(),
                )
                .with_state(summary.problem().map(str::to_owned), false)
            })
            .map_err(OperationError::from)
    }

    fn reserve_seek(&self, robot: &str) -> u64 {
        self.state.recording_buffer().reserve_seek(robot)
    }

    async fn sample_at(
        &self,
        episode: &str,
        seconds: f64,
    ) -> Result<RecordedSample, OperationError> {
        self.state
            .recording_buffer()
            .sample_at(episode, seconds)
            .map(|sample| {
                let (frame, seconds, _, arms) = sample.into_parts();
                RecordedSample::new(
                    seconds,
                    frame,
                    arms.into_iter()
                        .map(|arm| {
                            let (id, normalized, ..) = arm.into_parts();
                            SampledArm::new(id, normalized)
                        })
                        .collect(),
                )
            })
            .map_err(OperationError::from)
    }

    async fn commit_if_current(&self, plan: PhysicalSeekPlan) -> Result<bool, OperationError> {
        let (ticket, robot, episode, pose, sample_hz) = plan.into_parts();
        let _gate = self.state.recording_buffer().lock_seek().await;
        if !self
            .state
            .recording_buffer()
            .seek_is_current(&robot, ticket)
        {
            return Ok(false);
        }
        let trajectory = spiral_robotics_hardware::ReplayTrajectory::single_pose(pose, sample_hz)
            .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        let request = spiral_robotics_hardware::RobotPlaybackRequest::physical_seek(
            robot, episode, trajectory,
        )
        .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        self.state
            .hardware()
            .start_replay(request)
            .await
            .map_err(OperationError::conflict)?;
        Ok(true)
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

    /// Move a physical follower to one committed recorded sample.
    pub(super) async fn seek_episode(
        &self,
        id: String,
        robot: String,
        source_arm: Option<String>,
        seconds: f64,
    ) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                PhysicalSeekActions::new(&SeekAdapter { state: &state })
                    .seek(SeekEpisodeCommand::new(id, robot, seconds).with_source_arm(source_arm))
                    .await?;
                Ok(())
            })
            .await
    }

    pub(super) async fn start_replay(
        &self,
        command: StartReplayCommand,
    ) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move { command.run(&state).await.map_err(Into::into) })
            .await
    }

    pub(super) async fn set_replay_paused(&self, paused: bool) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state
                    .hardware()
                    .set_replay_paused(paused)
                    .await
                    .map_err(|error| OperationError::conflict(error).into())
            })
            .await
    }
}

impl From<crate::projects::context::ProjectContext> for RecordingCommands {
    fn from(state: crate::projects::context::ProjectContext) -> Self {
        Self { state }
    }
}
