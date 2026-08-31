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
    DatasetCatalogResponse, EpisodeArmScalarSamples, EpisodeCatalogResponse, EpisodeInfo,
    EpisodeInspectionArm, EpisodeInspectionSnapshot, EpisodeListItem, EpisodeOptionalFrameMatrix,
    EpisodeVideoAssetMetadata, EpisodeVideoFormat, EpisodeVideoIdentity, EpisodeVideoTimeline,
    SetRecordingPausedRequest, SetReplayPausedRequest, StartRecordingRequest, StartReplayRequest,
    StopRecordingRequest,
};
use spiral_robotics_api::relations::{SpiralQueryRequest, SpiralQueryResponse};
use spiral_robotics_api::robots::{
    CalibrationReapplyResponse, CameraStatus, ConnectionStatus, CreateRobotRequest, GoalRequest,
    IdentifyPortRequest, IdentifyPortResponse, ModeSelection, PortSettings, RobotSummary,
    SerialPortSummary, UpdateRobotRequest,
};
use spiral_robotics_api::rollout::{RolloutSnapshot, StartRolloutRequest};
use spiral_robotics_api::settings::{
    ConnectionTestResponse, DataSettings, SpiralSettings as ContractSpiralSettings,
    UpdateDataSettingsRequest, VideoSettings,
};
use spiral_robotics_api::status::{
    ApiError, BackendReadiness, BackendReadinessState, StudioSnapshot,
};
use spiral_robotics_api::training::{StartTrainingRequest, TrainingRun};
use spiral_robotics_hardware::{Calibration, MotorSpec, SerialPortCatalog};

use crate::OperationError;
use crate::adapters::hardware::HardwareApiProjection as _;
#[cfg(test)]
use crate::data::SpiralDestinationSettings;
use crate::data::catalog::{
    DeleteEpisodeCommand, EpisodeActivity, EpisodeCatalogActions, EpisodeCatalogPort,
};
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease};
use crate::data::connection::SpiralConnectionController;
use crate::data::queries::{EpisodeQueries, EpisodeQueryPort};
use crate::data::seek::{
    ActiveRecording, PhysicalSeekActions, PhysicalSeekPlan, PhysicalSeekPort, RecordedSample,
    RecordedSeekState, SampledArm, SeekActivity, SeekControlMode, SeekEpisodeCommand, SeekTarget,
    SeekableEpisode,
};
use crate::data::{
    ConnectionCheck, DataCatalogPort, DataSettingsSnapshot, DataSettingsUpdate,
    QueryColumn as CapabilityQueryColumn, QueryCommand, QueryResult as CapabilityQueryResult,
    QueryValue, TestConnectionCommand, UpdateDataSettingsCommand,
};
use crate::recording::{
    RecordingActions, RecordingContext, RecordingPort, RecordingResult, ReplayActions,
    ReplayEpisode, ReplayPlan, ReplayPort, ReplayTrajectory, SetRecordingPausedCommand,
    SetReplayPausedCommand, StartRecordingCommand, StartReplayCommand, StopRecordingCommand,
    TeleoperationPair,
};
use crate::recording_session::RecordingSessionManager;
use crate::robotics::cameras::{CameraManager, Frame};
use crate::robotics::video::{
    CameraCaptureSpec, VideoAsset, VideoCapture, VideoCaptureRequest, VideoEncoderSettings,
};
use crate::robots::calibration::{
    CalibrationActions, CalibrationControlActions, CalibrationHistoryActions,
    CalibrationHistoryActivity, CalibrationHistoryPort, CalibrationUpdate, RobotCalibrationStore,
};
use crate::robots::control::{RobotControlActions, RobotControlCommand};
use crate::robots::discovery::{
    CameraDiscoveryPort, IdentifySerialPortCommand, SerialPortInventory, SerialPortQueryPort,
};
use crate::robots::mode::{ModeActions, OperatorMode};
use crate::robots::service::{
    AutoConnectOutcome, AutoConnectStatus, CameraAssignment, CreateRobotCommand, NewRobot,
    RobotActions, RobotCatalogActions, RobotConnectionActions, RobotConnectionPort,
    RobotConnectionSpec, RobotStore, RobotUpdate, SavedRobot, SerialPortConfiguration,
    UpdateRobotCommand,
};
use crate::robots::workspace::{RobotRecord, RobotWorkspace, WorkspaceError};
use crate::rollout::manager::RolloutManager;
use crate::rollout::service::{
    RolloutActions, RolloutPhase as CapabilityRolloutPhase, RolloutRequest, RolloutSessionPort,
    RolloutSnapshot as CapabilityRolloutSnapshot,
};
use crate::training::manager::TrainingManager;
use crate::training::service::{
    DeleteTrainingRunCommand, TrainingActions, TrainingCheckpointStatus, TrainingPort,
    TrainingRunActivity, TrainingRunStatus,
};
use crate::workflow::agent::WorkflowAgentManager;
use crate::workflow::manager::WorkflowManager;
use spiral_robotics_hardware::{HardwareHandle, HardwareState, RobotSpec};
use spiral_robotics_storage::StorageManager;
use spiral_robotics_storage::StudioPreferencesStore;
use spiral_robotics_storage::episodes::{
    ArmCondition, EpisodeError, EpisodeInspection, EpisodeRecordingRequest, EpisodeStore,
    EpisodeWriter, JointMotorLimits, RobotPlaybackRequest,
};
use spiral_robotics_storage::video::{VideoAssetKey, VideoByteRange};

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
    recording_session: Arc<RecordingSessionManager>,
    video_encoder: VideoEncoderSettings,
}

