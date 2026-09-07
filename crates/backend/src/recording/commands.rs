//! Recording and replay application operations.
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};

use std::collections::BTreeSet;
#[cfg(test)]
use std::sync::Arc;

use crate::data::catalog::{
    DeleteEpisodeCommand, EpisodeActivity, EpisodeCatalogActions, EpisodeCatalogPort,
};
use crate::data::catalog_lease::CatalogLeaseToken;
#[cfg(test)]
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogMutationLease};
use crate::data::queries::EpisodeQueries;
use crate::data::seek::{
    ActiveRecording, PhysicalSeekActions, PhysicalSeekPlan, PhysicalSeekPort, RecordedSample,
    SampledArm, SeekActivity, SeekControlMode, SeekEpisodeCommand, SeekTarget, SeekableEpisode,
};
use crate::project_session::context::ProjectContext;
use crate::recording::{
    RecordingActions, RecordingContext, RecordingDraftRequest, RecordingPort, RecordingResult,
    ReplayActions, ReplayEpisode, ReplayPlan, ReplayPort, ReplayTrajectory,
    ReplayTrajectoryRequest, SetRecordingPausedCommand, SetReplayPausedCommand,
    StartRecordingCommand, StartReplayCommand, StopRecordingCommand, TeleoperationPair,
};
use crate::robotics::video::{CameraCaptureSpec, VideoAsset, VideoCapture, VideoCaptureRequest};
use crate::{BackendCommandError, OperationError};
use spiral_robotics_common::recording::{DatasetSummary, EpisodeInfo, EpisodeInspectionSnapshot};
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
    presented_recording: std::sync::Mutex<Option<EpisodeInfo>>,
}

