use super::*;
use spiral_robotics_studio_application::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken};

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
/// framework-neutral application action.
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

    fn present_recording(
        &self,
        result: RecordingResult,
    ) -> Result<EpisodeSummary, ApplicationError> {
        let summary = self
            .presented_recording
            .lock()
            .map_err(|_| {
                ApplicationError::Internal("recording presenter lock poisoned".to_owned())
            })?
            .take()
            .ok_or_else(|| {
                ApplicationError::Internal("recording result was not presented".to_owned())
            })?;
        if summary.id != result.episode {
            return Err(ApplicationError::Internal(
                "recording presenter returned a different episode".to_owned(),
            ));
        }
        Ok(summary)
    }
}

struct RecordingDraft {
    writer: EpisodeWriter,
    arms: Vec<spiral_robotics_contracts::api::EpisodeArm>,
    summary: EpisodeSummary,
}

impl RecordingPort for RecordingHttpAdapter<'_> {
    type RecordingDraft = RecordingDraft;
    type MediaCapture = VideoCapture;
    type MediaOutput = Vec<VideoAsset>;

    fn reserve_recording_start(&self) -> Option<CatalogLeaseToken> {
        self.state
            .catalog_lease
            .try_acquire(CatalogLeaseOwner::Recording)
    }

    fn bind_recording_lease(&self, token: CatalogLeaseToken, episode: String) {
        assert!(
            self.state.catalog_lease.bind_recording(token, episode),
            "the admitted recording must retain its exact catalog lease"
        );
    }

    fn claim_recording_lease(&self) -> Result<Option<CatalogLeaseToken>, ApplicationError> {
        if let Some(recording) = self.state.catalog_lease.claim_recording() {
            return Ok(Some(recording.token));
        }
        if self.state.catalog_lease.recording_is_opening() {
            return Err(ApplicationError::Conflict(
                "the recording is still opening; wait before stopping it".to_owned(),
            ));
        }
        Ok(None)
    }

    fn release_recording_lease(&self, token: CatalogLeaseToken) {
        let released = self.state.catalog_lease.release(token);
        debug_assert!(released, "recording releases its exact catalog lease once");
    }

    fn recording_active(&self) -> bool {
        self.state.hardware.state().telemetry.recording.is_some()
    }

    fn default_teleoperation_pair(&self) -> Option<TeleoperationPair> {
        default_teleoperation_pair(&build_snapshot(self.state).robots)
            .map(|(leader, follower)| TeleoperationPair { leader, follower })
    }

    fn recording_context(&self) -> RecordingContext {
        let hardware = self.state.hardware.state();
        RecordingContext {
            connected_arms: hardware
                .telemetry
                .robots
                .iter()
                .filter(|robot| {
                    hardware
                        .connections
                        .get(&robot.id)
                        .is_some_and(|connection| connection.connected)
                })
                .map(|robot| robot.id.clone())
                .collect(),
            sample_hz: if hardware.telemetry.target_hz > 0.0 {
                (hardware.telemetry.target_hz * 1_000.0).round() / 1_000.0
            } else {
                crate::backend::robotics::hardware::CONTROL_HZ as f64
            },
            media_finalizing: self
                .state
                .recording_video
                .try_lock()
                .map_or(true, |capture| capture.is_some()),
        }
    }

    async fn enter_teleoperation(&self, pair: &TeleoperationPair) -> Result<(), ApplicationError> {
        self.state
            .hardware
            .set_mode(ModeSelection::Teleoperate {
                leader: pair.leader.clone(),
                follower: pair.follower.clone(),
            })
            .await
            .map_err(ApplicationError::conflict)
    }

    async fn enter_idle(&self) -> Result<(), ApplicationError> {
        self.state
            .hardware
            .set_mode(ModeSelection::Idle)
            .await
            .map_err(ApplicationError::conflict)
    }

    fn create_recording_draft(
        &self,
        command: StartRecordingCommand,
        source: String,
        wanted: Vec<String>,
        sample_hz: f64,
    ) -> Result<RecordingDraft, ApplicationError> {
        create_recording_draft(self.state, command, source, wanted, sample_hz)
    }

    fn draft_episode(&self, draft: &Self::RecordingDraft) -> String {
        draft.summary.id.clone()
    }

    async fn start_recording_media(
        &self,
        episode: &str,
        arms: &[String],
    ) -> Result<Option<Self::MediaCapture>, ApplicationError> {
        start_recording_media_at_edge(self.state, episode, arms).await
    }

    async fn begin_recording(
        &self,
        draft: Self::RecordingDraft,
    ) -> Result<RecordingResult, ApplicationError> {
        self.state
            .hardware
            .start_recording(draft.writer, draft.arms)
            .await
            .map_err(ApplicationError::conflict)?;
        let result = RecordingResult {
            episode: draft.summary.id.clone(),
        };
        *self.presented_recording.lock().map_err(|_| {
            ApplicationError::Internal("recording presenter lock poisoned".to_owned())
        })? = Some(draft.summary);
        Ok(result)
    }

    async fn retain_recording_media(&self, capture: Option<Self::MediaCapture>) {
        *self.state.recording_video.lock().await = capture;
    }

    async fn abandon_recording_media(&self, capture: Self::MediaCapture) {
        let _ = capture.finish().await;
    }

    async fn take_recording_media(&self) -> Option<Self::MediaCapture> {
        self.state.recording_video.lock().await.take()
    }

    fn media_episode(&self, capture: &Self::MediaCapture) -> String {
        capture.episode_id().to_owned()
    }

    async fn stop_recording(
        &self,
        command: StopRecordingCommand,
    ) -> Result<RecordingResult, ApplicationError> {
        let summary = self
            .state
            .hardware
            .stop_recording(command.keep)
            .await
            .map_err(ApplicationError::conflict)?;
        let result = RecordingResult {
            episode: summary.id.clone(),
        };
        *self.presented_recording.lock().map_err(|_| {
            ApplicationError::Internal("recording presenter lock poisoned".to_owned())
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
    ) -> Result<(), ApplicationError> {
        let completed = self
            .state
            .episodes
            .completed(&episode)
            .map_err(episode_application_error)?;
        self.state
            .storage
            .commit_episode_blocking(&completed, &media)
            .map_err(ApplicationError::Internal)
    }

    async fn request_media_pause(&self) {
        if let Some(capture) = self.state.recording_video.lock().await.as_ref() {
            capture.request_pause();
        }
    }

    async fn set_recording_paused(&self, paused: bool) -> Result<f64, ApplicationError> {
        self.state
            .hardware
            .set_recording_paused(paused)
            .await
            .map_err(ApplicationError::conflict)
    }

    async fn finish_media_pause(&self) {
        let checkpoint = if let Some(capture) = self.state.recording_video.lock().await.as_mut() {
            for failure in capture.finish_pause().await {
                self.state
                    .cameras
                    .note_issue(format!("paused episode video was not saved: {failure}"));
            }
            Some((capture.episode_id().to_owned(), capture.completed_assets()))
        } else {
            None
        };
        let Some((episode, media)) = checkpoint else {
            return;
        };
        let episodes = self.state.episodes.clone();
        let storage = self.state.storage.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let completed = episodes
                .completed(&episode)
                .map_err(|error| error.to_string())?;
            storage.commit_episode_blocking(&completed, &media)
        })
        .await
        .unwrap_or_else(|error| Err(format!("video checkpoint task stopped: {error}")));
        if let Err(error) = outcome {
            self.state.cameras.note_issue(format!(
                "paused episode video was not saved to SpiralDB: {error}"
            ));
        }
    }

    async fn resume_media(&self, active_seconds: f64) {
        if let Some(capture) = self.state.recording_video.lock().await.as_mut()
            && let Err(error) = capture.resume(active_seconds)
        {
            self.state
                .cameras
                .note_issue(format!("episode video could not resume: {error}"));
        }
    }
}

