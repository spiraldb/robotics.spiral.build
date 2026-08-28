use super::*;
use crate::adapters::hardware::RecordingArmProjection as _;
use crate::data::catalog_lease::CatalogLeaseToken;
use crate::recording::{RecordingDraftRequest, ReplayTrajectoryRequest};
use spiral_robotics_hardware::ModeSelection as HardwareModeSelection;
use spiral_robotics_storage::episodes::EpisodeArmCollection as _;

impl AppState {
    pub(super) fn recording_router() -> Router<Self> {
        Router::new()
            .route("/episodes", get(Self::list_episodes))
            .route("/episodes/record", post(Self::start_recording))
            .route("/episodes/record/pause", post(Self::set_recording_paused))
            .route("/episodes/record/stop", post(Self::stop_recording))
            .route("/episodes/replay", post(Self::start_replay))
            .route("/episodes/replay/pause", post(Self::set_replay_paused))
            .route(
                "/episodes/{id}",
                axum::routing::delete(Self::delete_episode),
            )
            .route("/episodes/{id}/inspection", get(Self::episode_inspection))
            .route("/episodes/{id}/seek", post(Self::seek_episode))
            .route("/episodes/{id}/videos/{stream}", get(Self::episode_video))
    }
}

/// Concrete recording/replay adapters kept at the delivery edge.
///
/// Opening a take still assembles telemetry-derived arm layouts and camera
/// specifications here because those are concrete backend details. Session
/// validation, consequential ordering, and compensation live in the
/// framework-neutral capability action.
struct RecordingHttpAdapter<'a> {
    state: &'a AppState,
    presented_recording: std::sync::Mutex<Option<EpisodeInfo>>,
}

impl<'a> RecordingHttpAdapter<'a> {
    fn new(state: &'a AppState) -> Self {
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
    arms: Vec<spiral_robotics_api::recording::EpisodeArm>,
    summary: EpisodeInfo,
}

impl RecordingPort for RecordingHttpAdapter<'_> {
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
        self.state
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
        self.state.create_recording_draft(request).await
    }

    fn draft_episode(&self, draft: &Self::RecordingDraft) -> String {
        draft.summary.id().to_owned()
    }

    async fn start_recording_media(
        &self,
        episode: &str,
        arms: &[String],
    ) -> Result<Option<Self::MediaCapture>, OperationError> {
        self.state.start_recording_media(episode, arms).await
    }

    async fn begin_recording(
        &self,
        draft: Self::RecordingDraft,
    ) -> Result<RecordingResult, OperationError> {
        self.state
            .hardware()
            .start_recording(draft.writer, draft.arms.to_hardware())
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
        self.state.finish_video_capture(capture).await
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

impl ReplayPort for RecordingHttpAdapter<'_> {
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
pub(super) struct EpisodeHttpAdapter<'a> {
    state: &'a AppState,
}

