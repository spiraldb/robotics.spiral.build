use super::*;
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease};
use crate::recording::{RecordingDraftRequest, ReplayTrajectoryRequest};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/episodes", get(list_episodes))
        .route("/episodes/record", post(start_recording))
        .route("/episodes/record/pause", post(set_recording_paused))
        .route("/episodes/record/stop", post(stop_recording))
        .route("/episodes/replay", post(start_replay))
        .route("/episodes/replay/pause", post(set_replay_paused))
        .route("/episodes/{id}", axum::routing::delete(delete_episode))
        .route("/episodes/{id}/judge", post(judge_episode))
        .route("/episodes/{id}/trace", get(episode_trace))
        .route("/episodes/{id}/inspection", get(episode_inspection))
        .route("/episodes/{id}/timeline", get(episode_timeline))
        .route(
            "/episodes/{id}/timeline/traces",
            get(episode_timeline_traces),
        )
        .route(
            "/episodes/{id}/timeline/sample",
            get(episode_timeline_sample),
        )
        .route("/episodes/{id}/seek", post(seek_episode))
        .route("/episodes/{id}/videos/{stream}", get(episode_video))
}

/// Concrete recording/replay adapters kept at the delivery edge.
///
/// Opening a take still assembles telemetry-derived arm layouts and camera
/// specifications here because those are concrete backend details. Session
/// validation, consequential ordering, and compensation live in the
/// framework-neutral capability action.
struct RecordingHttpAdapter<'a> {
    state: &'a AppState,
    presented_recording: std::sync::Mutex<Option<EpisodeSummary>>,
}

impl<'a> RecordingHttpAdapter<'a> {
    fn new(state: &'a AppState) -> Self {
        Self {
            state,
            presented_recording: std::sync::Mutex::new(None),
        }
    }

    fn present_recording(&self, result: RecordingResult) -> Result<EpisodeSummary, OperationError> {
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
    summary: EpisodeSummary,
}

impl RecordingPort for RecordingHttpAdapter<'_> {
    type RecordingDraft = RecordingDraft;
    type MediaCapture = VideoCapture;
    type MediaOutput = Vec<VideoAsset>;

    fn reserve_recording_start(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
        reserve_recording_catalog_mutation(self.state.catalog_lease(), || {
            (
                self.state.workflow().is_active(),
                self.state.workflow_agent().is_active(),
            )
        })
    }

    fn bind_recording_lease(&self, token: CatalogLeaseToken, episode: String) {
        assert!(
            self.state.catalog_lease().bind_recording(token, episode),
            "the admitted recording must retain its exact catalog lease"
        );
    }

    fn claim_recording_lease(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
        if let Some(recording) = self.state.catalog_lease().claim_recording() {
            return Ok(Some(recording.token()));
        }
        if self.state.catalog_lease().recording_is_opening() {
            return Err(OperationError::Conflict(
                "the recording is still opening; wait before stopping it".to_owned(),
            ));
        }
        Ok(None)
    }

    fn release_recording_lease(&self, token: CatalogLeaseToken) {
        let released = self.state.catalog_lease().release(token);
        debug_assert!(released, "recording releases its exact catalog lease once");
    }

    fn recording_active(&self) -> bool {
        self.state
            .hardware()
            .state()
            .telemetry()
            .recording()
            .is_some()
    }

    fn default_teleoperation_pair(&self) -> Option<TeleoperationPair> {
        self.state
            .snapshot()
            .default_teleoperation_pair()
            .map(|(leader, follower)| TeleoperationPair::new(leader, follower))
    }

    fn recording_context(&self) -> RecordingContext {
        let hardware = self.state.hardware().state();
        let connected_arms = hardware
            .telemetry()
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
        let sample_hz = if hardware.telemetry().target_hz() > 0.0 {
            (hardware.telemetry().target_hz() * 1_000.0).round() / 1_000.0
        } else {
            spiral_robotics_hardware::CONTROL_HZ as f64
        };
        let media_finalizing = self
            .state
            .recording_video()
            .try_lock()
            .map_or(true, |capture| capture.is_some());
        RecordingContext::new(connected_arms, sample_hz, media_finalizing)
    }

    async fn enter_teleoperation(&self, pair: &TeleoperationPair) -> Result<(), OperationError> {
        self.state
            .hardware()
            .set_mode(ModeSelection::Teleoperate {
                leader: pair.leader().to_owned(),
                follower: pair.follower().to_owned(),
            })
            .await
            .map_err(OperationError::conflict)
    }