impl ReplayPort for RecordingHttpAdapter<'_> {
    fn replay_episode(&self, episode: &str) -> Result<ReplayEpisode, ApplicationError> {
        self.state
            .episodes
            .get(episode)
            .map(|summary| ReplayEpisode {
                id: summary.id,
                arms: summary.arms.into_iter().map(|arm| arm.id).collect(),
                sample_hz: summary.sample_hz,
            })
            .map_err(episode_application_error)
    }

    async fn load_trajectory(
        &self,
        episode: &str,
        source_arm: &str,
        start_seconds: f64,
    ) -> Result<ReplayTrajectory, ApplicationError> {
        let episodes = self.state.episodes.clone();
        let episode = episode.to_owned();
        let source_arm = source_arm.to_owned();
        tokio::task::spawn_blocking(move || {
            episodes.trajectory_at(&episode, &source_arm, start_seconds)
        })
        .await
        .map_err(|error| ApplicationError::Internal(error.to_string()))?
        .map(|trajectory| ReplayTrajectory {
            frames: trajectory.frames,
            start_frame: trajectory.start_frame,
            resolved_start_seconds: trajectory.resolved_start_seconds,
        })
        .map_err(episode_application_error)
    }

    async fn start_replay(&self, plan: ReplayPlan) -> Result<(), ApplicationError> {
        self.state
            .hardware
            .start_replay_at_frame(
                &plan.robot,
                &plan.episode,
                plan.frames,
                plan.sample_hz,
                plan.start_frame,
                plan.resolved_start_seconds,
            )
            .await
            .map_err(ApplicationError::conflict)
    }

    async fn set_replay_paused(&self, paused: bool) -> Result<(), ApplicationError> {
        self.state
            .hardware
            .set_replay_paused(paused)
            .await
            .map_err(ApplicationError::conflict)
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
        let hardware = self.state.hardware.state();
        EpisodeActivity {
            recording: hardware
                .telemetry
                .recording
                .map(|recording| recording.episode),
            replaying: match hardware.mode.selection {
                ModeSelection::Replay { episode, .. } => Some(episode),
                _ => None,
            },
        }
    }

    async fn judge_episode(
        &self,
        episode: String,
        discarded: bool,
    ) -> Result<EpisodeMutationResult, ApplicationError> {
        let episodes = self.state.episodes.clone();
        let summary = tokio::task::spawn_blocking(move || episodes.judge(&episode, discarded))
            .await
            .map_err(|error| ApplicationError::Internal(error.to_string()))?
            .map_err(episode_application_error)?;
        Ok(EpisodeMutationResult {
            episode: summary.id,
        })
    }

    async fn delete_episode(&self, episode: String) -> Result<(), ApplicationError> {
        let episodes = self.state.episodes.clone();
        tokio::task::spawn_blocking(move || episodes.delete(&episode))
            .await
            .map_err(|error| ApplicationError::Internal(error.to_string()))?
            .map_err(episode_application_error)?;

        Ok(())
    }
}

