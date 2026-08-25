//! The studio's HTTP surface.
//!
//! Handlers translate delivery models into framework-neutral capability
//! commands and present their results. Capability use cases own admission and
//! orchestration; core entities and the serialized control thread provide the
//! final invariant and hardware-safety backstops.

mod data;
mod devices;
mod recordings;
mod training_rollout;
mod workflow;

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, body::Body};
use bytes::{BufMut, Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use spiral_robotics_api::recording::{
    EpisodeArmOptionalChannels, EpisodeArmScalarSamples, EpisodeAssessment, EpisodeCatalogResponse,
    EpisodeIdentity, EpisodeInspectionArm, EpisodeInspectionSnapshot, EpisodeOptionalFrameMatrix,
    EpisodeSummary, EpisodeTimeline, EpisodeVideoAssetMetadata, EpisodeVideoFormat,
    EpisodeVideoIdentity, EpisodeVideoTimeline, JudgeEpisodeRequest, SetRecordingPausedRequest,
    SetReplayPausedRequest, StartRecordingRequest, StartReplayRequest, StartSpiralReplayRequest,
    StopRecordingRequest,
};
use spiral_robotics_api::relations::{SpiralQueryRequest, SpiralQueryResponse};
use spiral_robotics_api::robots::{
    CalibrationReapplyResponse, CameraStatus, CameraStreamFormat, CameraStreamState,
    ConnectionStatus, CreateRobotRequest, GoalRequest, IdentifyPortRequest, IdentifyPortResponse,
    ModeSelection, PortSettings, RobotSummary, SerialPortSummary, UpdateRobotRequest,
};
use spiral_robotics_api::rollout::{RolloutSnapshot, StartRolloutRequest};
use spiral_robotics_api::settings::{
    ConnectionTestResponse, DataSettings, SpiralSettings as ContractSpiralSettings,
    UpdateDataSettingsRequest, VideoSettings,
};
use spiral_robotics_api::status::{
    ApiError, BackendReadiness, BackendReadinessState, StudioSnapshot,
};
use spiral_robotics_api::training::{StartTrainingRequest, TrainingRun, TrainingSnapshot};
use spiral_robotics_core::calibration::{Calibration, MotorSpec};
use spiral_robotics_hardware::serial_ports::{available_ports, port_description, port_kind};

use crate::data::catalog::{
    self as episode_catalog, DeleteEpisodeCommand, EpisodeActivity, EpisodeCatalogPort,
    EpisodeMutationResult, JudgeEpisodeCommand,
};
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease};
use crate::data::queries::{
    EpisodeArmRecord, EpisodeCaptureSummary, EpisodeDisposition, EpisodeInspectionArmRecord,
    EpisodeInspectionRecord, EpisodeQueries, EpisodeQueryPort, EpisodeRecord,
    EpisodeRecordIdentity, EpisodeTimelineRecord, EpisodeTraceQuery, OptionalFrameMatrixRecord,
    RecordingGapRecord, TimelineArmSampleRecord, TimelineMetadataRecord, TimelineSampleRecord,
    TimelineTraceCollection, TimelineTraceLaneRecord, TimelineTracePointRecord,
    TimelineTraceSeriesRecord, VideoAssetFormat, VideoAssetIdentity, VideoAssetRecord,
    VideoAssetTimeline,
};
use crate::data::seek::{
    self as episode_seek, ActiveRecording, HistoricalSample, HistoricalSeekState, PhysicalSeekPlan,
    PhysicalSeekPort, SampledArm, SeekActivity, SeekControlMode, SeekEpisodeCommand, SeekTarget,
    SeekableEpisode,
};
use crate::data::{
    ConnectionCheck, DataCatalogPort, DataSettingsSnapshot, QueryColumn as CapabilityQueryColumn,
    QueryCommand, QueryResult as CapabilityQueryResult, QueryValue,
    ReplayTrajectory as SpiralReplayTrajectory, SpiralDestinationSettings,
    StartSpiralReplayCommand, UpdateDataSettingsCommand, VideoAssetQuery,
};
use crate::recording::{
    self, RecordingContext, RecordingPort, RecordingResult, ReplayEpisode, ReplayPlan, ReplayPort,
    ReplayTrajectory, SetRecordingPausedCommand, SetReplayPausedCommand, StartRecordingCommand,
    StartReplayCommand, StopRecordingCommand, TeleoperationPair,
};
use crate::robotics::cameras::{CameraManager, Frame};
use crate::robotics::video::{
    CameraCaptureSpec, VideoAsset, VideoAssetMetadata, VideoCapture, VideoCaptureRequest,
    VideoEncoderSettings,
};
use crate::robots::calibration::{
    self, CalibrationHistoryActivity, CalibrationHistoryPort, CalibrationUpdate,
    ReappliedPositions, RobotCalibrationStore,
};
use crate::robots::control::{self as robot_control, RobotControlCommand};
use crate::robots::discovery::{
    CameraDiscoveryFormat, CameraDiscoveryPort, CameraDiscoveryStatus, DiscoveredCamera,
    IdentifySerialPortCommand, SerialPortDescriptor, SerialPortInventory, SerialPortQueryPort,
};
use crate::robots::mode::{self as mode_control, OperatorMode};
use crate::robots::service::{
    self as robots, AutoConnectOutcome, AutoConnectStatus, CameraAssignment, CreateRobotCommand,
    ExportedRobotDocument, NewRobot, RobotConnectionPort, RobotConnectionSpec, RobotStore,
    RobotUpdate, SavedRobot, SerialPortConfiguration, UpdateRobotCommand,
};
use crate::robots::workspace::{RobotRecord, RobotWorkspace, WorkspaceError};
use crate::rollout::manager::RolloutManager;
use crate::rollout::service::{
    self as rollout_application, RolloutMetrics as CapabilityRolloutMetrics,
    RolloutPhase as CapabilityRolloutPhase, RolloutRequest, RolloutSessionPort,
    RolloutSnapshot as CapabilityRolloutSnapshot,
};
use crate::training::manager::TrainingManager;
use crate::training::service::{
    self as training_application, DeleteTrainingRunCommand, StartTrainingCommand,
    TrainingCheckpointStatus, TrainingPort, TrainingRun as CapabilityTrainingRun,
    TrainingRunActivity, TrainingRunStatus, TrainingSnapshot as CapabilityTrainingSnapshot,
};
use crate::workflow::agent::WorkflowAgentManager;
use crate::workflow::manager::WorkflowManager;
use spiral_robotics_core::OperationError;
use spiral_robotics_hardware::{HardwareHandle, HardwareState, RobotSpec};
use spiral_robotics_storage::BackendStateStore;
use spiral_robotics_storage::StorageManager;
use spiral_robotics_storage::StudioPreferencesStore;
use spiral_robotics_storage::episodes::{
    ArmCondition, EpisodeError, EpisodeInspection, EpisodeRecordingRequest, EpisodeStore,
    EpisodeWriter, ReplayRequest, TimelineMetadata, TimelineSample, TimelineTraces, build_arms,
};
use spiral_robotics_storage::video::{
    VideoAssetKey, VideoByteRange, VideoSegment, VideoSize, VideoTiming,
};