    async fn enter_idle(&self) -> Result<(), OperationError> {
        self.state
            .hardware()
            .set_mode(ModeSelection::Idle)
            .await
            .map_err(OperationError::conflict)
    }

    fn create_recording_draft(
        &self,
        request: RecordingDraftRequest,
    ) -> Result<RecordingDraft, OperationError> {
        create_recording_draft(self.state, request)
    }

    fn draft_episode(&self, draft: &Self::RecordingDraft) -> String {
        draft.summary.id().to_owned()
    }

    async fn start_recording_media(
        &self,
        episode: &str,
        arms: &[String],
    ) -> Result<Option<Self::MediaCapture>, OperationError> {
        start_recording_media_at_edge(self.state, episode, arms).await
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
        *self.state.recording_video().lock().await = capture;
    }

    async fn abandon_recording_media(&self, capture: Self::MediaCapture) {
        let _ = capture.finish().await;
    }

    async fn take_recording_media(&self) -> Option<Self::MediaCapture> {
        self.state.recording_video().lock().await.take()
    }

    fn media_episode(&self, capture: &Self::MediaCapture) -> String {
        capture.episode_id().to_owned()
    }

    async fn stop_recording(
        &self,
        command: StopRecordingCommand,
    ) -> Result<RecordingResult, OperationError> {
        let summary = self
            .state
            .hardware()
            .stop_recording(command.keep())
            .await
            .map_err(OperationError::conflict)?;
        let result = RecordingResult::new(summary.id());
        *self.presented_recording.lock().map_err(|_| {
            OperationError::Internal("recording presenter lock poisoned".to_owned())
        })? = Some(summary);
        Ok(result)
    }

    async fn finish_recording_media(&self, capture: Self::MediaCapture) -> Self::MediaOutput {
        finish_video_capture(self.state, capture).await
    }

    fn empty_media_output(&self) -> Self::MediaOutput {
        Vec::new()
    }

    fn commit_recording(
        &self,
        episode: String,
        media: Self::MediaOutput,
    ) -> Result<(), OperationError> {
        let completed = self
            .state
            .episodes()
            .completed(&episode)
            .map_err(OperationError::from)?;
        self.state
            .storage()
            .commit_episode_blocking(&completed, &media)
            .map_err(OperationError::Internal)
    }

    async fn request_media_pause(&self) {
        if let Some(capture) = self.state.recording_video().lock().await.as_ref() {
            capture.request_pause();
        }
    }

    async fn set_recording_paused(&self, paused: bool) -> Result<f64, OperationError> {
        self.state
            .hardware()
            .set_recording_paused(paused)
            .await
            .map_err(OperationError::conflict)
    }

    async fn finish_media_pause(&self) {
        let checkpoint = if let Some(capture) = self.state.recording_video().lock().await.as_mut() {
            for failure in capture.finish_pause().await {
                self.state
                    .cameras()
                    .note_issue(format!("paused episode video was not saved: {failure}"));
            }
            Some((capture.episode_id().to_owned(), capture.completed_assets()))
        } else {
            None
        };
        let Some((episode, media)) = checkpoint else {
            return;
        };
        let episodes = self.state.episodes().clone();
        let storage = self.state.storage().clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let completed = episodes
                .completed(&episode)
                .map_err(|error| error.to_string())?;
            storage.commit_episode_blocking(&completed, &media)
        })
        .await
        .unwrap_or_else(|error| Err(format!("video checkpoint task stopped: {error}")));
        if let Err(error) = outcome {
            self.state.cameras().note_issue(format!(
                "paused episode video was not saved to SpiralDB: {error}"
            ));
        }
    }

    async fn resume_media(&self, active_seconds: f64) {
        if let Some(capture) = self.state.recording_video().lock().await.as_mut()
            && let Err(error) = capture.resume(active_seconds)
        {
            self.state
                .cameras()
                .note_issue(format!("episode video could not resume: {error}"));
        }
    }
}

fn reserve_recording_catalog_mutation(
    lease: &CatalogMutationLease,
    activity: impl FnOnce() -> (bool, bool),
) -> Result<Option<CatalogLeaseToken>, OperationError> {
    let Some(token) = lease.try_acquire(CatalogLeaseOwner::Recording) else {
        return Ok(None);
    };
    let (workflow_active, assistant_active) = activity();
    let conflict = if workflow_active {
        Some("Wait for the active Workflow run to finish before recording a new take.")
    } else if assistant_active {
        Some("Wait for the active Workflow Assistant turn to finish before recording a new take.")
    } else {
        None
    };
    if let Some(message) = conflict {
        let released = lease.release(token);
        debug_assert!(
            released,
            "rejected recording admission releases its exact catalog lease"
        );
        return Err(OperationError::Conflict(message.to_owned()));
    }
    Ok(Some(token))
}

