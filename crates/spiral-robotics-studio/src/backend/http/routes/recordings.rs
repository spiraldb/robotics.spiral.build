use super::*;
use crate::backend::application::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/episodes", get(list_episodes))
        .route("/episodes/record", post(start_recording))
        .route("/episodes/record/pause", post(set_recording_paused))
        .route("/episodes/record/stop", post(stop_recording))
        .route("/episodes/replay", post(start_replay))
        .route("/episodes/replay/pause", post(set_replay_paused))
        .route("/episodes/export", post(export_episodes))
        .route("/episodes/export.zip", get(download_episodes))
        .route(
            "/episodes/import",
            post(import_episodes).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route("/episodes/{id}", axum::routing::delete(delete_episode))
        .route("/episodes/{id}/judge", post(judge_episode))
        .route("/episodes/{id}/trace", get(episode_trace))
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

    async fn publish_recording_media(&self, capture: Option<Self::MediaCapture>) {
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

    fn reload_episodes(&self) -> Result<(), ApplicationError> {
        self.state
            .episodes
            .reload()
            .map_err(episode_application_error)
    }

    fn stage_recording(
        &self,
        episode: String,
        media: Self::MediaOutput,
    ) -> Result<(), ApplicationError> {
        self.state
            .storage
            .stage_local(episode, media)
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
        if let Some(capture) = self.state.recording_video.lock().await.as_mut() {
            for failure in capture.finish_pause().await {
                self.state
                    .cameras
                    .note_issue(format!("paused episode video was not saved: {failure}"));
            }
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

/// Filesystem and control-state adapter for episode catalog actions.
///
/// Application policy selects and admits the mutation first. Blocking JSONL
/// work and best-effort video sidecar cleanup remain at this outer boundary.
pub(super) struct EpisodeHttpAdapter<'a> {
    state: &'a AppState,
    export_destination: Option<std::path::PathBuf>,
}

impl<'a> EpisodeHttpAdapter<'a> {
    pub(super) fn catalog(state: &'a AppState) -> Self {
        Self {
            state,
            export_destination: None,
        }
    }

    fn export(state: &'a AppState, destination: std::path::PathBuf) -> Self {
        Self {
            state,
            export_destination: Some(destination),
        }
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

    fn export_candidates(&self) -> Result<Vec<ExportCandidate>, ApplicationError> {
        Ok(self
            .state
            .episodes
            .list()
            .into_iter()
            .map(|episode| {
                let usable = episode.usable();
                ExportCandidate {
                    id: episode.id,
                    usable,
                    discarded: episode.discarded,
                }
            })
            .collect())
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
        let video_directory = self
            .state
            .episodes
            .directory()
            .join("videos")
            .join(&episode);
        let episodes = self.state.episodes.clone();
        tokio::task::spawn_blocking(move || episodes.delete(&episode))
            .await
            .map_err(|error| ApplicationError::Internal(error.to_string()))?
            .map_err(episode_application_error)?;

        // The JSONL entry is authoritative. A stale media sidecar must not
        // resurrect it or turn a successful catalog deletion into a failure.
        if let Err(error) = tokio::fs::remove_dir_all(&video_directory).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                video_directory = %video_directory.display(),
                "episode JSONL was deleted but its video sidecar could not be removed: {error}"
            );
        }
        Ok(())
    }

    async fn export_episodes(
        &self,
        selected: Vec<String>,
    ) -> Result<ExportEpisodesResult, ApplicationError> {
        let destination = self.export_destination.clone().ok_or_else(|| {
            ApplicationError::Internal("episode export adapter has no destination".to_owned())
        })?;
        let episodes = self.state.episodes.clone();
        let response =
            tokio::task::spawn_blocking(move || episodes.export_selected(&destination, &selected))
                .await
                .map_err(|error| ApplicationError::Internal(error.to_string()))?
                .map_err(episode_application_error)?;
        Ok(ExportEpisodesResult {
            episodes: response.episodes,
            frames: response.frames,
            skipped: response.skipped,
        })
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
        let videos = load_assets(self.state.episodes.directory(), &episode)
            .map_err(ApplicationError::Internal)?
            .iter()
            .map(VideoAsset::metadata)
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
        file: episode.file,
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
        file: episode.file,
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

struct DatasetDownload {
    temporary: tempfile::NamedTempFile,
    length: u64,
}

/// Archive and catalog adapter for application-owned dataset interchange.
///
/// An import source is captured by the adapter so the application command
/// contains only business input. Its staged value remains opaque until the use
/// case authorizes the all-or-none commit.
struct DatasetHttpAdapter<'a> {
    state: &'a AppState,
    import_source: Option<std::sync::Mutex<Option<(tempfile::NamedTempFile, std::fs::File)>>>,
    export_artifact: Option<std::sync::Mutex<Option<DatasetDownload>>>,
}

impl<'a> DatasetHttpAdapter<'a> {
    fn export(state: &'a AppState) -> Self {
        Self {
            state,
            import_source: None,
            export_artifact: Some(std::sync::Mutex::new(None)),
        }
    }

    fn import(
        state: &'a AppState,
        temporary: tempfile::NamedTempFile,
        source: std::fs::File,
    ) -> Self {
        Self {
            state,
            import_source: Some(std::sync::Mutex::new(Some((temporary, source)))),
            export_artifact: None,
        }
    }

    fn take_export_artifact(&self) -> Result<DatasetDownload, ApplicationError> {
        self.export_artifact
            .as_ref()
            .ok_or_else(|| ApplicationError::Internal("missing dataset export sink".to_owned()))?
            .lock()
            .map_err(|_| ApplicationError::Internal("dataset export sink poisoned".to_owned()))?
            .take()
            .ok_or_else(|| {
                ApplicationError::Internal("dataset export produced no artifact".to_owned())
            })
    }
}

impl DatasetPort for DatasetHttpAdapter<'_> {
    type PreparedExport = PreparedDatasetExport;
    type StagedImport = CodecStagedImport;

    fn reserve_catalog_mutation(&self) -> Option<CatalogLeaseToken> {
        self.state
            .catalog_lease
            .try_acquire(CatalogLeaseOwner::DatasetImport)
    }

    fn release_catalog_mutation(&self, token: CatalogLeaseToken) {
        let released = self.state.catalog_lease.release(token);
        debug_assert!(
            released,
            "dataset import releases its exact catalog lease once"
        );
    }

    fn activity(&self) -> DatasetActivity {
        DatasetActivity {
            recording: self
                .state
                .hardware
                .state()
                .telemetry
                .recording
                .map(|recording| recording.episode),
            catalog_mutation_active: self.state.catalog_lease.is_held(),
        }
    }

    fn export_candidates(&self) -> Result<Vec<DatasetCandidate>, ApplicationError> {
        Ok(self
            .state
            .episodes
            .list()
            .into_iter()
            .map(|episode| {
                let usable = episode.usable();
                DatasetCandidate {
                    id: episode.id,
                    dataset_id: episode.dataset_id,
                    usable,
                    discarded: episode.discarded,
                }
            })
            .collect())
    }

    async fn inspect_export(
        &self,
        episode_ids: Vec<String>,
    ) -> Result<InspectedDatasetExport<Self::PreparedExport>, ApplicationError> {
        let episodes = self.state.episodes.clone();
        let prepared =
            tokio::task::spawn_blocking(move || prepare_lerobot_export(&episodes, &episode_ids))
                .await
                .map_err(|error| ApplicationError::Internal(error.to_string()))?
                .map_err(episode_application_error)?;
        let inspections = prepared
            .inspections()
            .iter()
            .map(|episode| ExportEpisodeInspection {
                id: episode.id.clone(),
                frames: episode.frames,
                joints: episode.joints.clone(),
                sample_hz: episode.sample_hz,
                complete_observations: episode.complete_observations,
                complete_actions: episode.complete_actions,
                complete_velocity: episode.complete_velocity,
                complete_effort: episode.complete_effort,
            })
            .collect();
        Ok(InspectedDatasetExport {
            prepared,
            episodes: inspections,
        })
    }

    async fn write_export(
        &self,
        prepared: Self::PreparedExport,
        plan: ExportCompatibilityPlan,
    ) -> Result<(), ApplicationError> {
        let inspected_ids = prepared
            .inspections()
            .iter()
            .map(|episode| episode.id.clone())
            .collect::<Vec<_>>();
        if inspected_ids != plan.episode_ids
            || prepared
                .inspections()
                .iter()
                .any(|episode| episode.joints != plan.joints)
        {
            return Err(ApplicationError::Conflict(
                "the authorized export plan no longer matches its prepared takes".to_owned(),
            ));
        }
        let artifact = tokio::task::spawn_blocking(move || {
            let temporary = tempfile::NamedTempFile::new().map_err(|error| {
                ApplicationError::Internal(format!(
                    "could not create a temporary export file: {error}"
                ))
            })?;
            let destination = temporary.reopen().map_err(|error| {
                ApplicationError::Internal(format!(
                    "could not open the temporary export file: {error}"
                ))
            })?;
            export_prepared_lerobot_to_file(
                prepared,
                destination,
                plan.fps,
                plan.include_proprioception,
            )
            .map_err(episode_application_error)?;
            let length = temporary
                .as_file()
                .metadata()
                .map_err(|error| {
                    ApplicationError::Internal(format!(
                        "could not inspect the temporary export file: {error}"
                    ))
                })?
                .len();
            Ok(DatasetDownload { temporary, length })
        })
        .await
        .map_err(|error| ApplicationError::Internal(error.to_string()))??;
        let sink = self
            .export_artifact
            .as_ref()
            .ok_or_else(|| ApplicationError::Internal("missing dataset export sink".to_owned()))?;
        *sink
            .lock()
            .map_err(|_| ApplicationError::Internal("dataset export sink poisoned".to_owned()))? =
            Some(artifact);
        Ok(())
    }

    async fn stage_import(
        &self,
        dataset_id: String,
    ) -> Result<StagedDatasetImport<Self::StagedImport>, ApplicationError> {
        let source = self
            .import_source
            .as_ref()
            .ok_or_else(|| ApplicationError::Internal("missing dataset import source".to_owned()))?
            .lock()
            .map_err(|_| {
                ApplicationError::Internal("dataset import source lock failed".to_owned())
            })?
            .take()
            .ok_or_else(|| {
                ApplicationError::Conflict(
                    "the dataset import source was already consumed".to_owned(),
                )
            })?;
        let episodes = self.state.episodes.clone();
        let staged = tokio::task::spawn_blocking(move || {
            let (_temporary, source) = source;
            stage_lerobot_archive_file_into_dataset(&episodes, source, &dataset_id)
        })
        .await
        .map_err(|error| ApplicationError::Internal(error.to_string()))?
        .map_err(episode_application_error)?;
        let summary = staged.summary();
        Ok(StagedDatasetImport {
            summary: DatasetImportSummary {
                episodes: summary.episodes,
                frames: summary.frames,
                first_episode: summary.first_episode.clone(),
            },
            staged,
        })
    }

    async fn commit_import(&self, staged: Self::StagedImport) -> Result<(), ApplicationError> {
        let episodes = self.state.episodes.clone();
        tokio::task::spawn_blocking(move || commit_staged_lerobot_import(&episodes, staged))
            .await
            .map_err(|error| ApplicationError::Internal(error.to_string()))?
            .map(|_| ())
            .map_err(episode_application_error)
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
                unfinished: summary.file.ends_with(".jsonl.part"),
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

async fn list_episodes(State(state): State<AppState>) -> Json<Vec<EpisodeSummary>> {
    Json(
        episode_queries::list(&EpisodeHttpAdapter::catalog(&state))
            .into_iter()
            .map(contract_episode)
            .collect(),
    )
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
        file: String::new(),
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
    let video_settings = state.data_settings.get().video;
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
            state.episodes.directory(),
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
/// flush barrier and the partial file has been indexed. That makes timeline
/// reads deterministic while the operator scrubs, without blocking the motor
/// thread on filesystem I/O. Hardware owns restoring the exact control mode on
/// resume; the route brackets it with video capture so paused camera frames are
/// omitted as well.
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
/// disk I/O after a newer scrub; the one remaining pose enters the controller
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
    // Looking up the episode first validates the path component and prevents an
    // orphaned sidecar from becoming an independent file-serving namespace.
    episode_queries::authorize_asset(&EpisodeHttpAdapter::catalog(&state), id.clone())?;
    let assets = load_assets(state.episodes.directory(), &id)
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?;
    let asset = assets
        .into_iter()
        .filter(|asset| asset.stream_id == stream)
        .filter(|asset| {
            query
                .segment
                .is_none_or(|segment| asset.segment_index == segment)
        })
        .min_by_key(|asset| asset.segment_index)
        .ok_or_else(|| {
            Failure(
                StatusCode::NOT_FOUND,
                format!(
                    "take {id:?} has no recorded video stream {stream:?}{}",
                    query
                        .segment
                        .map(|segment| format!(" segment {segment}"))
                        .unwrap_or_default()
                ),
            )
        })?;
    let mut file = tokio::fs::File::open(&asset.path)
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let length = file
        .metadata()
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .len();
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
    if range.start > 0 {
        file.seek(std::io::SeekFrom::Start(range.start))
            .await
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    }
    let content_length = if length == 0 {
        0
    } else {
        range.end - range.start + 1
    };
    let stream = async_stream::stream! {
        let mut remaining = content_length;
        let mut buffer = vec![0_u8; 64 * 1024];
        while remaining > 0 {
            let wanted = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
            let read = match file.read(&mut buffer[..wanted]).await {
                Ok(read) => read,
                Err(error) => {
                    yield Err::<Bytes, std::io::Error>(error);
                    break;
                }
            };
            if read == 0 {
                yield Err::<Bytes, std::io::Error>(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "recorded video ended before its advertised length",
                ));
                break;
            }
            remaining -= read as u64;
            yield Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(&buffer[..read]));
        }
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
        .body(Body::from_stream(stream))
        .expect("valid local video response"))
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

async fn export_episodes(
    State(state): State<AppState>,
    Json(request): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, Failure> {
    let destination = resolve_export_directory(state.episodes.directory(), &request.directory)?;
    let directory = destination.display().to_string();
    let result = episode_catalog::export(
        &EpisodeHttpAdapter::export(&state, destination),
        ExportEpisodesCommand,
    )
    .await?;
    Ok(Json(ExportResponse {
        directory,
        episodes: result.episodes,
        frames: result.frames,
        skipped: result.skipped,
    }))
}

/// Build a current LeRobot v3 dataset and let the browser download it as one file.
#[derive(Debug, Deserialize)]
struct DatasetQuery {
    dataset_id: Option<String>,
}

async fn download_episodes(
    State(state): State<AppState>,
    Query(query): Query<DatasetQuery>,
) -> Result<Response, Failure> {
    let adapter = DatasetHttpAdapter::export(&state);
    let summary = dataset_application::export(
        &adapter,
        ExportDatasetCommand {
            dataset_id: query.dataset_id,
        },
    )
    .await?;
    let DatasetDownload { temporary, length } = adapter.take_export_artifact()?;
    let file = tokio::fs::File::open(temporary.path())
        .await
        .map_err(|error| {
            Failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not stream the temporary export file: {error}"),
            )
        })?;
    let stream = async_stream::stream! {
        // The named file removes itself only after the response body finishes
        // or is dropped by a disconnected browser.
        let _temporary = temporary;
        let mut file = file;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            match file.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => yield Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(&buffer[..read])),
                Err(error) => {
                    yield Err::<Bytes, std::io::Error>(error);
                    break;
                }
            }
        }
    };
    let mut response = Body::from_stream(stream).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/zip"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_static(
            "attachment; filename=\"spiral-robotics-lerobot-v3-dataset.zip\"",
        ),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_str(&length.to_string())
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-spiral-robotics-episodes"),
        header::HeaderValue::from_str(&summary.episodes.to_string())
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-spiral-robotics-frames"),
        header::HeaderValue::from_str(&summary.frames.to_string())
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-spiral-robotics-skipped"),
        header::HeaderValue::from_str(&summary.skipped.to_string())
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    // How many takes cost the whole dataset its velocity and effort columns.
    // The dataset card says this too, because that is what travels with the
    // file; the header is for a client that wants it without opening the ZIP.
    response.headers_mut().insert(
        header::HeaderName::from_static("x-spiral-robotics-without-proprioception"),
        header::HeaderValue::from_str(&summary.without_proprioception.to_string())
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    Ok(response)
}