/// Boundary marker separating parts of an MJPEG response.
const MJPEG_BOUNDARY: &str = "spiral-robotics-frame";

/// How often live telemetry is pushed to the browser.
const TELEMETRY_INTERVAL: Duration = Duration::from_millis(50);

/// How often the setup-level snapshot is rebuilt and diffed.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(2);

/// A short debounce keeps a just-started recording from being mistaken for an
/// automatic stop before the next telemetry snapshot is published.
const VIDEO_RECORDING_MONITOR_INTERVAL: Duration = Duration::from_millis(200);
const VIDEO_RECORDING_STOP_TICKS: u8 = 5;

/// Everything a request handler can reach.
#[derive(Clone)]
pub(crate) struct AppState {
    catalog: AppCatalogResources,
    robotics: AppRoboticsResources,
    runtime: AppRuntimeResources,
}

#[derive(Clone)]
pub(crate) struct AppCatalogResources {
    workspace: Arc<RobotWorkspace>,
    episodes: Arc<EpisodeStore>,
    storage: Arc<StorageManager>,
    catalog_lease: Arc<CatalogMutationLease>,
}

impl AppCatalogResources {
    pub(crate) fn new(
        workspace: Arc<RobotWorkspace>,
        episodes: Arc<EpisodeStore>,
        storage: Arc<StorageManager>,
    ) -> Self {
        Self {
            workspace,
            episodes,
            storage,
            catalog_lease: Arc::new(CatalogMutationLease::default()),
        }
    }