impl<'a> EpisodeHttpAdapter<'a> {
    pub(super) fn catalog(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl EpisodeCatalogPort for EpisodeHttpAdapter<'_> {
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

impl EpisodeQueryPort for EpisodeHttpAdapter<'_> {
    async fn episodes(&self) -> Vec<EpisodeInfo> {
        self.state.episodes().list().await
    }

    async fn ensure_episode_exists(&self, episode: &str) -> Result<(), OperationError> {
        self.state
            .episodes()
            .get(episode)
            .await
            .map(|_| ())
            .map_err(OperationError::from)
    }

    async fn inspection(
        &self,
        episode: String,
    ) -> Result<EpisodeInspectionSnapshot, OperationError> {
        let inspection = self
            .state
            .episodes()
            .inspection(&episode)
            .await
            .map_err(OperationError::from)?;
        Ok(inspection.into_snapshot(self.state.workspace()).await)
    }
}

/// Projects a stored inspection into the wire snapshot.
///
/// Both types are foreign to this crate, so the row-major to frame-major
/// transposition hangs off the stored value as a local extension trait rather
/// than a `From` impl.
trait InspectionProjection {
    async fn into_snapshot(self, workspace: &RobotWorkspace) -> EpisodeInspectionSnapshot;
}

impl InspectionProjection for EpisodeInspection {
    async fn into_snapshot(self, workspace: &RobotWorkspace) -> EpisodeInspectionSnapshot {
        let inspection = self;
        let (episode, summary, videos) = inspection.into_parts();
        let samples = episode.samples();
        let saved_robots = workspace.list().await;
        let arms = episode
            .start()
            .arms()
            .iter()
            .enumerate()
            .map(|(arm_index, arm)| {
                let joint_count = arm.joints().len();
                let calibration = saved_robots
                    .iter()
                    .find(|robot| robot.id() == arm.id())
                    .cloned()
                    .and_then(|record| record.calibration().ok().flatten());
                let normalized = samples
                    .iter()
                    .flat_map(|sample| sample.normalized()[arm_index].iter().copied())
                    .collect();
                let raw = samples
                    .iter()
                    .flat_map(|sample| sample.raw()[arm_index].iter().copied())
                    .collect();
                let degrees = samples
                    .iter()
                    .flat_map(|sample| {
                        sample.raw()[arm_index]
                            .iter()
                            .zip(arm.joints())
                            .map(|(raw, joint)| {
                                calibration
                                    .as_ref()
                                    .and_then(|calibration| calibration.motors().get(joint))
                                    .map(|motor| motor.degrees_from_center(*raw))
                            })
                    })
                    .collect();
                let velocity = OptionalFrameMatrix::new(samples, joint_count, |sample, index| {
                    sample
                        .velocity()
                        .and_then(|arms| arms.get(index))
                        .map(Vec::as_slice)
                })
                .for_arm(arm_index);
                let effort = OptionalFrameMatrix::new(samples, joint_count, |sample, index| {
                    sample
                        .effort()
                        .and_then(|arms| arms.get(index))
                        .map(Vec::as_slice)
                })
                .for_arm(arm_index);
                let action = OptionalFrameMatrix::new(samples, joint_count, |sample, index| {
                    sample
                        .action()
                        .get(index)
                        .and_then(Option::as_ref)
                        .map(Vec::as_slice)
                })
                .for_arm(arm_index);
                EpisodeInspectionArm::new(
                    arm.id(),
                    arm.joints().to_vec(),
                    EpisodeArmScalarSamples::new(normalized, raw).with_degrees(degrees),
                )
                .with_optional_channels(EpisodeArmOptionalChannels::new(velocity, effort, action))
            })
            .collect();
        EpisodeInspectionSnapshot::new(
            summary,
            samples.iter().map(|sample| sample.seconds()).collect(),
            samples
                .iter()
                .map(|sample| sample.follow_error_degrees())
                .collect(),
        )
        .with_arms(arms)
        .with_videos(
            videos
                .into_iter()
                .map(|video| {
                    let size = video.size();
                    let timing = video.timing();
                    let segment = video.segment();
                    EpisodeVideoAssetMetadata::new(
                        EpisodeVideoIdentity::new(
                            video.stream_id(),
                            video.camera_id(),
                            segment.index(),
                        ),
                        EpisodeVideoFormat::new(size.width(), size.height(), timing.fps()),
                        EpisodeVideoTimeline::new(
                            timing.frames(),
                            video.duration_seconds(),
                            segment.timeline_offset_seconds(),
                        ),
                    )
                })
                .collect(),
        )
    }
}

struct OptionalFrameMatrix<'a, F> {
    samples: &'a [spiral_robotics_storage::episodes::Sample],
    joint_count: usize,
    values_at: F,
}

impl<'samples, F> OptionalFrameMatrix<'samples, F>
where
    F: for<'sample> Fn(
        &'sample spiral_robotics_storage::episodes::Sample,
        usize,
    ) -> Option<&'sample [Option<f64>]>,
{
    fn new(
        samples: &'samples [spiral_robotics_storage::episodes::Sample],
        joint_count: usize,
        values_at: F,
    ) -> Self {
        Self {
            samples,
            joint_count,
            values_at,
        }
    }

    fn for_arm(self, arm_index: usize) -> EpisodeOptionalFrameMatrix {
        let mut present = Vec::with_capacity(self.samples.len());
        let mut values = Vec::with_capacity(self.samples.len().saturating_mul(self.joint_count));
        for sample in self.samples {
            match (self.values_at)(sample, arm_index) {
                Some(frame) => {
                    debug_assert_eq!(frame.len(), self.joint_count);
                    present.push(true);
                    values.extend(frame.iter().copied());
                }
                None => {
                    present.push(false);
                    values.extend(std::iter::repeat_n(None, self.joint_count));
                }
            }
        }
        EpisodeOptionalFrameMatrix::new(present, values)
    }
}

