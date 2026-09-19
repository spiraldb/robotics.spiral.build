//! Recording and replay application operations.
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};

use std::collections::BTreeSet;
#[cfg(test)]
use std::sync::Arc;

use crate::data::catalog_lease::CatalogLeaseToken;
#[cfg(test)]
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogMutationLease};
use crate::project_session::context::ProjectContext;
use crate::recording::catalog::{
    DeleteEpisodeCommand, EpisodeActivity, EpisodeCatalogActions, EpisodeCatalogPort,
};
use crate::recording::queries::EpisodeQueries;
use crate::recording::seek::{
    ActiveRecording, PhysicalSeekActions, PhysicalSeekPlan, PhysicalSeekPort, RecordedSample,
    SampledArm, SeekActivity, SeekControlMode, SeekEpisodeCommand, SeekTarget, SeekableEpisode,
};
use crate::recording::{
    RecordingActions, RecordingContext, RecordingDraftRequest, RecordingPort, RecordingResult,
    ReplayActions, ReplayEpisode, ReplayPlan, ReplayPort, ReplayTrajectory,
    ReplayTrajectoryRequest, StartRecordingCommand, StartReplayCommand, TeleoperationPair,
};
use crate::robotics::video::{
    CameraCaptureSpec, CameraRecording, VideoCapture, VideoCaptureRequest,
};
use crate::{BackendCommandError, OperationError};
use spiral_robotics_common::recording::{EpisodeInfo, EpisodeInspectionSnapshot};
use spiral_robotics_hardware::ModeSelection as HardwareModeSelection;
use spiral_robotics_storage::episodes::{
    ArmCondition, EpisodeArmCollection as _, EpisodeRecordingRequest, EpisodeWriter,
    JointMotorLimits, RobotPlaybackRequest,
};

/// Concrete recording/replay adapter kept at the backend edge.
///
/// Opening a take still assembles telemetry-derived arm layouts and camera
/// specifications here because those are concrete backend details. Session
/// validation, consequential ordering, and compensation live in the
/// framework-neutral capability action.
struct RecordingAdapter<'a> {
    state: &'a ProjectContext,
    clock: std::sync::Mutex<Option<spiral_robotics_hardware::CaptureClock>>,
    presented_recording: std::sync::Mutex<Option<EpisodeInfo>>,
}

impl<'a> RecordingAdapter<'a> {
    fn new(state: &'a ProjectContext) -> Self {
        Self {
            state,
            clock: std::sync::Mutex::new(None),
            presented_recording: std::sync::Mutex::new(None),
        }
    }

    fn present_recording(&self, result: RecordingResult) -> Result<EpisodeInfo, OperationError> {
        let summary = self
            .presented_recording
            .lock()
            .map_err(|_| OperationError::Internal("recording presenter lock poisoned".to_owned()))?
            .take()
            .ok_or_else(|| {
                OperationError::Internal("recording result was not presented".to_owned())
            })?;
        if summary.id() != result.episode() {
            return Err(OperationError::Internal(
                "recording presenter returned a different episode".to_owned(),
            ));
        }
        Ok(summary)
    }
}

struct RecordingDraft {
    writer: EpisodeWriter,
    arms: Vec<spiral_robotics_common::recording::EpisodeArm>,
    summary: EpisodeInfo,
}