impl ReplayPort for RecordingHttpAdapter<'_> {
    fn replay_episode(&self, episode: &str) -> Result<ReplayEpisode, OperationError> {
        self.state
            .episodes()
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
        let episodes = self.state.episodes().clone();
        let episode = request.episode().to_owned();
        let source_arm = request.source_arm().to_owned();
        let start_seconds = request.start_seconds();
        tokio::task::spawn_blocking(move || {
            episodes.trajectory_at(&episode, ReplayRequest::new(&source_arm, start_seconds))
        })
        .await
        .map_err(|error| OperationError::Internal(error.to_string()))?
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
        self.state
            .hardware()
            .start_replay(
                spiral_robotics_hardware::ReplayRequest::timeline(
                    robot,
                    episode,
                    spiral_robotics_hardware::ReplayTrajectory::new(frames, sample_hz),
                )
                .starting_at(start_frame, resolved_start_seconds),
            )
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
                .telemetry()
                .recording()
                .as_ref()
                .map(|recording| recording.episode().to_owned()),
            match &hardware.mode().selection() {
                ModeSelection::Replay { episode, .. } => Some(episode.clone()),
                _ => None,
            },
        )
    }

    async fn judge_episode(
        &self,
        episode: String,
        discarded: bool,
    ) -> Result<EpisodeMutationResult, OperationError> {
        let episodes = self.state.episodes().clone();
        let summary = tokio::task::spawn_blocking(move || episodes.judge(&episode, discarded))
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?
            .map_err(OperationError::from)?;
        Ok(EpisodeMutationResult::new(summary.id()))
    }

    async fn delete_episode(&self, episode: String) -> Result<(), OperationError> {
        let episodes = self.state.episodes().clone();
        tokio::task::spawn_blocking(move || episodes.delete(&episode))
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?
            .map_err(OperationError::from)?;

        Ok(())
    }
}

impl EpisodeQueryPort for EpisodeHttpAdapter<'_> {
    fn episodes(&self) -> Vec<EpisodeRecord> {
        self.state
            .episodes()
            .list()
            .iter()
            .cloned()
            .map(Into::into)
            .collect()
    }

    fn ensure_episode_exists(&self, episode: &str) -> Result<(), OperationError> {
        self.state
            .episodes()
            .get(episode)
            .map(|_| ())
            .map_err(OperationError::from)
    }

    fn follow_error_trace(
        &self,
        query: EpisodeTraceQuery,
    ) -> Result<Vec<Option<f64>>, OperationError> {
        let episode = query.episode().to_owned();
        let buckets = query.buckets();
        self.state
            .episodes()
            .trace(&episode, buckets)
            .map_err(OperationError::from)
    }

    fn timeline_traces(
        &self,
        query: EpisodeTraceQuery,
    ) -> Result<TimelineTraceCollection, OperationError> {
        let episode = query.episode().to_owned();
        let buckets = query.buckets();
        self.state
            .episodes()
            .timeline_traces(&episode, buckets)
            .map(Into::into)
            .map_err(OperationError::from)
    }

    fn timeline(&self, episode: String) -> Result<EpisodeTimelineRecord, OperationError> {
        let samples = self
            .state
            .episodes()
            .timeline_metadata(&episode)
            .map_err(OperationError::from)?;
        // Metadata refreshes a stale partial entry. Read the summary after it
        // so both projections describe the same durable episode.
        let summary = self
            .state
            .episodes()
            .get(&episode)
            .map_err(OperationError::from)?;
        let videos = self
            .state
            .episodes()
            .video_metadata(&episode)
            .map_err(OperationError::from)?
            .iter()
            .cloned()
            .map(Into::into)
            .collect();
        Ok(EpisodeTimelineRecord::new(
            summary.into(),
            TimelineMetadataRecord::new(
                samples.samples(),
                samples.first_seconds(),
                samples.last_seconds(),
            ),
            videos,
        ))
    }

    fn inspection(&self, episode: String) -> Result<EpisodeInspectionRecord, OperationError> {
        self.state
            .episodes()
            .inspection(&episode)
            .map(Into::into)
            .map_err(OperationError::from)
    }

    fn timeline_sample(
        &self,
        episode: String,
        seconds: f64,
    ) -> Result<TimelineSampleRecord, OperationError> {
        self.state
            .episodes()
            .sample_at(&episode, seconds)
            .map(Into::into)
            .map_err(OperationError::from)
    }
}