impl EpisodeQueryPort for EpisodeHttpAdapter<'_> {
    fn episodes(&self) -> Vec<EpisodeView> {
        self.state
            .episodes
            .list()
            .into_iter()
            .map(application_episode)
            .collect()
    }

    fn ensure_episode_exists(&self, episode: &str) -> Result<(), ApplicationError> {
        self.state
            .episodes
            .get(episode)
            .map(|_| ())
            .map_err(episode_application_error)
    }

    fn follow_error_trace(
        &self,
        query: EpisodeTraceQuery,
    ) -> Result<Vec<Option<f64>>, ApplicationError> {
        self.state
            .episodes
            .trace(&query.episode, query.buckets)
            .map_err(episode_application_error)
    }

    fn timeline_traces(
        &self,
        query: EpisodeTraceQuery,
    ) -> Result<TimelineTracesView, ApplicationError> {
        self.state
            .episodes
            .timeline_traces(&query.episode, query.buckets)
            .map(application_timeline_traces)
            .map_err(episode_application_error)
    }

    fn timeline(&self, episode: String) -> Result<EpisodeTimelineView, ApplicationError> {
        let samples = self
            .state
            .episodes
            .timeline_metadata(&episode)
            .map_err(episode_application_error)?;
        // Metadata refreshes a stale partial entry. Read the summary after it
        // so both projections describe the same durable episode.
        let summary = self
            .state
            .episodes
            .get(&episode)
            .map_err(episode_application_error)?;
        let videos = self
            .state
            .episodes
            .video_metadata(&episode)
            .map_err(episode_application_error)?
            .into_iter()
            .map(application_video_asset)
            .collect();
        Ok(EpisodeTimelineView {
            episode: application_episode(summary),
            samples: TimelineMetadataView {
                samples: samples.samples,
                first_seconds: samples.first_seconds,
                last_seconds: samples.last_seconds,
            },
            videos,
        })
    }

    fn inspection(&self, episode: String) -> Result<EpisodeInspectionView, ApplicationError> {
        self.state
            .episodes
            .inspection(&episode)
            .map(application_episode_inspection)
            .map_err(episode_application_error)
    }

    fn timeline_sample(
        &self,
        episode: String,
        seconds: f64,
    ) -> Result<TimelineSampleView, ApplicationError> {
        self.state
            .episodes
            .sample_at(&episode, seconds)
            .map(application_timeline_sample)
            .map_err(episode_application_error)
    }
}