    pub(crate) fn with_lease(mut self, catalog_lease: Arc<CatalogMutationLease>) -> Self {
        self.catalog_lease = catalog_lease;
        self
    }
}

#[derive(Clone)]
pub(crate) struct AppRoboticsResources {
    hardware: Arc<HardwareHandle>,
    cameras: Arc<CameraManager>,
    video: AppVideoResources,
}

#[derive(Clone)]
pub(crate) struct AppVideoResources {
    recording_video: Arc<tokio::sync::Mutex<Option<VideoCapture>>>,
    video_encoder: VideoEncoderSettings,
}

impl AppVideoResources {
    pub(crate) fn new(
        recording_video: Arc<tokio::sync::Mutex<Option<VideoCapture>>>,
        video_encoder: VideoEncoderSettings,
    ) -> Self {
        Self {
            recording_video,
            video_encoder,
        }
    }
}

impl AppRoboticsResources {
    pub(crate) fn new(
        hardware: Arc<HardwareHandle>,
        cameras: Arc<CameraManager>,
        video: AppVideoResources,
    ) -> Self {
        Self {
            hardware,
            cameras,
            video,
        }
    }
}

#[derive(Clone)]
pub(crate) struct AppActivityManagers {
    training: Arc<TrainingManager>,
    rollout: Arc<RolloutManager>,
    workflow: AppWorkflowManagers,
}

#[derive(Clone)]
pub(crate) struct AppWorkflowManagers {
    workflow: Arc<WorkflowManager>,
    workflow_agent: Arc<WorkflowAgentManager>,
}

impl AppWorkflowManagers {
    pub(crate) fn new(
        workflow: Arc<WorkflowManager>,
        workflow_agent: Arc<WorkflowAgentManager>,
    ) -> Self {
        Self {
            workflow,
            workflow_agent,
        }
    }
}

impl AppActivityManagers {
    pub(crate) fn new(
        training: Arc<TrainingManager>,
        rollout: Arc<RolloutManager>,
        workflow: AppWorkflowManagers,
    ) -> Self {
        Self {
            training,
            rollout,
            workflow,
        }
    }
}

#[derive(Clone)]
pub(crate) struct AppPlatformResources {
    backend_state: Arc<BackendStateStore>,
    preferences: Arc<StudioPreferencesStore>,
    bootstrap: BootstrapState,
}

impl AppPlatformResources {
    pub(crate) fn new(
        backend_state: Arc<BackendStateStore>,
        preferences: Arc<StudioPreferencesStore>,
        bootstrap: BootstrapState,
    ) -> Self {
        Self {
            backend_state,
            preferences,
            bootstrap,
        }
    }
}

#[derive(Clone)]
pub(crate) struct AppRuntimeResources {
    activities: AppActivityManagers,
    platform: AppPlatformResources,
}

impl AppRuntimeResources {
    pub(crate) fn new(activities: AppActivityManagers, platform: AppPlatformResources) -> Self {
        Self {
            activities,
            platform,
        }
    }
}

impl AppState {
    pub(crate) fn new(
        catalog: AppCatalogResources,
        robotics: AppRoboticsResources,
        runtime: AppRuntimeResources,
    ) -> Self {
        Self {
            catalog,
            robotics,
            runtime,
        }
    }