impl RecordingPort for RecordingAdapter<'_> {
    type RecordingDraft = RecordingDraft;
    type MediaCapture = VideoCapture;
    type MediaOutput = Vec<CameraRecording>;
    type Finalization = crate::recording_session::RecordingOperation;

    type CaptureChange = crate::recording_session::RecordingOperation;

    fn reserve_recording_start(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
        self.state
            .recording_session()
            .reserve_start(|| self.state.workflow().is_active())
    }

    fn bind_recording_lease(&self, token: CatalogLeaseToken, take: (String, Option<VideoCapture>)) {
        self.state.recording_session().begin(token, take);
    }

    fn claim_finalization(&self) -> Result<Option<Self::Finalization>, OperationError> {
        self.state.recording_session().claim_finalization()
    }

    fn finalization_episode(&self, operation: &Self::Finalization) -> String {
        operation.episode().to_owned()
    }

    fn complete_recording(&self, operation: Self::Finalization, media: Self::MediaOutput) {
        operation.complete(media);
    }

    fn release_recording_lease(&self, token: CatalogLeaseToken) {
        self.state.recording_session().cancel_start(token);
    }

    fn recording_active(&self) -> bool {
        self.state.hardware().state().recording().is_some()
    }

    fn recording_context(&self) -> RecordingContext {
        let hardware = self.state.hardware().state();
        let connected_arms = hardware
            .robots()
            .iter()
            .filter(|robot| {
                hardware
                    .connections()
                    .get(robot.id())
                    .is_some_and(|connection| connection.connected())
            })
            .map(|robot| robot.id().to_owned())
            .collect();
        let sample_hz = if hardware.target_hz() > 0.0 {
            (hardware.target_hz() * 1_000.0).round() / 1_000.0
        } else {
            spiral_robotics_hardware::CONTROL_HZ as f64
        };
        RecordingContext::new(connected_arms, sample_hz)
    }

    async fn enter_teleoperation(&self, pair: &TeleoperationPair) -> Result<(), OperationError> {
        self.state
            .hardware()
            .set_mode(HardwareModeSelection::Teleoperate {
                leader: pair.leader().to_owned(),
                follower: pair.follower().to_owned(),
            })
            .await
            .map_err(OperationError::conflict)
    }

    async fn enter_idle(&self) -> Result<(), OperationError> {
        self.state
            .hardware()
            .set_mode(HardwareModeSelection::Idle)
            .await
            .map_err(OperationError::conflict)
    }

    async fn create_recording_draft(
        &self,
        request: RecordingDraftRequest,
    ) -> Result<RecordingDraft, OperationError> {
        self.assemble_draft(request).await
    }

    fn draft_episode(&self, draft: &Self::RecordingDraft) -> String {
        draft.summary.id().to_owned()
    }

    async fn start_recording_media(
        &self,
        episode: &str,
        arms: &[String],
    ) -> Result<Option<Self::MediaCapture>, OperationError> {
        self.assemble_media(episode, arms).await
    }

    async fn begin_recording(
        &self,
        draft: Self::RecordingDraft,
    ) -> Result<RecordingResult, OperationError> {
        self.state
            .hardware()
            .start_recording(draft.writer, draft.arms)
            .await
            .map_err(OperationError::conflict)?;
        let result = RecordingResult::new(draft.summary.id());
        *self.presented_recording.lock().map_err(|_| {
            OperationError::Internal("recording presenter lock poisoned".to_owned())
        })? = Some(draft.summary);
        Ok(result)
    }

    async fn abandon_recording_media(&self, capture: Self::MediaCapture) {
        let _ = capture.finish().await;
    }

    async fn stop_recording(&self, keep: bool) -> Result<RecordingResult, OperationError> {
        let completion = self
            .state
            .hardware()
            .stop_recording(keep)
            .await
            .map_err(OperationError::conflict)?;
        let summary = self
            .state
            .recording_buffer()
            .completed_episode(completion.episode())
            .map(|episode| episode.summary())
            .map_err(OperationError::from)?;
        let result = RecordingResult::new(completion.episode());
        *self.presented_recording.lock().map_err(|_| {
            OperationError::Internal("recording presenter lock poisoned".to_owned())
        })? = Some(summary);
        Ok(result)
    }

    async fn finish_recording_media(
        &self,
        operation: &mut Self::Finalization,
    ) -> Self::MediaOutput {
        let (videos, failures) = operation.finish_media().await;
        for failure in failures {
            self.state.cameras().note_issue(failure);
        }
        videos
    }

    async fn commit_recording(
        &self,
        episode: String,
        media: &Self::MediaOutput,
    ) -> Result<(), OperationError> {
        let full_episode = self
            .state
            .recording_buffer()
            .completed_episode(&episode)
            .map_err(OperationError::from)?;
        self.state
            .storage()
            .commit_recording(&full_episode, media)
            .await
            .map_err(OperationError::Internal)?;
        Ok(())
    }

    fn claim_capture_change(&self) -> Result<Self::CaptureChange, OperationError> {
        self.state.recording_session().claim_capture_change()
    }

    fn request_media_pause(&self, operation: &Self::CaptureChange) {
        operation.request_pause();
    }

    async fn set_recording_paused(&self, paused: bool) -> Result<f64, OperationError> {
        self.state
            .hardware()
            .set_recording_paused(paused)
            .await
            .map_err(OperationError::conflict)
    }

    async fn finish_media_pause(
        &self,
        operation: &mut Self::CaptureChange,
    ) -> Result<(), OperationError> {
        let (_, failures) = operation.finish_media().await;
        for failure in failures {
            self.state.cameras().note_issue(failure);
        }
        // The writer's pause checkpoint remains in this recording session.
        // Only Stop publishes the finalized MCAP as a Dataset member.
        Ok(())
    }

    fn resume_media(&self, operation: &mut Self::CaptureChange, active_seconds: f64) {
        if let Err(error) = operation.resume(active_seconds) {
            self.state
                .cameras()
                .note_issue(format!("episode video could not resume: {error}"));
        }
    }
}