fn application_episode(episode: EpisodeSummary) -> EpisodeView {
    EpisodeView {
        id: episode.id,
        dataset_id: episode.dataset_id,
        arms: episode
            .arms
            .into_iter()
            .map(|arm| EpisodeArmView {
                id: arm.id,
                robot_type: arm.robot_type,
                joints: arm.joints,
            })
            .collect(),
        frames: episode.frames,
        duration_seconds: episode.duration_seconds,
        recording_gaps: episode
            .recording_gaps
            .into_iter()
            .map(|gap| RecordingGapView {
                after_seconds: gap.after_seconds,
                duration_seconds: gap.duration_seconds,
            })
            .collect(),
        sample_hz: episode.sample_hz,
        started_ms: episode.started_ms,
        source: episode.source,
        max_follow_error_degrees: episode.max_follow_error_degrees,
        discarded: episode.discarded,
        problem: episode.problem,
    }
}

pub(super) fn contract_episode(episode: EpisodeView) -> EpisodeSummary {
    EpisodeSummary {
        id: episode.id,
        dataset_id: episode.dataset_id,
        arms: episode
            .arms
            .into_iter()
            .map(|arm| spiral_robotics_contracts::api::EpisodeArm {
                id: arm.id,
                robot_type: arm.robot_type,
                joints: arm.joints,
            })
            .collect(),
        frames: episode.frames,
        duration_seconds: episode.duration_seconds,
        recording_gaps: episode
            .recording_gaps
            .into_iter()
            .map(|gap| spiral_robotics_contracts::api::RecordingGap {
                after_seconds: gap.after_seconds,
                duration_seconds: gap.duration_seconds,
            })
            .collect(),
        sample_hz: episode.sample_hz,
        started_ms: episode.started_ms,
        source: episode.source,
        max_follow_error_degrees: episode.max_follow_error_degrees,
        discarded: episode.discarded,
        problem: episode.problem,
    }
}

fn application_video_asset(video: VideoAssetMetadata) -> VideoAssetView {
    VideoAssetView {
        stream_id: video.stream_id,
        camera_id: video.camera_id,
        width: video.width,
        height: video.height,
        fps: video.fps,
        frames: video.frames,
        duration_seconds: video.duration_seconds,
        segment_index: video.segment_index,
        timeline_offset_seconds: video.timeline_offset_seconds,
    }
}

fn application_episode_inspection(inspection: EpisodeInspection) -> EpisodeInspectionView {
    let samples = &inspection.episode.samples;
    let arms = inspection
        .episode
        .start
        .arms
        .iter()
        .enumerate()
        .map(|(arm_index, arm)| {
            let joint_count = arm.joints.len();
            EpisodeInspectionArmView {
                arm: arm.id.clone(),
                joints: arm.joints.clone(),
                normalized: samples
                    .iter()
                    .flat_map(|sample| sample.normalized[arm_index].iter().copied())
                    .collect(),
                raw: samples
                    .iter()
                    .flat_map(|sample| sample.raw[arm_index].iter().copied())
                    .collect(),
                velocity: application_optional_frame_matrix(
                    samples,
                    arm_index,
                    joint_count,
                    |sample, index| {
                        sample
                            .velocity
                            .as_ref()
                            .and_then(|arms| arms.get(index))
                            .map(Vec::as_slice)
                    },
                ),
                effort: application_optional_frame_matrix(
                    samples,
                    arm_index,
                    joint_count,
                    |sample, index| {
                        sample
                            .effort
                            .as_ref()
                            .and_then(|arms| arms.get(index))
                            .map(Vec::as_slice)
                    },
                ),
                action: application_optional_frame_matrix(
                    samples,
                    arm_index,
                    joint_count,
                    |sample, index| {
                        sample
                            .action
                            .get(index)
                            .and_then(Option::as_ref)
                            .map(Vec::as_slice)
                    },
                ),
            }
        })
        .collect();
    EpisodeInspectionView {
        episode: application_episode(inspection.summary),
        sample_times_seconds: samples.iter().map(|sample| sample.seconds).collect(),
        follow_error_degrees: samples
            .iter()
            .map(|sample| sample.follow_error_degrees)
            .collect(),
        arms,
        videos: inspection
            .videos
            .into_iter()
            .map(application_video_asset)
            .collect(),
    }
}