    pub(crate) fn workspace(&self) -> &Arc<RobotWorkspace> {
        &self.catalog.workspace
    }
    pub(crate) fn episodes(&self) -> &Arc<EpisodeStore> {
        &self.catalog.episodes
    }
    pub(crate) fn storage(&self) -> &Arc<StorageManager> {
        &self.catalog.storage
    }
    pub(crate) fn catalog_lease(&self) -> &Arc<CatalogMutationLease> {
        &self.catalog.catalog_lease
    }
    pub(crate) fn hardware(&self) -> &Arc<HardwareHandle> {
        &self.robotics.hardware
    }
    pub(crate) fn cameras(&self) -> &Arc<CameraManager> {
        &self.robotics.cameras
    }
    pub(crate) fn recording_video(&self) -> &Arc<tokio::sync::Mutex<Option<VideoCapture>>> {
        &self.robotics.video.recording_video
    }
    pub(crate) fn video_encoder(&self) -> &VideoEncoderSettings {
        &self.robotics.video.video_encoder
    }
    pub(crate) fn training(&self) -> &Arc<TrainingManager> {
        &self.runtime.activities.training
    }
    pub(crate) fn workflow(&self) -> &Arc<WorkflowManager> {
        &self.runtime.activities.workflow.workflow
    }
    pub(crate) fn workflow_agent(&self) -> &Arc<WorkflowAgentManager> {
        &self.runtime.activities.workflow.workflow_agent
    }
    pub(crate) fn rollout(&self) -> &Arc<RolloutManager> {
        &self.runtime.activities.rollout
    }
    pub(crate) fn backend_state(&self) -> &Arc<BackendStateStore> {
        &self.runtime.platform.backend_state
    }
    pub(crate) fn preferences(&self) -> &Arc<StudioPreferencesStore> {
        &self.runtime.platform.preferences
    }
    pub(crate) fn bootstrap(&self) -> &BootstrapState {
        &self.runtime.platform.bootstrap
    }
}

/// Shared backend lifecycle published to the browser and consulted by
/// catalog-dependent request admission.
#[derive(Clone)]
pub(crate) struct BootstrapState {
    current: tokio::sync::watch::Sender<BackendReadiness>,
}

impl BootstrapState {
    pub(crate) fn new() -> Self {
        let (current, receiver) = tokio::sync::watch::channel(BackendReadiness::default());
        drop(receiver);
        Self { current }
    }

    pub(crate) fn current(&self) -> BackendReadiness {
        self.current.borrow().clone()
    }

    pub(crate) fn bootstrapping(&self, detail: impl Into<String>) {
        self.current.send_replace(BackendReadiness::new(
            BackendReadinessState::Bootstrapping,
            false,
            detail,
        ));
    }

    pub(crate) fn ready(&self) {
        self.current.send_replace(BackendReadiness::new(
            BackendReadinessState::Ready,
            true,
            "Studio services are ready",
        ));
    }

    pub(crate) fn degraded(&self, catalog_available: bool, detail: impl Into<String>) {
        self.current.send_replace(BackendReadiness::new(
            BackendReadinessState::Degraded,
            catalog_available,
            detail,
        ));
    }

    fn catalog_unavailable(&self) -> Option<String> {
        let current = self.current();
        (!current.catalog_available()).then(|| current.detail().to_owned())
    }
}

pub(crate) async fn monitor_video_recording(state: AppState) {
    let mut ticker = tokio::time::interval(VIDEO_RECORDING_MONITOR_INTERVAL);
    let mut missing_episode = String::new();
    let mut missing_ticks = 0_u8;
    loop {
        ticker.tick().await;
        let capture_episode = state
            .recording_video()
            .lock()
            .await
            .as_ref()
            .map(|capture| capture.episode_id().to_owned());
        let watched_episode = state
            .catalog_lease()
            .recording_episode()
            .or(capture_episode);
        let Some(capture_episode) = watched_episode else {
            missing_episode.clear();
            missing_ticks = 0;
            continue;
        };
        let hardware = state.hardware().state();
        let hardware_episode = hardware
            .telemetry()
            .recording()
            .as_ref()
            .map(|recording| recording.episode().to_owned());
        if hardware_episode.as_deref() == Some(&capture_episode) {
            missing_episode.clear();
            missing_ticks = 0;
            continue;
        }
        if missing_episode == capture_episode {
            missing_ticks = missing_ticks.saturating_add(1);
        } else {
            missing_episode = capture_episode;
            missing_ticks = 1;
        }
        if missing_ticks < VIDEO_RECORDING_STOP_TICKS {
            continue;
        }
        finish_stale_video_capture(&state).await;
        missing_episode.clear();
        missing_ticks = 0;
    }
}

pub(crate) async fn auto_connect_saved_robots(state: AppState) {
    devices::auto_connect_saved_robots(state).await;
}