/// Adapter for recorded physical seeks. The current-ticket check and motor
/// handoff share the store's serialization gate, so a coalesced result proves
/// no stale pose reached hardware.
struct SeekHttpAdapter<'a> {
    state: &'a AppState,
}

impl PhysicalSeekPort for SeekHttpAdapter<'_> {
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
        Ok(self
            .state
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

impl AppState {
    async fn list_episodes(State(state): State<Self>) -> Json<EpisodeCatalogResponse> {
        // A paused take has a readable SpiralDB checkpoint. Hide its catalog row
        // here because live recording state is delivered through telemetry.
        let recording = state
            .hardware()
            .state()
            .recording()
            .as_ref()
            .map(|progress| progress.episode().to_owned());
        let episodes = EpisodeQueries::new(&EpisodeHttpAdapter::catalog(&state))
            .list()
            .await
            .into_iter()
            .filter(|episode| Some(episode.id()) != recording.as_deref())
            .collect();
        let mut issues = state.episodes().issues();
        let datasets = match state.storage().datasets().await {
            Ok(datasets) => datasets,
            Err(error) => {
                issues.push(format!("dataset catalog: {error}"));
                Vec::new()
            }
        };
        Json(EpisodeCatalogResponse::new(datasets, episodes, issues))
    }

    /// Begin a take.
    ///
    /// Record is the session-level command: it chooses the configured pair, enters
    /// teleoperation, and only then opens the take. If opening the take fails, the
    /// mode change is rolled back so a failed recording cannot leave an unnoticed
    /// teleoperation session running.
    async fn start_recording(
        State(state): State<Self>,
        Json(request): Json<StartRecordingRequest>,
    ) -> Result<Json<EpisodeInfo>, Failure> {
        let adapter = RecordingHttpAdapter::new(&state);
        let result = RecordingActions::new(&adapter)
            .start(StartRecordingCommand::new(
                request.dataset_id(),
                request.arms().to_vec(),
                request.note().map(str::to_owned),
            ))
            .await?;
        Ok(Json(adapter.present_recording(result)?))
    }

    /// Assemble a storage draft from authoritative live telemetry.
    ///
    /// Eligibility, sample-rate admission, consequential ordering, and cleanup
    /// belong to the capability action. This adapter only translates live backend
    /// structures into the episode writer's concrete representation.
    async fn create_recording_draft(
        &self,
        request: RecordingDraftRequest,
    ) -> Result<RecordingDraft, OperationError> {
        let (command, source, wanted, sample_hz) = request.into_parts();
        let (dataset_id, _requested_arms, note) = command.into_parts();
        let hardware = self.hardware().state();
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
        let saved_robots = self.workspace().list().await;
        let arms = spiral_robotics_api::recording::EpisodeArm::from_joint_lists(&arms)
            .map_err(OperationError::from)?
            .into_iter()
            .map(|arm| {
                let robot_type = saved_robots
                    .iter()
                    .find(|robot| robot.id() == arm.id())
                    .cloned()
                    .and_then(|robot| robot.robot_type());
                spiral_robotics_api::recording::EpisodeArm::new(
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
        let summary = EpisodeInfo::new(
            EpisodeIdentity::new(id, dataset_id, arms.clone()).with_source(source),
            EpisodeTimeline::new(0, 0.0, sample_hz).with_started_ms(started_ms),
            EpisodeAssessment::new(false, None, None),
        );
        Ok(RecordingDraft {
            writer,
            arms,
            summary,
        })
    }

    /// Assemble camera capture for an already-created episode draft.
    async fn start_recording_media(
        &self,
        episode: &str,
        wanted: &[String],
    ) -> Result<Option<VideoCapture>, OperationError> {
        if self.preferences().get().video().enabled() {
            let snapshot = self.snapshot().await;
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
                self.cameras().clone(),
                VideoCaptureRequest::new(episode, cameras),
                self.video_encoder(),
            )
            .await
            .map_err(|error| OperationError::InvalidInput(error.to_string()))
        } else {
            Ok(None)
        }
    }

    async fn stop_recording(
        State(state): State<Self>,
        Json(request): Json<StopRecordingRequest>,
    ) -> Result<Json<EpisodeInfo>, Failure> {
        let adapter = RecordingHttpAdapter::new(&state);
        let result = RecordingActions::new(&adapter)
            .stop(StopRecordingCommand::new(request.keep()))
            .await?;
        Ok(Json(adapter.present_recording(result)?))
    }

    /// Suspend or resume capture without closing the episode writer.
    ///
    /// Pause acknowledges only after every accepted sample has crossed the writer
    /// checkpoint barrier and the episode has been committed to SpiralDB. That
    /// makes timeline reads deterministic while the operator scrubs. Hardware owns
    /// restoring the exact control mode on resume; the route brackets it with video
    /// capture so paused camera frames are omitted as well.
    async fn set_recording_paused(
        State(state): State<Self>,
        Json(request): Json<SetRecordingPausedRequest>,
    ) -> Result<StatusCode, Failure> {
        RecordingActions::new(&RecordingHttpAdapter::new(&state))
            .set_paused(SetRecordingPausedCommand::new(request.paused()))
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn delete_episode(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<StatusCode, Failure> {
        let catalog = EpisodeHttpAdapter::catalog(&state);
        EpisodeCatalogActions::new(&catalog)
            .delete(DeleteEpisodeCommand::new(id))
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }
}

impl AppState {
    async fn episode_inspection(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<Json<EpisodeInspectionSnapshot>, Failure> {
        let inspection = EpisodeQueries::new(&EpisodeHttpAdapter::catalog(&state))
            .inspection(id)
            .await?;
        Ok(Json(inspection))
    }
}

impl AppState {}

impl AppState {}

#[derive(Debug, Deserialize)]
struct RecordedSeekRequest {
    robot: String,
    #[serde(default)]
    source_arm: Option<String>,
    seconds: f64,
}

#[derive(Debug, Serialize)]
struct RecordedSeekResponse {
    state: &'static str,
    episode: String,
    robot: String,
    source_arm: String,
    requested_seconds: f64,
    resolved_seconds: f64,
    frame: u64,
}

/// Move a physical follower to one committed recorded sample.
///
/// This is intentionally distinct from visual sampling. The controller still
/// owns torque, calibration, joint compatibility, bounded ramping, obstruction
/// timeout, and disconnect handling. A ticket drops stale requests that finish
/// a SpiralDB read after a newer scrub; the one remaining pose enters the controller
/// as a one-frame replay, so no route can bypass those protections.
impl AppState {
    async fn seek_episode(
        State(state): State<Self>,
        Path(id): Path<String>,
        Json(request): Json<RecordedSeekRequest>,
    ) -> Result<Json<RecordedSeekResponse>, Failure> {
        let adapter = SeekHttpAdapter { state: &state };
        let result = PhysicalSeekActions::new(&adapter)
            .seek(
                SeekEpisodeCommand::new(id, request.robot, request.seconds)
                    .with_source_arm(request.source_arm),
            )
            .await?;
        Ok(Json(RecordedSeekResponse {
            state: match result.state() {
                RecordedSeekState::Seeking => "seeking",
                RecordedSeekState::Coalesced => "coalesced",
            },
            episode: result.episode().to_owned(),
            robot: result.robot().to_owned(),
            source_arm: result.source_arm().to_owned(),
            requested_seconds: result.requested_seconds(),
            resolved_seconds: result.resolved_seconds(),
            frame: result.frame(),
        }))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ByteRange {
    start: u64,
    end: u64,
}

#[cfg(test)]
impl ByteRange {
    pub(super) const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }
}

impl ByteRange {
    pub(super) fn parse(value: &str, length: u64) -> Option<Self> {
        if length == 0 {
            return None;
        }
        let value = value.trim().strip_prefix("bytes=")?;
        if value.contains(',') {
            return None;
        }
        let (start, end) = value.split_once('-')?;
        if start.trim().is_empty() {
            let suffix = end.trim().parse::<u64>().ok()?;
            if suffix == 0 {
                return None;
            }
            return Some(Self {
                start: length.saturating_sub(suffix.min(length)),
                end: length - 1,
            });
        }
        let start = start.trim().parse::<u64>().ok()?;
        if start >= length {
            return None;
        }
        let end = if end.trim().is_empty() {
            length - 1
        } else {
            end.trim().parse::<u64>().ok()?.min(length - 1)
        };
        (end >= start).then_some(Self { start, end })
    }

    fn unsatisfied_response(length: u64) -> Response {
        Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{length}"))
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Body::empty())
            .expect("valid unsatisfied range response")
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct LocalVideoQuery {
    #[serde(default)]
    segment: Option<u32>,
}

impl AppState {
    async fn episode_video(
        State(state): State<Self>,
        (Path((id, stream)), Query(query), headers): (
            Path<(String, String)>,
            Query<LocalVideoQuery>,
            HeaderMap,
        ),
    ) -> Result<Response, Failure> {
        // Looking up the episode first prevents an orphaned media row from becoming
        // an independent serving namespace.
        EpisodeQueries::new(&EpisodeHttpAdapter::catalog(&state))
            .authorize_asset(id.clone())
            .await?;
        let asset = VideoAssetKey::new(id, stream, query.segment);
        let length = state
            .storage()
            .video_asset_size(&asset)
            .await
            .map_err(|error| Failure(StatusCode::NOT_FOUND, error))?;
        let requested_range = match headers.get(header::RANGE) {
            None => None,
            Some(value) => match value
                .to_str()
                .ok()
                .and_then(|value| ByteRange::parse(value, length))
            {
                Some(range) => Some(range),
                None => return Ok(ByteRange::unsatisfied_response(length)),
            },
        };
        let range = requested_range.unwrap_or(ByteRange {
            start: 0,
            end: length.saturating_sub(1),
        });
        let content_length = if length == 0 {
            0
        } else {
            range.end - range.start + 1
        };
        let body = if content_length == 0 {
            Bytes::new()
        } else {
            let payload = state
                .storage()
                .video_asset_range(&asset, VideoByteRange::new(range.start, content_length))
                .await
                .map_err(|error| Failure(StatusCode::NOT_FOUND, error))?;
            if payload.len() as u64 != content_length {
                return Err(Failure(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "Spiral returned {} video bytes for requested range {}-{} (expected {content_length})",
                        payload.len(),
                        range.start,
                        range.end,
                    ),
                ));
            }
            Bytes::from(payload)
        };
        let mut response = Response::builder()
            .status(if requested_range.is_some() {
                StatusCode::PARTIAL_CONTENT
            } else {
                StatusCode::OK
            })
            .header(header::CONTENT_TYPE, "video/mp4")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, content_length)
            .header(header::CACHE_CONTROL, "private, no-store");
        if requested_range.is_some() {
            response = response.header(
                header::CONTENT_RANGE,
                format!("bytes {}-{}/{}", range.start, range.end, length),
            );
        }
        Ok(response
            .body(Body::from(body))
            .expect("valid Spiral video response"))
    }

    async fn start_replay(
        State(state): State<Self>,
        Json(request): Json<StartReplayRequest>,
    ) -> Result<StatusCode, Failure> {
        ReplayActions::new(&RecordingHttpAdapter::new(&state))
            .start(
                StartReplayCommand::new(
                    request.episode(),
                    request.robot(),
                    request.source_arm().map(str::to_owned),
                )
                .starting_at(request.start_seconds()),
            )
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn set_replay_paused(
        State(state): State<Self>,
        Json(request): Json<SetReplayPausedRequest>,
    ) -> Result<StatusCode, Failure> {
        ReplayActions::new(&RecordingHttpAdapter::new(&state))
            .set_paused(SetReplayPausedCommand::new(request.paused()))
            .await?;
        Ok(StatusCode::NO_CONTENT)
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