fn application_optional_frame_matrix(
    samples: &[crate::backend::recording::episodes::Sample],
    arm_index: usize,
    joint_count: usize,
    values_at: impl for<'a> Fn(
        &'a crate::backend::recording::episodes::Sample,
        usize,
    ) -> Option<&'a [Option<f64>]>,
) -> OptionalFrameMatrixView {
    let mut present = Vec::with_capacity(samples.len());
    let mut values = Vec::with_capacity(samples.len().saturating_mul(joint_count));
    for sample in samples {
        match values_at(sample, arm_index) {
            Some(frame) => {
                debug_assert_eq!(frame.len(), joint_count);
                present.push(true);
                values.extend(frame.iter().copied());
            }
            None => {
                present.push(false);
                values.extend(std::iter::repeat_n(None, joint_count));
            }
        }
    }
    OptionalFrameMatrixView { present, values }
}

fn contract_video_asset(video: VideoAssetView) -> VideoAssetMetadata {
    VideoAssetMetadata {
        stream_id: video.stream_id,
        camera_id: video.camera_id,
        width: video.width,
        height: video.height,
        fps: video.fps,
        frames: video.frames,
        duration_seconds: video.duration_seconds,
        segment_index: video.segment_index,
        timeline_offset_seconds: video.timeline_offset_seconds,
    }
}

fn contract_episode_inspection(inspection: EpisodeInspectionView) -> EpisodeInspectionSnapshot {
    EpisodeInspectionSnapshot {
        episode: contract_episode(inspection.episode),
        sample_times_seconds: inspection.sample_times_seconds,
        follow_error_degrees: inspection.follow_error_degrees,
        arms: inspection
            .arms
            .into_iter()
            .map(|arm| EpisodeInspectionArm {
                arm: arm.arm,
                joints: arm.joints,
                normalized: arm.normalized,
                raw: arm.raw,
                velocity: contract_optional_frame_matrix(arm.velocity),
                effort: contract_optional_frame_matrix(arm.effort),
                action: contract_optional_frame_matrix(arm.action),
            })
            .collect(),
        videos: inspection
            .videos
            .into_iter()
            .map(|video| EpisodeVideoAssetMetadata {
                stream_id: video.stream_id,
                camera_id: video.camera_id,
                width: video.width,
                height: video.height,
                fps: video.fps,
                frames: video.frames,
                duration_seconds: video.duration_seconds,
                segment_index: video.segment_index,
                timeline_offset_seconds: video.timeline_offset_seconds,
            })
            .collect(),
    }
}

fn contract_optional_frame_matrix(matrix: OptionalFrameMatrixView) -> EpisodeOptionalFrameMatrix {
    EpisodeOptionalFrameMatrix {
        present: matrix.present,
        values: matrix.values,
    }
}

fn application_timeline_sample(sample: TimelineSample) -> TimelineSampleView {
    TimelineSampleView {
        frame: sample.frame,
        seconds: sample.seconds,
        follow_error_degrees: sample.follow_error_degrees,
        arms: sample
            .arms
            .into_iter()
            .map(|arm| TimelineArmSampleView {
                arm: arm.arm,
                normalized: arm.normalized,
                raw: arm.raw,
                velocity: arm.velocity,
                effort: arm.effort,
                action: arm.action,
            })
            .collect(),
    }
}

fn contract_timeline_sample(sample: TimelineSampleView) -> TimelineSample {
    TimelineSample {
        frame: sample.frame,
        seconds: sample.seconds,
        follow_error_degrees: sample.follow_error_degrees,
        arms: sample
            .arms
            .into_iter()
            .map(
                |arm| crate::backend::recording::episodes::TimelineArmSample {
                    arm: arm.arm,
                    normalized: arm.normalized,
                    raw: arm.raw,
                    velocity: arm.velocity,
                    effort: arm.effort,
                    action: arm.action,
                },
            )
            .collect(),
    }
}