/// Close and commit any capture whose corresponding control-loop recording has
/// already ended. Also used during graceful backend shutdown.
pub(crate) async fn finish_stale_video_capture(state: &AppState) {
    // Claim the recording exactly once, but retain catalog exclusion until the
    // episode and finalized media have both been committed. This exists
    // independently of VideoCapture because camera-less recordings still
    // allocate catalog identities.
    let recording = state.catalog_lease().claim_recording();
    let capture = state.recording_video().lock().await.take();
    if recording.is_none() && capture.is_none() {
        return;
    }
    let episode_id = recording
        .as_ref()
        .map(|recording| recording.episode().to_owned())
        .or_else(|| {
            capture
                .as_ref()
                .map(|capture| capture.episode_id().to_owned())
        });
    let videos = match capture {
        Some(capture) => finish_video_capture(state, capture).await,
        None => Vec::new(),
    };
    if let Some(episode_id) = episode_id {
        let episodes = state.episodes().clone();
        let storage = state.storage().clone();
        let committed = tokio::task::spawn_blocking(move || {
            let episode = episodes
                .completed(&episode_id)
                .map_err(|error| error.to_string())?;
            storage.commit_episode_blocking(&episode, &videos)
        })
        .await;
        if let Err(error) = committed
            .map_err(|error| error.to_string())
            .and_then(|result| result)
        {
            tracing::error!("could not commit automatically stopped episode media: {error}");
            state.cameras().note_issue(format!(
                "episode media could not be committed to SpiralDB: {error}"
            ));
        }
    }
    if let Some(recording) = recording {
        let released = state.catalog_lease().release(recording.token());
        debug_assert!(
            released,
            "claimed recording lease must release exactly once"
        );
    }
}

pub(super) async fn finish_video_capture(
    state: &AppState,
    capture: VideoCapture,
) -> Vec<VideoAsset> {
    let (videos, failures) = capture.finish().await;
    for failure in failures {
        state
            .cameras()
            .note_issue(format!("episode video was not saved: {failure}"));
    }
    videos
}

/// A failed request, rendered as `{"error": "..."}`.
#[derive(Debug)]
pub(crate) struct Failure(StatusCode, String);

impl Failure {
    fn bad_request(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }

    fn import(message: impl Into<String>) -> Self {
        let message = message.into();
        let status = if spiral_robotics_storage::is_unavailable_error(&message) {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::BAD_REQUEST
        };
        Self(status, message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self(StatusCode::CONFLICT, message.into())
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self(StatusCode::SERVICE_UNAVAILABLE, message.into())
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let retry = self.0 == StatusCode::SERVICE_UNAVAILABLE;
        let mut response = (self.0, Json(ApiError::new(self.1))).into_response();
        if retry {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, "1".parse().expect("valid retry delay"));
        }
        response
    }
}

impl From<EpisodeError> for Failure {
    fn from(error: EpisodeError) -> Self {
        let status = match &error {
            EpisodeError::InvalidId(_) | EpisodeError::Invalid(_) => StatusCode::BAD_REQUEST,
            EpisodeError::NotFound(_) => StatusCode::NOT_FOUND,
        };
        Self(status, error.to_string())
    }
}

impl From<WorkspaceError> for Failure {
    fn from(error: WorkspaceError) -> Self {
        let status = match &error {
            WorkspaceError::InvalidId(_) | WorkspaceError::Invalid(_) => StatusCode::BAD_REQUEST,
            WorkspaceError::NotFound(_) => StatusCode::NOT_FOUND,
            WorkspaceError::AlreadyExists(_) => StatusCode::CONFLICT,
            WorkspaceError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(status, error.to_string())
    }
}

/// Control-thread refusals are the operator asking for something not currently
/// allowed, which is a client error rather than a server fault.
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self(StatusCode::CONFLICT, message)
    }
}