impl<'a> RecordingAdapter<'a> {
    fn new(state: &'a ProjectContext) -> Self {
        Self {
            state,
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
    type MediaOutput = Vec<VideoAsset>;

    fn reserve_recording_start(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
        self.state.recording_session().reserve_start(|| {
            (
                self.state.workflow().is_active(),
                self.state.workflow_agent().is_active(),
            )
        })
    }

    fn bind_recording_lease(&self, token: CatalogLeaseToken, episode: String) {
        assert!(
            self.state.recording_session().bind_episode(token, episode),
            "the admitted recording must retain its exact catalog lease"
        );
    }

    fn claim_recording_lease(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
        self.state.recording_session().claim_finalization()
    }

    fn release_recording_lease(&self, token: CatalogLeaseToken) {
        self.state.recording_session().release(token);
    }

    fn recording_active(&self) -> bool {
        self.state.hardware().state().recording().is_some()
    }

    async fn default_teleoperation_pair(&self) -> Option<TeleoperationPair> {
        crate::project_session::setup::ProjectSetup::new(self.state)
            .snapshot()
            .await
            .default_teleoperation_pair()
            .map(|(leader, follower)| TeleoperationPair::new(leader, follower))
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
        let media_finalizing = self.state.recording_session().media_finalizing();
        RecordingContext::new(connected_arms, sample_hz, media_finalizing)
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

    async fn retain_recording_media(&self, capture: Option<Self::MediaCapture>) {
        self.state.recording_session().retain_media(capture).await;
    }

    async fn abandon_recording_media(&self, capture: Self::MediaCapture) {
        let _ = capture.finish().await;
    }

    async fn take_recording_media(&self) -> Option<Self::MediaCapture> {
        self.state.recording_session().take_media().await
    }

    fn media_episode(&self, capture: &Self::MediaCapture) -> String {
        capture.episode_id().to_owned()
    }

    async fn stop_recording(
        &self,
        command: StopRecordingCommand,
    ) -> Result<RecordingResult, OperationError> {
        let completion = self
            .state
            .hardware()
            .stop_recording(command.keep())
            .await
            .map_err(OperationError::conflict)?;
        let summary = self
            .state
            .episodes()
            .completed_episode(completion.episode())
            .map(|episode| episode.summary())
            .map_err(OperationError::from)?;
        let result = RecordingResult::new(completion.episode());
        *self.presented_recording.lock().map_err(|_| {
            OperationError::Internal("recording presenter lock poisoned".to_owned())
        })? = Some(summary);
        Ok(result)
    }

    async fn finish_recording_media(&self, capture: Self::MediaCapture) -> Self::MediaOutput {
        crate::recording::supervisor::RecordingSupervisor::new(self.state.clone())
            .finish_video_capture(capture)
            .await
    }

    fn empty_media_output(&self) -> Self::MediaOutput {
        Vec::new()
    }

    async fn commit_recording(
        &self,
        episode: String,
        media: Self::MediaOutput,
    ) -> Result<(), OperationError> {
        let full_episode = self
            .state
            .episodes()
            .completed_episode(&episode)
            .map_err(OperationError::from)?;
        self.state
            .storage()
            .commit_episode(&full_episode, &media)
            .await
            .map_err(OperationError::Internal)?;
        self.state.episodes().forget_completed(&episode);
        Ok(())
    }

    async fn request_media_pause(&self) {
        self.state.recording_session().request_media_pause().await;
    }

    async fn set_recording_paused(&self, paused: bool) -> Result<f64, OperationError> {
        self.state
            .hardware()
            .set_recording_paused(paused)
            .await
            .map_err(OperationError::conflict)
    }

    async fn finish_media_pause(&self) -> Result<(), OperationError> {
        let checkpoint = self.state.recording_session().finish_media_pause().await;
        let (episode, media) = match checkpoint {
            Some((episode, media, failures)) => {
                for failure in failures {
                    self.state
                        .cameras()
                        .note_issue(format!("paused episode video was not saved: {failure}"));
                }
                (episode, media)
            }
            None => {
                let Some(episode) = self.state.recording_session().episode() else {
                    return Ok(());
                };
                (episode, Vec::new())
            }
        };
        let full_episode = self
            .state
            .episodes()
            .completed_episode(&episode)
            .map_err(OperationError::from)?;
        self.state
            .storage()
            .commit_episode(&full_episode, &media)
            .await
            .map_err(OperationError::Internal)?;
        self.state.episodes().forget_completed(&episode);
        Ok(())
    }

    async fn resume_media(&self, active_seconds: f64) {
        if let Err(error) = self
            .state
            .recording_session()
            .resume_media(active_seconds)
            .await
        {
            self.state
                .cameras()
                .note_issue(format!("episode video could not resume: {error}"));
        }
    }
}

impl ReplayPort for RecordingAdapter<'_> {
    async fn replay_episode(&self, episode: &str) -> Result<ReplayEpisode, OperationError> {
        self.state
            .episodes()
            .get(episode)
            .await
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
            .episodes()
            .trajectory_at(
                &episode,
                RobotPlaybackRequest::new(&source_arm, start_seconds),
            )
            .await
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
        self.state
            .episodes()
            .delete(&episode)
            .await
            .map_err(OperationError::from)?;

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
        Ok(crate::project_session::setup::ProjectSetup::new(self.state)
            .snapshot()
            .await
            .robots()
            .iter()
            .find(|candidate| candidate.id() == robot)
            .map(|target| {
                SeekTarget::new(
                    target.id(),
                    target.robot_type().map(str::to_owned),
                    target.connection().connected(),
                )
                .with_readiness(target.calibrated(), target.problem().map(str::to_owned))
            }))
    }

    async fn episode(&self, episode: &str) -> Result<SeekableEpisode, OperationError> {
        self.state
            .episodes()
            .get(episode)
            .await
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
        self.state.episodes().reserve_seek(robot)
    }

    async fn sample_at(
        &self,
        episode: &str,
        seconds: f64,
    ) -> Result<RecordedSample, OperationError> {
        self.state
            .episodes()
            .sample_at(episode, seconds)
            .await
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
        let _gate = self.state.episodes().lock_seek().await;
        if !self.state.episodes().seek_is_current(&robot, ticket) {
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
    pub(crate) async fn datasets(&self) -> Result<Vec<DatasetSummary>, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .read(async move {
                let _activity = activity;
                commands.require_storage_ready()?;
                commands
                    .state
                    .storage()
                    .datasets()
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn episodes(
        &self,
        dataset_id: String,
    ) -> Result<spiral_robotics_common::recording::EpisodeCatalog, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .read(async move {
                let _activity = activity;
                commands.require_storage_ready()?;
                // A paused take has a readable SpiralDB checkpoint. Hide its catalog row
                // here because live recording state is delivered through telemetry.
                let recording = commands
                    .state
                    .hardware()
                    .state()
                    .recording()
                    .as_ref()
                    .map(|progress| progress.episode().to_owned());
                let episodes = EpisodeQueries::new(&commands.state)
                    .list(dataset_id)
                    .await?
                    .into_iter()
                    .filter(|episode| Some(episode.id()) != recording.as_deref())
                    .collect();
                Ok(spiral_robotics_common::recording::EpisodeCatalog::new(
                    episodes,
                    commands.state.episodes().issues(),
                ))
            })
            .await?
    }

    /// Begin a take using the configured teleoperation pair.
    pub(crate) async fn start_recording(
        &self,
        dataset_id: String,
    ) -> Result<EpisodeInfo, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                let adapter = RecordingAdapter::new(&commands.state);
                let result = RecordingActions::new(&adapter)
                    .start(StartRecordingCommand::new(dataset_id, Vec::new(), None))
                    .await?;
                adapter.present_recording(result).map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn stop_recording(
        &self,
        keep: bool,
    ) -> Result<EpisodeInfo, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                let adapter = RecordingAdapter::new(&commands.state);
                let result = RecordingActions::new(&adapter)
                    .stop(StopRecordingCommand::new(keep))
                    .await?;
                adapter.present_recording(result).map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn set_recording_paused(
        &self,
        paused: bool,
    ) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                RecordingActions::new(&RecordingAdapter::new(&commands.state))
                    .set_paused(SetRecordingPausedCommand::new(paused))
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn delete_episode(&self, id: String) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                EpisodeCatalogActions::new(&EpisodeAdapter::new(&commands.state))
                    .delete(DeleteEpisodeCommand::new(id))
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn inspection(
        &self,
        id: String,
    ) -> Result<EpisodeInspectionSnapshot, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_storage_ready()?;
                EpisodeQueries::new(&commands.state)
                    .inspection(id)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    /// Move a physical follower to one committed recorded sample.
    pub(crate) async fn seek_episode(
        &self,
        id: String,
        robot: String,
        source_arm: Option<String>,
        seconds: f64,
    ) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                PhysicalSeekActions::new(&SeekAdapter {
                    state: &commands.state,
                })
                .seek(SeekEpisodeCommand::new(id, robot, seconds).with_source_arm(source_arm))
                .await?;
                Ok(())
            })
            .await?
    }

    pub(crate) async fn start_replay(
        &self,
        episode: String,
        robot: String,
        source_arm: Option<String>,
        start_seconds: f64,
    ) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                ReplayActions::new(&RecordingAdapter::new(&commands.state))
                    .start(
                        StartReplayCommand::new(episode, robot, source_arm)
                            .starting_at(start_seconds),
                    )
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn set_replay_paused(&self, paused: bool) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                ReplayActions::new(&RecordingAdapter::new(&commands.state))
                    .set_paused(SetReplayPausedCommand::new(paused))
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    fn require_storage_ready(&self) -> Result<(), BackendCommandError> {
        match self.state.bootstrap().storage_unavailable() {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    fn require_catalog_ready(&self) -> Result<(), BackendCommandError> {
        match self.state.bootstrap().catalog_unavailable() {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
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
        let (command, source, wanted, sample_hz) = request.into_parts();
        let (dataset_id, _requested_arms, note) = command.into_parts();
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
            .episodes()
            .begin_in_dataset(
                EpisodeRecordingRequest::new(dataset_id.clone(), source.clone(), sample_hz)
                    .with_note(note)
                    .with_arms(arms.clone(), condition),
            )
            .await
            .map_err(OperationError::from)?;
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
        if self.state.preferences().get().video().enabled() {
            let snapshot = crate::project_session::setup::ProjectSetup::new(self.state)
                .snapshot()
                .await;
            let camera_status = snapshot
                .cameras()
                .iter()
                .map(|camera| (camera.id(), camera))
                .collect::<std::collections::BTreeMap<_, _>>();
            let mut seen = BTreeSet::new();
            let mut cameras = Vec::new();
            for robot in snapshot
                .robots()
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
            VideoCapture::start(
                self.state.cameras().clone(),
                VideoCaptureRequest::new(episode, cameras),
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
    fn active_workflow_or_assistant_blocks_recording_and_releases_admission() {
        for (activity, expected, next_owner) in [
            (
                (true, false),
                "active Workflow run",
                CatalogLeaseOwner::WorkflowStart,
            ),
            (
                (false, true),
                "active Workflow Assistant turn",
                CatalogLeaseOwner::WorkflowAgentStart,
            ),
        ] {
            let lease = Arc::new(CatalogMutationLease::default());
            let session = crate::recording_session::RecordingSessionManager::new(lease.clone());
            let error = session
                .reserve_start(|| activity)
                .expect_err("published activity must block a later recording");
            assert!(error.to_string().contains(expected));

            let next = lease
                .try_acquire(next_owner)
                .expect("a rejected recording must release its admission lease");
            assert!(lease.release(next));
        }
    }

    #[test]
    fn admitted_recording_blocks_later_workflow_and_assistant_admission() {
        let lease = Arc::new(CatalogMutationLease::default());
        let session = crate::recording_session::RecordingSessionManager::new(lease.clone());
        let recording = session
            .reserve_start(|| (false, false))
            .expect("idle activity check")
            .expect("recording admission");

        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowStart)
                .is_none(),
            "manual Workflow cannot pass an admitted recording"
        );
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowAgentStart)
                .is_none(),
            "Assistant cannot pass an admitted recording"
        );
        session.release(recording);
    }
}