impl From<EpisodeSummary> for EpisodeRecord {
    fn from(episode: EpisodeSummary) -> Self {
        let identity =
            EpisodeRecordIdentity::new(episode.id(), episode.dataset_id(), episode.source())
                .started_at(episode.started_ms());
        let capture = EpisodeCaptureSummary::new(
            episode
                .arms()
                .iter()
                .map(|arm| {
                    EpisodeArmRecord::new(
                        arm.id(),
                        arm.robot_type().map(str::to_owned),
                        arm.joints().to_vec(),
                    )
                })
                .collect(),
            episode.frames(),
            episode.duration_seconds(),
        )
        .with_timing(
            episode
                .recording_gaps()
                .iter()
                .map(|gap| RecordingGapRecord::new(gap.after_seconds(), gap.duration_seconds()))
                .collect(),
            episode.sample_hz(),
        )
        .with_maximum_follow_error(episode.max_follow_error_degrees());
        Self::new(
            identity,
            capture,
            EpisodeDisposition::new(episode.discarded(), episode.problem().map(str::to_owned)),
        )
    }
}

impl From<EpisodeRecord> for EpisodeSummary {
    fn from(episode: EpisodeRecord) -> Self {
        let identity = EpisodeIdentity::new(
            episode.id(),
            episode.dataset_id(),
            episode
                .arms()
                .iter()
                .map(|arm| {
                    spiral_robotics_api::recording::EpisodeArm::new(
                        arm.id(),
                        arm.robot_type().map(str::to_owned),
                        arm.joints().to_vec(),
                    )
                })
                .collect(),
        )
        .with_source(episode.source());
        let timeline = EpisodeTimeline::new(
            episode.frames(),
            episode.duration_seconds(),
            episode.sample_hz(),
        )
        .with_recording_gaps(
            episode
                .recording_gaps()
                .iter()
                .map(|gap| {
                    spiral_robotics_api::recording::RecordingGap::new(
                        gap.after_seconds(),
                        gap.duration_seconds(),
                    )
                })
                .collect(),
        )
        .with_started_ms(episode.started_ms());
        Self::new(
            identity,
            timeline,
            EpisodeAssessment::new(
                episode.discarded(),
                episode.max_follow_error_degrees(),
                episode.problem().map(str::to_owned),
            ),
        )
    }
}

impl From<VideoAssetMetadata> for VideoAssetRecord {
    fn from(video: VideoAssetMetadata) -> Self {
        let size = video.size();
        let timing = video.timing();
        let segment = video.segment();
        Self::new(
            VideoAssetIdentity::new(video.stream_id(), video.camera_id(), segment.index()),
            VideoAssetFormat::new(size.width(), size.height(), timing.fps()),
            VideoAssetTimeline::new(
                timing.frames(),
                video.duration_seconds(),
                segment.timeline_offset_seconds(),
            ),
        )
    }
}