impl AppVideoResources {
    pub(crate) fn new(
        recording_session: Arc<RecordingSessionManager>,
        video_encoder: VideoEncoderSettings,
    ) -> Self {
        Self {
            recording_session,
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
    connection: Arc<SpiralConnectionController>,
    preferences: Arc<StudioPreferencesStore>,
    bootstrap: BootstrapState,
}

impl AppPlatformResources {
    pub(crate) fn new(
        connection: Arc<SpiralConnectionController>,
        preferences: Arc<StudioPreferencesStore>,
        bootstrap: BootstrapState,
    ) -> Self {
        Self {
            connection,
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
    pub(crate) fn recording_session(&self) -> &Arc<RecordingSessionManager> {
        &self.robotics.video.recording_session
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
    pub(crate) fn connection(&self) -> &Arc<SpiralConnectionController> {
        &self.runtime.platform.connection
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

    fn subscribe(&self) -> tokio::sync::watch::Receiver<BackendReadiness> {
        self.current.subscribe()
    }

    pub(crate) fn bootstrapping(&self, detail: impl Into<String>) {
        self.current.send_replace(BackendReadiness::new(
            BackendReadinessState::Bootstrapping,
            false,
            detail,
        ));
    }

    pub(crate) fn hydrating_services(&self, detail: impl Into<String>) {
        self.current.send_replace(
            BackendReadiness::new(BackendReadinessState::Bootstrapping, false, detail)
                .with_storage_available(),
        );
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

    fn storage_unavailable(&self) -> Option<String> {
        let current = self.current();
        (!current.storage_available()).then(|| current.detail().to_owned())
    }
}

impl AppState {
    pub(crate) async fn monitor_video_recording(self) {
        let mut ticker = tokio::time::interval(VIDEO_RECORDING_MONITOR_INTERVAL);
        let mut missing_episode = String::new();
        let mut missing_ticks = 0_u8;
        loop {
            ticker.tick().await;
            let Some(capture_episode) = self.recording_session().episode() else {
                missing_episode.clear();
                missing_ticks = 0;
                continue;
            };
            let hardware = self.hardware().state();
            let hardware_episode = hardware
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
            self.finish_stale_video_capture().await;
            missing_episode.clear();
            missing_ticks = 0;
        }
    }

    /// Close and commit any capture whose corresponding control-loop recording
    /// has already ended. Also used during graceful backend shutdown.
    pub(crate) async fn finish_stale_video_capture(&self) {
        // Claim the recording exactly once, but retain catalog exclusion until the
        // episode and finalized media have both been committed. This exists
        // independently of VideoCapture because camera-less recordings still
        // allocate catalog identities.
        let token = match self.recording_session().claim_finalization() {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!("could not claim stale recording finalization: {error}");
                return;
            }
        };
        let capture = self.recording_session().take_media().await;
        if token.is_none() && capture.is_none() {
            return;
        }
        let episode_id = self.recording_session().episode().or_else(|| {
            capture
                .as_ref()
                .map(|capture| capture.episode_id().to_owned())
        });
        let videos = match capture {
            Some(capture) => self.finish_video_capture(capture).await,
            None => Vec::new(),
        };
        if let Some(episode_id) = episode_id {
            let committed = async {
                let episode = self
                    .episodes()
                    .completed_episode(&episode_id)
                    .map_err(|error| error.to_string())?;
                self.storage().commit_episode(&episode, &videos).await?;
                self.episodes().forget_completed(&episode_id);
                Ok::<_, String>(())
            }
            .await;
            if let Err(error) = committed {
                tracing::error!("could not commit automatically stopped episode media: {error}");
                self.cameras().note_issue(format!(
                    "episode media could not be committed to SpiralDB: {error}"
                ));
            }
        }
        if let Some(token) = token {
            self.recording_session().release(token);
        }
    }

    pub(super) async fn finish_video_capture(&self, capture: VideoCapture) -> Vec<VideoAsset> {
        let (videos, failures) = capture.finish().await;
        for failure in failures {
            self.cameras()
                .note_issue(format!("episode video was not saved: {failure}"));
        }
        videos
    }
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
        let status = if spiral_robotics_storage::ProjectStore::is_unavailable_error(&message) {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::BAD_REQUEST
        };
        Self(status, message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self(StatusCode::CONFLICT, message.into())
    }

    fn catalog_read(message: impl Into<String>) -> Self {
        let message = message.into();
        let status = if spiral_robotics_storage::ProjectStore::is_unavailable_error(&message) {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        Self(status, message)
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
impl AppState {
    pub(crate) fn api_router(self) -> Router {
        let storage_routes = Self::recording_storage_router().route_layer(
            middleware::from_fn_with_state(self.clone(), Self::require_storage_ready),
        );
        let catalog_routes = Router::new()
            .merge(Self::data_catalog_router())
            .merge(Self::device_catalog_router())
            .merge(Self::recording_router())
            .merge(Self::training_rollout_router())
            .merge(Self::workflow_router())
            .route_layer(middleware::from_fn_with_state(
                self.clone(),
                Self::require_catalog_ready,
            ));

        Router::new()
            .merge(Self::data_bootstrap_router())
            .merge(Self::device_bootstrap_router())
            .merge(storage_routes)
            .merge(catalog_routes)
            .route("/events", get(Self::events))
            .with_state(self)
    }

    async fn require_storage_ready(
        State(state): State<Self>,
        request: Request,
        next: Next,
    ) -> Result<Response, Failure> {
        match state.bootstrap().storage_unavailable() {
            Some(detail) => Err(Failure::unavailable(format!(
                "SpiralDB storage is not ready: {detail}"
            ))),
            None => Ok(next.run(request).await),
        }
    }

    async fn require_catalog_ready(
        State(state): State<Self>,
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
    pub(crate) async fn snapshot(&self) -> StudioSnapshot {
        let state = self;
        let hardware = state.hardware().state();
        let readiness = state.bootstrap().current();
        let robots = if readiness.catalog_available() {
            let robot_adapter = devices::RobotHttpAdapter::prefetched(state).await;
            let mut summaries = Vec::new();
            for saved in RobotCatalogActions::new(&robot_adapter).list().await {
                let connection = hardware
                    .connections()
                    .get(saved.id())
                    .cloned()
                    .unwrap_or_default();
                let summary = robot_adapter
                    .summary(saved.id(), connection.to_api())
                    .await
                    .ok();
                let Some(mut summary) = summary else {
                    continue;
                };
                // Limits come from the motors, not the saved configuration, so they
                // are folded in here after loading the SpiralDB row.
                if let Some(limits) = hardware.limits().get(saved.id()) {
                    let motors = summary
                        .motors()
                        .iter()
                        .cloned()
                        .map(|mut motor| {
                            if let Some(limits) = limits.get(motor.name()) {
                                motor.set_limits(limits.to_api());
                            }
                            motor
                        })
                        .collect();
                    summary = summary.with_motors(motors);
                }
                summaries.push(summary);
            }
            summaries
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

        let cameras = devices::CameraDiscoveryAdapter::new(state.cameras().clone()).cameras();
        StudioSnapshot::new(readiness, "SpiralDB", hardware.to_api())
            .with_robots(robots)
            .with_cameras(cameras)
            .with_issues(issues)
    }

    /// Push setup changes and live telemetry to the browser.
    ///
    /// The snapshot is only re-sent when it actually differs, so an idle studio
    /// costs one small telemetry frame per tick instead of a full state dump.
    async fn events(
        State(state): State<Self>,
    ) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
        let stream = async_stream::stream! {
            let mut snapshot_timer = tokio::time::interval(SNAPSHOT_INTERVAL);
            snapshot_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut telemetry_timer = tokio::time::interval(TELEMETRY_INTERVAL);
            let mut readiness = state.bootstrap().subscribe();
            let (initial_training, mut training) = state.training().subscription();
            let (initial_workflow, mut workflow) = state.workflow().subscription();
            let mut last_workflow = initial_workflow.clone();
            let (initial_agent, mut workflow_agent) = state.workflow_agent().subscription();
            let mut last_agent_cursor = AgentCursor::from(&initial_agent);
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
                    changed = readiness.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let snapshot = state.snapshot().await;
                        if last_snapshot.as_ref() == Some(&snapshot) {
                            continue;
                        }
                        last_snapshot = Some(snapshot.clone());
                        if let Ok(event) = Event::default().event("state").json_data(&snapshot) {
                            yield Ok(event);
                        }
                    }
                    _ = snapshot_timer.tick() => {
                        let snapshot = state.snapshot().await;
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
                        let telemetry: spiral_robotics_api::robots::TelemetryFrame =
                            hardware.to_api();
                        if telemetry.sequence() == last_sequence {
                            continue;
                        }
                        last_sequence = telemetry.sequence();
                        if let Ok(event) = Event::default()
                            .event("telemetry")
                            .json_data(telemetry)
                        {
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
                                if let Ok(event) = Event::default()
                                    .event("workflow_update")
                                    .json_data(&snapshot)
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
                                let cursor = AgentCursor::from(&snapshot);
                                if !last_agent_cursor.advance_to(cursor) {
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
                                let cursor = AgentCursor::from(&snapshot);
                                if !last_agent_cursor.advance_to(cursor) {
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
}

struct AgentCursor {
    scope_id: String,
    revision: u64,
}

impl AgentCursor {
    fn advance_to(&mut self, next: Self) -> bool {
        if self.scope_id == next.scope_id && self.revision == next.revision {
            return false;
        }
        *self = next;
        true
    }
}

impl From<&spiral_robotics_api::workflow::WorkflowAgentStateResponse> for AgentCursor {
    fn from(state: &spiral_robotics_api::workflow::WorkflowAgentStateResponse) -> Self {
        Self {
            scope_id: state.scope_id().to_owned(),
            revision: state.revision(),
        }
    }
}

#[cfg(test)]
mod tests;