impl ReplayPort for RecordingAdapter<'_> {
    async fn replay_episode(&self, episode: &str) -> Result<ReplayEpisode, OperationError> {
        self.state
            .recording_buffer()
            .get(episode)
            .map(|summary| {
                ReplayEpisode::new(
                    summary.id(),
                    summary
                        .arms()
                        .iter()
                        .map(|arm| arm.id().to_owned())
                        .collect(),
                    summary.sample_hz(),
                )
            })
            .map_err(OperationError::from)
    }

    async fn load_trajectory(
        &self,
        request: ReplayTrajectoryRequest<'_>,
    ) -> Result<ReplayTrajectory, OperationError> {
        let episode = request.episode().to_owned();
        let source_arm = request.source_arm().to_owned();
        let start_seconds = request.start_seconds();
        self.state
            .recording_buffer()
            .trajectory_at(
                &episode,
                RobotPlaybackRequest::new(&source_arm, start_seconds),
            )
            .map(|trajectory| {
                let start_frame = trajectory.start_frame();
                let resolved_start_seconds = trajectory.resolved_start_seconds();
                ReplayTrajectory::new(trajectory.into_frames(), start_frame)
                    .starting_at(resolved_start_seconds)
            })
            .map_err(OperationError::from)
    }

    async fn start_replay(&self, plan: ReplayPlan) -> Result<(), OperationError> {
        let (robot, episode, frames, sample_hz, start_frame, resolved_start_seconds) =
            plan.into_parts();
        let trajectory = spiral_robotics_hardware::ReplayTrajectory::new(frames, sample_hz)
            .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        let request =
            spiral_robotics_hardware::RobotPlaybackRequest::timeline(robot, episode, trajectory)
                .and_then(|request| request.starting_at(start_frame, resolved_start_seconds))
                .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        self.state
            .hardware()
            .start_replay(request)
            .await
            .map_err(OperationError::conflict)
    }

    async fn set_replay_paused(&self, paused: bool) -> Result<(), OperationError> {
        self.state
            .hardware()
            .set_replay_paused(paused)
            .await
            .map_err(OperationError::conflict)
    }
}

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
        let target = record.summary(connection);
        Ok(Some(
            SeekTarget::new(
                target.id(),
                target.robot_type().map(str::to_owned),
                target.connection().connected(),
            )
            .with_readiness(target.calibrated(), target.problem().map(str::to_owned)),
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

/// Project-bound operations; the context owns admission and backend task lifetime.
#[derive(Clone)]
pub(crate) struct RecordingCommands {
    state: ProjectContext,
}

impl RecordingCommands {
    pub(crate) fn recording_signals(
        &self,
        cursor: RecordingCursor,
    ) -> Result<RecordingWindow, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.hardware().recording_signals(cursor))
    }

    /// Begin a take using the configured teleoperation pair.
    pub(super) async fn start_recording(
        &self,
        request: spiral_robotics_common::recording::RecordingStart,
    ) -> Result<EpisodeInfo, BackendCommandError> {
        let (dataset_id, (leader, follower)) = request.into_parts();
        self.state
            .execute(move |state, _activity| async move {
                state.require_catalog_ready()?;
                state.preferences().load().await?;
                state
                    .storage()
                    .validate_mcap_destination(&dataset_id)
                    .await?;
                let adapter = RecordingAdapter::new(&state);
                let result = RecordingActions::new(&adapter)
                    .start(StartRecordingCommand::new(
                        dataset_id,
                        TeleoperationPair::new(leader, follower),
                    ))
                    .await?;
                adapter.present_recording(result).map_err(Into::into)
            })
            .await
    }

    pub(super) async fn stop_recording(
        &self,
        keep: bool,
    ) -> Result<EpisodeInfo, BackendCommandError> {
        self.state
            .execute(move |state, _activity| async move {
                state.require_catalog_ready()?;
                let adapter = RecordingAdapter::new(&state);
                let result = RecordingActions::new(&adapter).stop(keep).await?;
                adapter.present_recording(result).map_err(Into::into)
            })
            .await
    }

    pub(super) async fn set_recording_paused(
        &self,
        paused: bool,
    ) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state, _activity| async move {
                state.require_catalog_ready()?;
                RecordingActions::new(&RecordingAdapter::new(&state))
                    .set_paused(paused)
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn delete_episode(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state, _activity| async move {
                state.require_catalog_ready()?;
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
            .execute(move |state, _activity| async move {
                state.require_catalog_ready()?;
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
        let _activity = self.state.admit()?;
        self.state.require_catalog_ready()?;
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
            .execute(move |state, _activity| async move {
                state.require_catalog_ready()?;
                PhysicalSeekActions::new(&SeekAdapter { state: &state })
                    .seek(SeekEpisodeCommand::new(id, robot, seconds).with_source_arm(source_arm))
                    .await?;
                Ok(())
            })
            .await
    }

    pub(super) async fn start_replay(
        &self,
        episode: String,
        robot: String,
        source_arm: Option<String>,
        start_seconds: f64,
    ) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state, _activity| async move {
                state.require_catalog_ready()?;
                ReplayActions::new(&RecordingAdapter::new(&state))
                    .start(
                        StartReplayCommand::new(episode, robot, source_arm)
                            .starting_at(start_seconds),
                    )
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(super) async fn set_replay_paused(&self, paused: bool) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state, _activity| async move {
                state.require_catalog_ready()?;
                ReplayActions::new(&RecordingAdapter::new(&state))
                    .set_paused(paused)
                    .await
                    .map_err(Into::into)
            })
            .await
    }
}

impl RecordingAdapter<'_> {
    /// Assemble a storage draft from authoritative live telemetry.
    ///
    /// Eligibility, sample-rate admission, consequential ordering, and cleanup
    /// belong to the capability action. This adapter only translates live backend
    /// structures into the episode writer's concrete representation.
    async fn assemble_draft(
        &self,
        request: RecordingDraftRequest,
    ) -> Result<RecordingDraft, OperationError> {
        let (dataset_id, source, wanted, sample_hz) = request.into_parts();
        let hardware = self.state.hardware().state();
        let connected = hardware
            .robots()
            .iter()
            .filter(|robot| {
                hardware
                    .connections()
                    .get(robot.id())
                    .is_some_and(|connection| connection.connected())
            })
            .collect::<Vec<_>>();

        let arms = wanted
            .iter()
            .map(|id| {
                let robot = connected
                    .iter()
                    .find(|robot| robot.id() == id)
                    .ok_or_else(|| {
                        OperationError::Conflict(format!(
                            "robot {id:?} disconnected while the recording was opening"
                        ))
                    })?;
                Ok((
                    id.clone(),
                    robot
                        .joints()
                        .iter()
                        .map(|joint| joint.name().to_owned())
                        .collect::<Vec<_>>(),
                ))
            })
            .collect::<Result<Vec<_>, OperationError>>()?;
        let saved_robots = self.state.workspace().list().await;
        let arms = spiral_robotics_common::recording::EpisodeArm::from_joint_lists(&arms)
            .map_err(OperationError::from)?
            .into_iter()
            .map(|arm| {
                let robot_type = saved_robots
                    .iter()
                    .find(|robot| robot.id() == arm.id())
                    .cloned()
                    .and_then(|robot| robot.robot_type());
                spiral_robotics_common::recording::EpisodeArm::new(
                    arm.id(),
                    robot_type,
                    arm.joints().to_vec(),
                )
            })
            .collect::<Vec<_>>();

        // What the motors were like as the take opened. Recorded once here rather
        // than on every frame: these move over seconds, and the footer's extremes
        // close the story the header starts.
        let condition = arms
            .iter()
            .filter_map(|arm| {
                let robot = connected.iter().find(|robot| robot.id() == arm.id())?;
                let limits = hardware.limits().get(arm.id());
                let joint = |name: &str| robot.joints().iter().find(|joint| joint.name() == name);
                Some(
                    ArmCondition::new(arm.id().to_owned())
                        .with_measurements(
                            arm.joints()
                                .iter()
                                .map(|name| {
                                    joint(name).and_then(|joint| joint.temperature_celsius())
                                })
                                .collect(),
                            arm.joints()
                                .iter()
                                .map(|name| joint(name).and_then(|joint| joint.voltage_volts()))
                                .collect(),
                        )
                        .with_limits(
                            arm.joints()
                                .iter()
                                .map(|name| {
                                    limits
                                        .and_then(|limits| limits.get(name))
                                        .map(JointMotorLimits::from)
                                        .unwrap_or_default()
                                })
                                .collect(),
                        ),
                )
            })
            .collect();

        let writer = self
            .state
            .recording_buffer()
            .begin(
                EpisodeRecordingRequest::new(dataset_id.clone(), source.clone(), sample_hz)
                    .with_arms(arms.clone(), condition),
            )
            .map_err(OperationError::from)?;
        *self
            .clock
            .lock()
            .map_err(|_| OperationError::Internal("Recording clock lock poisoned".into()))? =
            Some(writer.clock());
        let id = writer.id().to_owned();
        let started_ms = writer.started_ms();
        let summary = EpisodeInfo::new(id, dataset_id)
            .with_arms(arms.clone())
            .with_source(source)
            .with_sample_hz(sample_hz)
            .with_started_ms(started_ms);
        Ok(RecordingDraft {
            writer,
            arms,
            summary,
        })
    }

    /// Assemble camera capture for an already-created episode draft.
    async fn assemble_media(
        &self,
        episode: &str,
        wanted: &[String],
    ) -> Result<Option<VideoCapture>, OperationError> {
        if self
            .state
            .preferences()
            .load()
            .await
            .map_err(OperationError::External)?
            .video()
            .enabled()
        {
            let records = self
                .state
                .workspace()
                .try_list()
                .await
                .map_err(OperationError::from)?;
            let statuses = self.state.cameras().statuses();
            let camera_status = statuses
                .iter()
                .map(|camera| (camera.id(), camera))
                .collect::<std::collections::BTreeMap<_, _>>();
            let mut seen = BTreeSet::new();
            let mut cameras = Vec::new();
            for robot in records
                .iter()
                .filter(|robot| wanted.iter().any(|id| id == robot.id()))
            {
                for camera in robot.cameras() {
                    if !seen.insert(camera.id().to_owned()) {
                        continue;
                    }
                    let Some(status) = camera_status.get(camera.id()) else {
                        continue;
                    };
                    cameras.push(CameraCaptureSpec::new(
                        format!("{}.camera.{}", robot.id(), camera.name()),
                        camera.id(),
                        (status.width(), status.height(), status.fps()),
                    ));
                }
            }
            let clock = self
                .clock
                .lock()
                .map_err(|_| OperationError::Internal("Recording clock lock poisoned".into()))?
                .clone()
                .ok_or_else(|| OperationError::Internal("Recording clock is missing".into()))?;
            VideoCapture::start(
                self.state.cameras().clone(),
                VideoCaptureRequest::new(episode, cameras, clock),
                self.state.video_encoder(),
            )
            .await
            .map_err(|error| OperationError::InvalidInput(error.to_string()))
        } else {
            Ok(None)
        }
    }
}

impl From<crate::project_session::context::ProjectContext> for RecordingCommands {
    fn from(state: crate::project_session::context::ProjectContext) -> Self {
        Self { state }
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    #[test]
    fn workflow_execution_and_recording_exclude_each_other() {
        let lease = Arc::new(CatalogMutationLease::default());
        let session = crate::recording_session::RecordingSessionManager::new(lease.clone());
        assert!(session.reserve_start(|| true).is_err());
        let token = lease.try_acquire(CatalogLeaseOwner::WorkflowStart).unwrap();
        assert!(lease.release(token));
        let recording = session.reserve_start(|| false).unwrap().unwrap();
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowStart)
                .is_none()
        );
        session.cancel_start(recording);
    }
}