/// Convert a browser-uploaded LeRobot ZIP into replayable local episodes.
async fn import_episodes(
    State(state): State<AppState>,
    Query(query): Query<DatasetQuery>,
    body: Body,
) -> Result<Json<ImportResponse>, Failure> {
    let temporary = tempfile::NamedTempFile::new().map_err(|error| {
        Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not create a temporary import file: {error}"),
        )
    })?;
    let output = temporary.reopen().map_err(|error| {
        Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not open the temporary import file: {error}"),
        )
    })?;
    let mut output = tokio::fs::File::from_std(output);
    let mut stream = body.into_data_stream();
    let mut written = 0_usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            Failure::bad_request(format!("could not read the uploaded ZIP: {error}"))
        })?;
        written = written.checked_add(chunk.len()).ok_or_else(|| {
            Failure(
                StatusCode::PAYLOAD_TOO_LARGE,
                "uploaded ZIP is too large".to_owned(),
            )
        })?;
        if written > MAX_ARCHIVE_COMPRESSED_BYTES {
            return Err(Failure(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "the uploaded ZIP exceeds the supported {} MiB limit",
                    MAX_ARCHIVE_COMPRESSED_BYTES / (1024 * 1024)
                ),
            ));
        }
        output.write_all(&chunk).await.map_err(|error| {
            Failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not spool the uploaded ZIP: {error}"),
            )
        })?;
    }
    output.flush().await.map_err(|error| {
        Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not finish the uploaded ZIP: {error}"),
        )
    })?;
    drop(output);
    if written == 0 {
        return Err(Failure::bad_request(
            "choose a LeRobot dataset ZIP to import",
        ));
    }
    let source = temporary.reopen().map_err(|error| {
        Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not reopen the uploaded ZIP: {error}"),
        )
    })?;
    let summary = dataset_application::import(
        &DatasetHttpAdapter::import(&state, temporary, source),
        ImportDatasetCommand {
            dataset_id: query.dataset_id.unwrap_or_else(|| "default".to_owned()),
        },
    )
    .await?;
    Ok(Json(ImportResponse {
        episodes: summary.episodes,
        frames: summary.frames,
        first_episode: summary.first_episode,
    }))
}

/// Resolve an export destination, refusing anything that escapes the workspace.
///
/// A relative path lands beside the episode directory, which is where a dataset
/// belongs. An absolute path is the operator saying exactly where they want it,
/// and is taken at its word; what is refused is the shape that looks relative
/// and is not — a `..` that walks out of the directory it appears to be under.
pub(super) fn resolve_export_directory(
    episode_directory: &std::path::Path,
    requested: &str,
) -> Result<std::path::PathBuf, Failure> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err(Failure::bad_request("name a directory to export into"));
    }
    let path = std::path::Path::new(requested);
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(Failure::bad_request(
            "an export directory cannot contain '..'; give a name or an absolute path",
        ));
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let parent = episode_directory
        .parent()
        .unwrap_or(episode_directory)
        .to_path_buf();
    Ok(parent.join(path))
}