impl From<EpisodeInspection> for EpisodeInspectionRecord {
    fn from(inspection: EpisodeInspection) -> Self {
        let (episode, summary, videos) = inspection.into_parts();
        let samples = episode.samples();
        let arms = episode
            .start()
            .arms()
            .iter()
            .enumerate()
            .map(|(arm_index, arm)| {
                let joint_count = arm.joints().len();
                let normalized = samples
                    .iter()
                    .flat_map(|sample| sample.normalized()[arm_index].iter().copied())
                    .collect();
                let raw = samples
                    .iter()
                    .flat_map(|sample| sample.raw()[arm_index].iter().copied())
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
                EpisodeInspectionArmRecord::new(arm.id(), arm.joints().to_vec(), normalized)
                    .with_raw(raw)
                    .with_dynamics(velocity, effort)
                    .with_action(action)
            })
            .collect();
        Self::new(
            summary.into(),
            samples.iter().map(|sample| sample.seconds()).collect(),
            arms,
        )
        .with_follow_error(
            samples
                .iter()
                .map(|sample| sample.follow_error_degrees())
                .collect(),
        )
        .with_videos(videos.into_iter().map(Into::into).collect())
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

    fn for_arm(self, arm_index: usize) -> OptionalFrameMatrixRecord {
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
        OptionalFrameMatrixRecord::new(present, values)
    }
}

impl From<VideoAssetRecord> for VideoAssetMetadata {
    fn from(video: VideoAssetRecord) -> Self {
        Self::new(
            video.stream_id().to_owned(),
            video.camera_id().to_owned(),
            video.duration_seconds(),
        )
        .with_size(VideoSize::new(video.width(), video.height()))
        .with_timing(VideoTiming::new(video.fps(), video.frames()))
        .with_segment(VideoSegment::new(
            video.segment_index(),
            video.timeline_offset_seconds(),
        ))
    }
}

impl From<EpisodeInspectionRecord> for EpisodeInspectionSnapshot {
    fn from(inspection: EpisodeInspectionRecord) -> Self {
        let (episode, sample_times_seconds, follow_error_degrees, arm_records, video_records) =
            inspection.into_parts();
        let arms = arm_records
            .into_iter()
            .map(|arm| {
                let (arm, joints, normalized, raw, velocity, effort, action) = arm.into_parts();
                EpisodeInspectionArm::new(
                    arm,
                    joints,
                    EpisodeArmScalarSamples::new(normalized, raw),
                )
                .with_optional_channels(EpisodeArmOptionalChannels::new(
                    velocity.into(),
                    effort.into(),
                    action.into(),
                ))
            })
            .collect();
        let videos = video_records
            .into_iter()
            .map(|video| {
                EpisodeVideoAssetMetadata::new(
                    EpisodeVideoIdentity::new(
                        video.stream_id(),
                        video.camera_id(),
                        video.segment_index(),
                    ),
                    EpisodeVideoFormat::new(video.width(), video.height(), video.fps()),
                    EpisodeVideoTimeline::new(
                        video.frames(),
                        video.duration_seconds(),
                        video.timeline_offset_seconds(),
                    ),
                )
            })
            .collect();
        Self::new(episode.into(), sample_times_seconds, follow_error_degrees)
            .with_arms(arms)
            .with_videos(videos)
    }
}

impl From<OptionalFrameMatrixRecord> for EpisodeOptionalFrameMatrix {
    fn from(matrix: OptionalFrameMatrixRecord) -> Self {
        let (present, values) = matrix.into_parts();
        Self::new(present, values)
    }
}

impl From<TimelineSample> for TimelineSampleRecord {
    fn from(sample: TimelineSample) -> Self {
        let (frame, seconds, follow_error_degrees, arms) = sample.into_parts();
        let arms = arms
            .into_iter()
            .map(|arm| {
                let (pose, signals) = arm.into_groups();
                let (arm, normalized, raw) = pose.into_parts();
                let (velocity, effort, action) = signals.into_parts();
                TimelineArmSampleRecord::new(arm, normalized, raw)
                    .with_dynamics(velocity, effort)
                    .with_action(action)
            })
            .collect();
        Self::new(frame, seconds, arms).with_follow_error(follow_error_degrees)
    }
}

impl From<TimelineSampleRecord> for TimelineSample {
    fn from(sample: TimelineSampleRecord) -> Self {
        let (frame, seconds, follow_error_degrees, arms) = sample.into_parts();
        Self::new(frame, seconds)
            .with_follow_error_degrees(follow_error_degrees)
            .with_arms(
                arms.into_iter()
                    .map(|arm| {
                        let (arm, normalized, raw, velocity, effort, action) = arm.into_parts();
                        spiral_robotics_storage::episodes::TimelineArmSample::new(
                            arm, normalized, raw,
                        )
                        .with_velocity(velocity)
                        .with_effort(effort)
                        .with_action(action)
                    })
                    .collect(),
            )
    }
}

impl From<TimelineTraces> for TimelineTraceCollection {
    fn from(traces: TimelineTraces) -> Self {
        let (episode, samples, buckets, start_seconds, end_seconds, lanes) = traces.into_parts();
        let lanes = lanes
            .into_iter()
            .map(|lane| {
                let (joint, series) = lane.into_parts();
                let series = series
                    .into_iter()
                    .map(|series| {
                        let (arm, points) = series.into_parts();
                        let points = points
                            .iter()
                            .map(|point| {
                                TimelineTracePointRecord::new(point.seconds(), point.value())
                            })
                            .collect();
                        TimelineTraceSeriesRecord::new(arm, points)
                    })
                    .collect();
                TimelineTraceLaneRecord::new(joint, series)
            })
            .collect();
        Self::new(episode, samples, buckets)
            .with_range(start_seconds, end_seconds)
            .with_lanes(lanes)
    }
}

impl From<TimelineTraceCollection> for TimelineTraces {
    fn from(traces: TimelineTraceCollection) -> Self {
        let (episode, samples, buckets, start_seconds, end_seconds, lanes) = traces.into_parts();
        Self::new(episode, samples, buckets)
            .with_range(start_seconds, end_seconds)
            .with_lanes(
                lanes
                    .into_iter()
                    .map(|lane| {
                        let (joint, series) = lane.into_parts();
                        spiral_robotics_storage::episodes::TimelineTraceLane::new(
                            joint,
                            series
                                .into_iter()
                                .map(|series| {
                                    let (arm, points) = series.into_parts();
                                    spiral_robotics_storage::episodes::TimelineTraceSeries::new(
                                arm,
                                points
                                    .into_iter()
                                    .map(|point| {
                                        spiral_robotics_storage::episodes::TimelineTracePoint::new(
                                            point.seconds(),
                                            point.value(),
                                        )
                                    })
                                    .collect(),
                            )
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            )
    }
}

/// Adapter for historical physical seeks. The current-ticket check and motor
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
                .telemetry()
                .recording()
                .as_ref()
                .map(|recording| ActiveRecording::new(recording.episode(), recording.paused())),
            match hardware.mode().selection() {
                ModeSelection::Idle => SeekControlMode::Idle,
                ModeSelection::Replay { .. } => SeekControlMode::Replay,
                _ => SeekControlMode::Other,
            },
        )
    }

    fn target(&self, robot: &str) -> Result<Option<SeekTarget>, OperationError> {
        Ok(self
            .state
            .snapshot()
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

    fn episode(&self, episode: &str) -> Result<SeekableEpisode, OperationError> {
        self.state
            .episodes()
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
        self.state.episodes().reserve_seek(robot)
    }

    async fn sample_at(
        &self,
        episode: &str,
        seconds: f64,
    ) -> Result<HistoricalSample, OperationError> {
        let episodes = self.state.episodes().clone();
        let episode = episode.to_owned();
        tokio::task::spawn_blocking(move || episodes.sample_at(&episode, seconds))
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?
            .map(|sample| {
                let (frame, seconds, _, arms) = sample.into_parts();
                HistoricalSample::new(
                    seconds,
                    frame,
                    arms.into_iter()
                        .map(|arm| {
                            let (pose, _) = arm.into_groups();
                            let (id, normalized, _) = pose.into_parts();
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
        self.state
            .hardware()
            .start_replay(spiral_robotics_hardware::ReplayRequest::physical_seek(
                robot,
                episode,
                spiral_robotics_hardware::ReplayTrajectory::single_pose(pose, sample_hz),
            ))
            .await
            .map_err(OperationError::conflict)?;
        Ok(true)
    }
}

async fn list_episodes(State(state): State<AppState>) -> Json<EpisodeCatalogResponse> {
    // A paused take has a readable SpiralDB checkpoint. Hide its catalog row
    // here because live recording state is delivered through telemetry.
    let recording = state
        .hardware()
        .state()
        .telemetry()
        .recording()
        .as_ref()
        .map(|progress| progress.episode().to_owned());
    let episodes = EpisodeQueries::new(&EpisodeHttpAdapter::catalog(&state))
        .list()
        .into_iter()
        .filter(|episode| Some(episode.id()) != recording.as_deref())
        .map(Into::into)
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
    State(state): State<AppState>,
    Json(request): Json<StartRecordingRequest>,
) -> Result<Json<EpisodeSummary>, Failure> {
    let adapter = RecordingHttpAdapter::new(&state);
    let result = recording::start_recording(
        &adapter,
        StartRecordingCommand::new(
            request.dataset_id(),
            request.arms().to_vec(),
            request.note().map(str::to_owned),
        ),
    )
    .await?;
    Ok(Json(adapter.present_recording(result)?))
}

/// Assemble a storage draft from authoritative live telemetry.
///
/// Eligibility, sample-rate admission, consequential ordering, and cleanup
/// belong to the capability action. This adapter only translates live backend
/// structures into the episode writer's concrete representation.
fn create_recording_draft(
    state: &AppState,
    request: RecordingDraftRequest,
) -> Result<RecordingDraft, OperationError> {
    let (command, source, wanted, sample_hz) = request.into_parts();
    let (dataset_id, _requested_arms, note) = command.into_parts();
    let hardware = state.hardware().state();
    let connected = hardware
        .telemetry()
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
    let arms = build_arms(&arms)
        .map_err(OperationError::from)?
        .into_iter()
        .map(|arm| {
            let robot_type = state
                .workspace()
                .get(arm.id())
                .ok()
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
                            .map(|name| joint(name).and_then(|joint| joint.temperature_celsius()))
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
                                    .copied()
                                    .unwrap_or_default()
                            })
                            .collect(),
                    ),
            )
        })
        .collect();

    let writer = state
        .episodes()
        .begin_in_dataset(
            EpisodeRecordingRequest::new(dataset_id.clone(), source.clone(), sample_hz)
                .with_note(note)
                .with_arms(arms.clone(), condition),
        )
        .map_err(OperationError::from)?;
    let id = writer.id().to_owned();
    let started_ms = writer.started_ms();
    let summary = EpisodeSummary::new(
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
async fn start_recording_media_at_edge(
    state: &AppState,
    episode: &str,
    wanted: &[String],
) -> Result<Option<VideoCapture>, OperationError> {
    let video_settings = state.preferences().get().video().clone();
    if video_settings.enabled() {
        let snapshot = state.snapshot();
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
            state.cameras().clone(),
            VideoCaptureRequest::new(episode, cameras),
            state.video_encoder(),
        )
        .await
        .map_err(|error| OperationError::InvalidInput(error.to_string()))
    } else {
        Ok(None)
    }
}

async fn stop_recording(
    State(state): State<AppState>,
    Json(request): Json<StopRecordingRequest>,
) -> Result<Json<EpisodeSummary>, Failure> {
    let adapter = RecordingHttpAdapter::new(&state);
    let result =
        recording::stop_recording(&adapter, StopRecordingCommand::new(request.keep())).await?;
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
    State(state): State<AppState>,
    Json(request): Json<SetRecordingPausedRequest>,
) -> Result<StatusCode, Failure> {
    recording::set_recording_paused(
        &RecordingHttpAdapter::new(&state),
        SetRecordingPausedCommand::new(request.paused()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn judge_episode(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<JudgeEpisodeRequest>,
) -> Result<Json<EpisodeSummary>, Failure> {
    let result = episode_catalog::judge(
        &EpisodeHttpAdapter::catalog(&state),
        JudgeEpisodeCommand::new(id, request.discarded()),
    )
    .await?;
    Ok(Json(state.episodes().get(result.episode())?))
}

async fn delete_episode(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    episode_catalog::delete(
        &EpisodeHttpAdapter::catalog(&state),
        DeleteEpisodeCommand::new(id),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// How many points a trace is reduced to before it is sent.
#[derive(Debug, Deserialize)]
struct TraceQuery {
    #[serde(default = "default_trace_buckets")]
    buckets: usize,
}

fn default_trace_buckets() -> usize {
    240
}

async fn episode_trace(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TraceQuery>,
) -> Result<Json<Vec<Option<f64>>>, Failure> {
    let trace = tokio::task::spawn_blocking(move || {
        EpisodeQueries::new(&EpisodeHttpAdapter::catalog(&state))
            .follow_error_trace(EpisodeTraceQuery::new(id, query.buckets))
    })
    .await
    .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    Ok(Json(trace))
}

async fn episode_timeline_traces(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TraceQuery>,
) -> Result<Json<TimelineTraces>, Failure> {
    let traces = tokio::task::spawn_blocking(move || {
        EpisodeQueries::new(&EpisodeHttpAdapter::catalog(&state))
            .timeline_traces(EpisodeTraceQuery::new(id, query.buckets))
            .map(Into::into)
    })
    .await
    .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    Ok(Json(traces))
}

async fn episode_inspection(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<EpisodeInspectionSnapshot>, Failure> {
    let inspection = tokio::task::spawn_blocking(move || {
        EpisodeQueries::new(&EpisodeHttpAdapter::catalog(&state))
            .inspection(id)
            .map(Into::into)
    })
    .await
    .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    Ok(Json(inspection))
}

#[derive(Debug, Serialize)]
struct EpisodeTimelineResponse {
    episode: EpisodeSummary,
    samples: TimelineMetadata,
    videos: Vec<VideoAssetMetadata>,
}

async fn episode_timeline(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<EpisodeTimelineResponse>, Failure> {
    let result = tokio::task::spawn_blocking(move || {
        let timeline = EpisodeQueries::new(&EpisodeHttpAdapter::catalog(&state)).timeline(id)?;
        let (episode, samples, videos) = timeline.into_parts();
        Ok::<_, Failure>(EpisodeTimelineResponse {
            episode: episode.into(),
            samples: TimelineMetadata::new(
                samples.samples(),
                samples.first_seconds(),
                samples.last_seconds(),
            ),
            videos: videos.into_iter().map(Into::into).collect(),
        })
    })
    .await
    .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct TimelineSampleQuery {
    at: f64,
}

async fn episode_timeline_sample(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TimelineSampleQuery>,
) -> Result<Json<TimelineSample>, Failure> {
    let sample = tokio::task::spawn_blocking(move || {
        EpisodeQueries::new(&EpisodeHttpAdapter::catalog(&state))
            .timeline_sample(id, query.at)
            .map(Into::into)
    })
    .await
    .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    Ok(Json(sample))
}

#[derive(Debug, Deserialize)]
struct HistoricalSeekRequest {
    robot: String,
    #[serde(default)]
    source_arm: Option<String>,
    seconds: f64,
}

#[derive(Debug, Serialize)]
struct HistoricalSeekResponse {
    state: &'static str,
    episode: String,
    robot: String,
    source_arm: String,
    requested_seconds: f64,
    resolved_seconds: f64,
    frame: u64,
}

/// Move a physical follower to one committed historical sample.
///
/// This is intentionally distinct from visual sampling. The controller still
/// owns torque, calibration, joint compatibility, bounded ramping, obstruction
/// timeout, and disconnect handling. A ticket drops stale requests that finish
/// a SpiralDB read after a newer scrub; the one remaining pose enters the controller
/// as a one-frame replay, so no route can bypass those protections.
async fn seek_episode(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<HistoricalSeekRequest>,
) -> Result<Json<HistoricalSeekResponse>, Failure> {
    let result = episode_seek::seek(
        &SeekHttpAdapter { state: &state },
        SeekEpisodeCommand::new(id, request.robot, request.seconds)
            .with_source_arm(request.source_arm),
    )
    .await?;
    Ok(Json(HistoricalSeekResponse {
        state: match result.state() {
            HistoricalSeekState::Seeking => "seeking",
            HistoricalSeekState::Coalesced => "coalesced",
        },
        episode: result.episode().to_owned(),
        robot: result.robot().to_owned(),
        source_arm: result.source_arm().to_owned(),
        requested_seconds: result.requested_seconds(),
        resolved_seconds: result.resolved_seconds(),
        frame: result.frame(),
    }))
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

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct LocalVideoQuery {
    #[serde(default)]
    segment: Option<u32>,
}

async fn episode_video(
    State(state): State<AppState>,
    (Path((id, stream)), Query(query), headers): (
        Path<(String, String)>,
        Query<LocalVideoQuery>,
        HeaderMap,
    ),
) -> Result<Response, Failure> {
    // Looking up the episode first prevents an orphaned media row from becoming
    // an independent serving namespace.
    EpisodeQueries::new(&EpisodeHttpAdapter::catalog(&state)).authorize_asset(id.clone())?;
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
            .and_then(|value| parse_byte_range(value, length))
        {
            Some(range) => Some(range),
            None => return Ok(range_not_satisfiable(length)),
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

pub(super) fn parse_byte_range(value: &str, length: u64) -> Option<ByteRange> {
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
        return Some(ByteRange {
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
    (end >= start).then_some(ByteRange { start, end })
}

fn range_not_satisfiable(length: u64) -> Response {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(header::CONTENT_RANGE, format!("bytes */{length}"))
        .header(header::ACCEPT_RANGES, "bytes")
        .body(Body::empty())
        .expect("valid unsatisfied range response")
}

async fn start_replay(
    State(state): State<AppState>,
    Json(request): Json<StartReplayRequest>,
) -> Result<StatusCode, Failure> {
    recording::start_replay(
        &RecordingHttpAdapter::new(&state),
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
    State(state): State<AppState>,
    Json(request): Json<SetReplayPausedRequest>,
) -> Result<StatusCode, Failure> {
    recording::set_replay_paused(
        &RecordingHttpAdapter::new(&state),
        SetReplayPausedCommand::new(request.paused()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
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
            let lease = CatalogMutationLease::default();
            let error = reserve_recording_catalog_mutation(&lease, || activity)
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
        let lease = CatalogMutationLease::default();
        let recording = reserve_recording_catalog_mutation(&lease, || (false, false))
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
        assert!(lease.release(recording));
    }
}