impl From<OperationError> for Failure {
    fn from(error: OperationError) -> Self {
        let status = match error {
            OperationError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            OperationError::NotFound(_) => StatusCode::NOT_FOUND,
            OperationError::Conflict(_) => StatusCode::CONFLICT,
            OperationError::External(_) => StatusCode::BAD_GATEWAY,
            OperationError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(status, error.to_string())
    }
}

/// Build the studio's API routes.
pub(crate) fn api_router(state: AppState) -> Router {
    let catalog_routes = Router::new()
        .merge(data::catalog_router())
        .merge(devices::catalog_router())
        .merge(recordings::router())
        .merge(training_rollout::router())
        .merge(workflow::router())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_catalog_ready,
        ));

    Router::new()
        .merge(data::bootstrap_router())
        .merge(devices::bootstrap_router())
        .merge(catalog_routes)
        .route("/state", get(get_state))
        .route("/events", get(events))
        .with_state(state)
}

async fn require_catalog_ready(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, Failure> {
    match state.bootstrap().catalog_unavailable() {
        Some(detail) => Err(Failure::unavailable(format!(
            "Studio catalog is not ready: {detail}"
        ))),
        None => Ok(next.run(request).await),
    }
}

/// Assemble the current setup-level view of the studio.
impl AppState {
    pub(crate) fn snapshot(&self) -> StudioSnapshot {
        let state = self;
        let hardware = state.hardware().state();
        let readiness = state.bootstrap().current();
        let robots = if readiness.catalog_available() {
            let robot_adapter = devices::RobotHttpAdapter::prefetched(state);
            robots::list(&robot_adapter)
                .into_iter()
                .filter_map(|saved| {
                    let connection = hardware
                        .connections()
                        .get(saved.id())
                        .cloned()
                        .unwrap_or_default();
                    let mut summary = robot_adapter.summary(saved.id(), connection).ok()?;
                    // Limits come from the motors, not the saved configuration, so they
                    // are folded in here after loading the SpiralDB row.
                    if let Some(limits) = hardware.limits().get(saved.id()) {
                        let motors = summary
                            .motors()
                            .iter()
                            .cloned()
                            .map(|mut motor| {
                                if let Some(limits) = limits.get(motor.name()) {
                                    motor.set_limits(*limits);
                                }
                                motor
                            })
                            .collect();
                        summary = summary.with_motors(motors);
                    }
                    Some(summary)
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut issues = if readiness.catalog_available() {
            state.workspace().issues()
        } else {
            Vec::new()
        };
        issues.extend(state.cameras().issues());
        // A take that stopped itself has to be visible after the counters that were
        // describing it have gone.
        if let Some(issue) = hardware.recording_issue() {
            issues.push(issue.to_owned());
        }

        let cameras = devices::CameraDiscoveryAdapter::new(state.cameras().clone())
            .cameras()
            .into_iter()
            .map(|camera| {
                CameraStatus::new(
                    camera.id(),
                    camera.name(),
                    CameraStreamFormat::new(camera.width(), camera.height(), camera.fps()),
                )
                .with_state(CameraStreamState::new(
                    camera.streaming(),
                    camera.frames(),
                    camera.error().map(str::to_owned),
                ))
            })
            .collect();
        StudioSnapshot::new(readiness, "SpiralDB", hardware.mode().clone())
            .with_robots(robots)
            .with_cameras(cameras)
            .with_issues(issues)
    }
}

async fn get_state(State(state): State<AppState>) -> Json<StudioSnapshot> {
    Json(state.snapshot())
}

/// Push setup changes and live telemetry to the browser.
///
/// The snapshot is only re-sent when it actually differs, so an idle studio
/// costs one small telemetry frame per tick instead of a full state dump.
async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        let mut snapshot_timer = tokio::time::interval(SNAPSHOT_INTERVAL);
        snapshot_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut telemetry_timer = tokio::time::interval(TELEMETRY_INTERVAL);
        let (initial_training, mut training) = state.training().subscription();
        let (initial_workflow, mut workflow) = state.workflow().subscription();
        let mut last_workflow = initial_workflow.clone();
        let (initial_agent, mut workflow_agent) = state.workflow_agent().subscription();
        let mut last_agent_cursor = (
            initial_agent.scope_id().to_owned(),
            initial_agent.revision(),
        );
        let initial_rollout = RolloutManager::snapshot(state.rollout().as_ref());
        let mut rollout = state.rollout().subscribe();
        let mut last_snapshot: Option<StudioSnapshot> = None;
        let mut last_sequence = 0_u64;

        if let Ok(event) = Event::default()
            .event("training_snapshot")
            .json_data(&initial_training)
        {
            yield Ok(event);
        }
        if let Ok(event) = Event::default()
            .event("rollout_state")
            .json_data(&initial_rollout)
        {
            yield Ok(event);
        }
        if let Ok(event) = Event::default()
            .event("workflow_snapshot")
            .json_data(&initial_workflow)
        {
            yield Ok(event);
        }
        if let Ok(event) = Event::default()
            .event("workflow_agent")
            .json_data(&initial_agent)
        {
            yield Ok(event);
        }

        loop {
            tokio::select! {
                _ = snapshot_timer.tick() => {
                    let snapshot = state.snapshot();
                    if last_snapshot.as_ref() == Some(&snapshot) {
                        continue;
                    }
                    last_snapshot = Some(snapshot.clone());
                    if let Ok(event) = Event::default().event("state").json_data(&snapshot) {
                        yield Ok(event);
                    }
                }
                _ = telemetry_timer.tick() => {
                    let hardware = state.hardware().state();
                    let telemetry = hardware.telemetry();
                    if telemetry.sequence() == last_sequence {
                        continue;
                    }
                    last_sequence = telemetry.sequence();
                    if let Ok(event) = Event::default().event("telemetry").json_data(telemetry) {
                        yield Ok(event);
                    }
                }
                update = training.recv() => {
                    match update {
                        Ok(update) => {
                            if let Ok(event) = Event::default()
                                .event("training_update")
                                .json_data(&update)
                            {
                                yield Ok(event);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let (snapshot, replacement) = state.training().subscription();
                            training = replacement;
                            if let Ok(event) = Event::default()
                                .event("training_snapshot")
                                .json_data(&snapshot)
                            {
                                yield Ok(event);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                update = workflow.recv() => {
                    match update {
                        Ok(_) => {
                            // Broadcast values are wakeups. A queued update
                            // from the project that was just replaced must not
                            // overwrite the manager's newer, lower-revision
                            // scoped state.
                            let snapshot = state.workflow().state();
                            if snapshot == last_workflow {
                                continue;
                            }
                            last_workflow = snapshot.clone();
                            let update = spiral_robotics_api::workflow::WorkflowUpdate::new(snapshot);
                            if let Ok(event) = Event::default()
                                .event("workflow_update")
                                .json_data(&update)
                            {
                                yield Ok(event);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let (snapshot, replacement) = state.workflow().subscription();
                            workflow = replacement;
                            if snapshot == last_workflow {
                                continue;
                            }
                            last_workflow = snapshot.clone();
                            if let Ok(event) = Event::default()
                                .event("workflow_snapshot")
                                .json_data(&snapshot)
                            {
                                yield Ok(event);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                update = workflow_agent.recv() => {
                    match update {
                        Ok(_) => {
                            // Broadcast values are wakeups. Snapshot current
                            // manager truth so a queued update from a prior
                            // conversation or project scope cannot regress SSE.
                            let snapshot = state.workflow_agent().state();
                            let cursor = (snapshot.scope_id().to_owned(), snapshot.revision());
                            if !replace_agent_cursor(&mut last_agent_cursor, &cursor) {
                                continue;
                            }
                            if let Ok(event) = Event::default()
                                .event("workflow_agent")
                                .json_data(&snapshot)
                            {
                                yield Ok(event);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let (snapshot, replacement) = state.workflow_agent().subscription();
                            workflow_agent = replacement;
                            let cursor = (snapshot.scope_id().to_owned(), snapshot.revision());
                            if !replace_agent_cursor(&mut last_agent_cursor, &cursor) {
                                continue;
                            }
                            if let Ok(event) = Event::default()
                                .event("workflow_agent")
                                .json_data(&snapshot)
                            {
                                yield Ok(event);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                changed = rollout.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let snapshot = rollout.borrow_and_update().clone();
                    if let Ok(event) = Event::default()
                        .event("rollout_state")
                        .json_data(&snapshot)
                    {
                        yield Ok(event);
                    }
                }
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn replace_agent_cursor(last: &mut (String, u64), next: &(String, u64)) -> bool {
    if last == next {
        return false;
    }
    *last = next.clone();
    true
}

#[cfg(test)]
mod tests;