fn application_timeline_traces(traces: TimelineTraces) -> TimelineTracesView {
    TimelineTracesView {
        episode: traces.episode,
        samples: traces.samples,
        buckets: traces.buckets,
        start_seconds: traces.start_seconds,
        end_seconds: traces.end_seconds,
        lanes: traces
            .lanes
            .into_iter()
            .map(|lane| TimelineTraceLaneView {
                joint: lane.joint,
                series: lane
                    .series
                    .into_iter()
                    .map(|series| TimelineTraceSeriesView {
                        arm: series.arm,
                        points: series
                            .points
                            .into_iter()
                            .map(|point| TimelineTracePointView {
                                seconds: point.seconds,
                                value: point.value,
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn contract_timeline_traces(traces: TimelineTracesView) -> TimelineTraces {
    TimelineTraces {
        episode: traces.episode,
        samples: traces.samples,
        buckets: traces.buckets,
        start_seconds: traces.start_seconds,
        end_seconds: traces.end_seconds,
        lanes: traces
            .lanes
            .into_iter()
            .map(
                |lane| crate::backend::recording::episodes::TimelineTraceLane {
                    joint: lane.joint,
                    series: lane
                        .series
                        .into_iter()
                        .map(
                            |series| crate::backend::recording::episodes::TimelineTraceSeries {
                                arm: series.arm,
                                points: series
                                    .points
                                    .into_iter()
                                    .map(|point| {
                                        crate::backend::recording::episodes::TimelineTracePoint {
                                            seconds: point.seconds,
                                            value: point.value,
                                        }
                                    })
                                    .collect(),
                            },
                        )
                        .collect(),
                },
            )
            .collect(),
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
        let hardware = self.state.hardware.state();
        SeekActivity {
            recording: hardware
                .telemetry
                .recording
                .map(|recording| ActiveRecording {
                    episode: recording.episode,
                    paused: recording.paused,
                }),
            mode: match hardware.mode.selection {
                ModeSelection::Idle => SeekControlMode::Idle,
                ModeSelection::Replay { .. } => SeekControlMode::Replay,
                _ => SeekControlMode::Other,
            },
        }
    }

    fn target(&self, robot: &str) -> Result<Option<SeekTarget>, ApplicationError> {
        Ok(build_snapshot(self.state)
            .robots
            .into_iter()
            .find(|candidate| candidate.id == robot)
            .map(|target| SeekTarget {
                id: target.id,
                robot_type: target.robot_type,
                connected: target.connection.connected,
                calibrated: target.calibrated,
                problem: target.problem,
            }))
    }

    fn episode(&self, episode: &str) -> Result<SeekableEpisode, ApplicationError> {
        self.state
            .episodes
            .get(episode)
            .map(|summary| SeekableEpisode {
                id: summary.id,
                arms: summary.arms.into_iter().map(|arm| arm.id).collect(),
                sample_hz: summary.sample_hz,
                problem: summary.problem,
                unfinished: false,
            })
            .map_err(episode_application_error)
    }

    fn reserve_seek(&self, robot: &str) -> u64 {
        self.state.episodes.reserve_seek(robot)
    }

    async fn sample_at(
        &self,
        episode: &str,
        seconds: f64,
    ) -> Result<HistoricalSample, ApplicationError> {
        let episodes = self.state.episodes.clone();
        let episode = episode.to_owned();
        tokio::task::spawn_blocking(move || episodes.sample_at(&episode, seconds))
            .await
            .map_err(|error| ApplicationError::Internal(error.to_string()))?
            .map(|sample| HistoricalSample {
                seconds: sample.seconds,
                frame: sample.frame,
                arms: sample
                    .arms
                    .into_iter()
                    .map(|arm| SampledArm {
                        id: arm.arm,
                        normalized: arm.normalized,
                    })
                    .collect(),
            })
            .map_err(episode_application_error)
    }

    async fn commit_if_current(&self, plan: PhysicalSeekPlan) -> Result<bool, ApplicationError> {
        let _gate = self.state.episodes.lock_seek().await;
        if !self
            .state
            .episodes
            .seek_is_current(&plan.robot, plan.ticket)
        {
            return Ok(false);
        }
        self.state
            .hardware
            .start_physical_seek(&plan.robot, &plan.episode, plan.pose, plan.sample_hz)
            .await
            .map_err(ApplicationError::conflict)?;
        Ok(true)
    }
}

async fn list_episodes(State(state): State<AppState>) -> Json<EpisodeCatalogResponse> {
    // A paused take has a readable SpiralDB checkpoint. Hide its catalog row
    // here because live recording state is delivered through telemetry.
    let recording = state
        .hardware
        .state()
        .telemetry
        .recording
        .map(|progress| progress.episode);
    let episodes = episode_queries::list(&EpisodeHttpAdapter::catalog(&state))
        .into_iter()
        .filter(|episode| Some(&episode.id) != recording.as_ref())
        .map(contract_episode)
        .collect();
    Json(EpisodeCatalogResponse {
        episodes,
        issues: state.episodes.issues(),
    })
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
        StartRecordingCommand {
            dataset_id: request.dataset_id,
            arms: request.arms,
            note: request.note,
        },
    )
    .await?;
    Ok(Json(adapter.present_recording(result)?))
}

/// Assemble a storage draft from authoritative live telemetry.
///
/// Eligibility, sample-rate admission, consequential ordering, and cleanup
/// belong to the application action. This adapter only translates live backend
/// structures into the episode writer's concrete representation.
fn create_recording_draft(
    state: &AppState,
    command: StartRecordingCommand,
    source: String,
    wanted: Vec<String>,
    sample_hz: f64,
) -> Result<RecordingDraft, ApplicationError> {
    let hardware = state.hardware.state();
    let connected = hardware
        .telemetry
        .robots
        .iter()
        .filter(|robot| {
            hardware
                .connections
                .get(&robot.id)
                .is_some_and(|connection| connection.connected)
        })
        .collect::<Vec<_>>();

    let arms = wanted
        .iter()
        .map(|id| {
            let robot = connected
                .iter()
                .find(|robot| &robot.id == id)
                .ok_or_else(|| {
                    ApplicationError::Conflict(format!(
                        "robot {id:?} disconnected while the recording was opening"
                    ))
                })?;
            Ok((
                id.clone(),
                robot
                    .joints
                    .iter()
                    .map(|joint| joint.name.clone())
                    .collect::<Vec<_>>(),
            ))
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    let mut arms = build_arms(&arms).map_err(episode_application_error)?;
    for arm in &mut arms {
        arm.robot_type = state
            .workspace
            .get(&arm.id)
            .ok()
            .and_then(|robot| robot.robot_type());
    }

    // What the motors were like as the take opened. Recorded once here rather
    // than on every frame: these move over seconds, and the footer's extremes
    // close the story the header starts.
    let condition = arms
        .iter()
        .filter_map(|arm| {
            let robot = connected.iter().find(|robot| robot.id == arm.id)?;
            let limits = hardware.limits.get(&arm.id);
            let joint = |name: &str| robot.joints.iter().find(|joint| joint.name == *name);
            Some(ArmCondition {
                arm: arm.id.clone(),
                temperature_celsius: arm
                    .joints
                    .iter()
                    .map(|name| joint(name).and_then(|joint| joint.temperature_celsius))
                    .collect(),
                voltage_volts: arm
                    .joints
                    .iter()
                    .map(|name| joint(name).and_then(|joint| joint.voltage_volts))
                    .collect(),
                limits: arm
                    .joints
                    .iter()
                    .map(|name| {
                        limits
                            .and_then(|limits| limits.get(name))
                            .copied()
                            .unwrap_or_default()
                    })
                    .collect(),
            })
        })
        .collect();

    let writer = state
        .episodes
        .begin_in_dataset(
            command.dataset_id.clone(),
            source.clone(),
            command.note,
            sample_hz,
            arms.clone(),
            condition,
        )
        .map_err(episode_application_error)?;
    let id = writer.id().to_owned();
    let started_ms = writer.started_ms();
    let summary = EpisodeSummary {
        id,
        dataset_id: command.dataset_id,
        arms: arms.clone(),
        frames: 0,
        duration_seconds: 0.0,
        recording_gaps: Vec::new(),
        sample_hz,
        started_ms,
        source,
        max_follow_error_degrees: None,
        discarded: false,
        problem: None,
    };
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
) -> Result<Option<VideoCapture>, ApplicationError> {
    let video_settings = state.preferences.get().video;
    if video_settings.enabled {
        let snapshot = build_snapshot(state);
        let camera_status = snapshot
            .cameras
            .iter()
            .map(|camera| (camera.id.as_str(), camera))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut seen = BTreeSet::new();
        let mut cameras = Vec::new();
        for robot in snapshot
            .robots
            .iter()
            .filter(|robot| wanted.contains(&robot.id))
        {
            for camera in &robot.cameras {
                if !seen.insert(camera.id.clone()) {
                    continue;
                }
                let Some(status) = camera_status.get(camera.id.as_str()) else {
                    continue;
                };
                cameras.push(CameraCaptureSpec {
                    stream_id: format!("{}.camera.{}", robot.id, camera.name),
                    camera_id: camera.id.clone(),
                    width: status.width,
                    height: status.height,
                    fps: status.fps,
                });
            }
        }
        VideoCapture::start(
            state.cameras.clone(),
            episode,
            cameras,
            &state.video_encoder.executable,
            &state.video_encoder.codec,
        )
        .await
        .map_err(|error| ApplicationError::InvalidInput(error.to_string()))
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
        recording::stop_recording(&adapter, StopRecordingCommand { keep: request.keep }).await?;
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
        SetRecordingPausedCommand {
            paused: request.paused,
        },
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
        JudgeEpisodeCommand {
            episode: id,
            discarded: request.discarded,
        },
    )
    .await?;
    Ok(Json(state.episodes.get(&result.episode)?))
}

async fn delete_episode(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    episode_catalog::delete(
        &EpisodeHttpAdapter::catalog(&state),
        DeleteEpisodeCommand { episode: id },
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
        episode_queries::follow_error_trace(
            &EpisodeHttpAdapter::catalog(&state),
            EpisodeTraceQuery {
                episode: id,
                buckets: query.buckets,
            },
        )
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
        episode_queries::timeline_traces(
            &EpisodeHttpAdapter::catalog(&state),
            EpisodeTraceQuery {
                episode: id,
                buckets: query.buckets,
            },
        )
        .map(contract_timeline_traces)
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
        episode_queries::inspection(&EpisodeHttpAdapter::catalog(&state), id)
            .map(contract_episode_inspection)
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
        let timeline = episode_queries::timeline(&EpisodeHttpAdapter::catalog(&state), id)?;
        Ok::<_, Failure>(EpisodeTimelineResponse {
            episode: contract_episode(timeline.episode),
            samples: TimelineMetadata {
                samples: timeline.samples.samples,
                first_seconds: timeline.samples.first_seconds,
                last_seconds: timeline.samples.last_seconds,
            },
            videos: timeline
                .videos
                .into_iter()
                .map(contract_video_asset)
                .collect(),
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
        episode_queries::timeline_sample(&EpisodeHttpAdapter::catalog(&state), id, query.at)
            .map(contract_timeline_sample)
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
        SeekEpisodeCommand {
            episode: id,
            robot: request.robot,
            source_arm: request.source_arm,
            seconds: request.seconds,
        },
    )
    .await?;
    Ok(Json(HistoricalSeekResponse {
        state: match result.state {
            HistoricalSeekState::Seeking => "seeking",
            HistoricalSeekState::Coalesced => "coalesced",
        },
        episode: result.episode,
        robot: result.robot,
        source_arm: result.source_arm,
        requested_seconds: result.requested_seconds,
        resolved_seconds: result.resolved_seconds,
        frame: result.frame,
    }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ByteRange {
    pub(super) start: u64,
    pub(super) end: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct LocalVideoQuery {
    #[serde(default)]
    segment: Option<u32>,
}

async fn episode_video(
    State(state): State<AppState>,
    Path((id, stream)): Path<(String, String)>,
    Query(query): Query<LocalVideoQuery>,
    headers: HeaderMap,
) -> Result<Response, Failure> {
    // Looking up the episode first prevents an orphaned media row from becoming
    // an independent serving namespace.
    episode_queries::authorize_asset(&EpisodeHttpAdapter::catalog(&state), id.clone())?;
    let length = state
        .storage
        .video_asset_size(&id, &stream, query.segment)
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
            .storage
            .video_asset_range(&id, &stream, query.segment, range.start, content_length)
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
        StartReplayCommand {
            episode: request.episode,
            robot: request.robot,
            source_arm: request.source_arm,
            start_seconds: request.start_seconds,
        },
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
        SetReplayPausedCommand {
            paused: request.paused,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
