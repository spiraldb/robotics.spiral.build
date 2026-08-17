//! The studio's HTTP surface.
//!
//! Handlers translate delivery models into framework-neutral application
//! commands and present their results. Application use cases own admission and
//! orchestration; core entities and the serialized control thread provide the
//! final invariant and hardware-safety backstops.

mod data;
mod devices;
mod recordings;
mod training_rollout;

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
use spiral_robotics::serial_ports::{available_ports, port_description, port_kind};
use spiral_robotics_contracts::api::{
    ApiError, BackendReadiness, BackendReadinessState, CalibrationReapplyResponse, CameraStatus,
    ConnectionTestResponse, CreateRobotRequest, DataSettings, EpisodeCatalogResponse,
    EpisodeSummary, GoalRequest, IdentifyPortRequest, IdentifyPortResponse, JudgeEpisodeRequest,
    ModeSelection, PortSettings, RobotSummary, RolloutSnapshot, SerialPortSummary,
    SetRecordingPausedRequest, SetReplayPausedRequest, SpiralQueryRequest, SpiralQueryResponse,
    SpiralSettings as ContractSpiralSettings, StartRecordingRequest, StartReplayRequest,
    StartRolloutRequest, StartSpiralReplayRequest, StartTrainingRequest, StopRecordingRequest,
    StudioSnapshot, TrainingPolicy as ContractTrainingPolicy, TrainingRun, TrainingSnapshot,
    UpdateDataSettingsRequest, UpdateRobotRequest, VideoSettings, default_teleoperation_pair,
};
use spiral_robotics_core::calibration::{Calibration, CalibrationInput, MotorSpec};

use crate::backend::application::calibration::{
    self, CalibrationControlPort, CalibrationHistoryActivity, CalibrationHistoryPort,
    ReappliedPositions, RobotCalibrationRepository,
};
use crate::backend::application::catalog_lease::{
    CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease,
};
use crate::backend::application::data::{
    self as data_application, ConnectionCheck, DataCatalogPort, DataSettingsView,
    QueryColumn as ApplicationQueryColumn, QueryCommand, QueryResult as ApplicationQueryResult,
    QueryValue, ReplayTrajectory as SpiralReplayTrajectory, SpiralDestinationSettings,
    SpiralReplayPort, StartSpiralReplayCommand, UpdateDataSettingsCommand, VideoAssetQuery,
};
use crate::backend::application::device_discovery::{
    self as device_discovery, CameraDiscoveryPort, DiscoveredCamera, IdentifySerialPortCommand,
    SerialPortInventory, SerialPortQueryPort, SerialPortView,
};
use crate::backend::application::episode_queries::{
    self, EpisodeArmView, EpisodeQueryPort, EpisodeTimelineView, EpisodeTraceQuery, EpisodeView,
    RecordingGapView, TimelineArmSampleView, TimelineMetadataView, TimelineSampleView,
    TimelineTraceLaneView, TimelineTracePointView, TimelineTraceSeriesView, TimelineTracesView,
    VideoAssetView,
};
use crate::backend::application::episode_seek::{
    self, ActiveRecording, HistoricalSample, HistoricalSeekState, PhysicalSeekPlan,
    PhysicalSeekPort, SampledArm, SeekActivity, SeekControlMode, SeekEpisodeCommand, SeekTarget,
    SeekableEpisode,
};
use crate::backend::application::episodes::{
    self as episode_catalog, DeleteEpisodeCommand, EpisodeActivity, EpisodeCatalogPort,
    EpisodeMutationResult, JudgeEpisodeCommand,
};
use crate::backend::application::error::ApplicationError;
use crate::backend::application::mode_control::{self, ModeControlPort, OperatorMode};
use crate::backend::application::recording::{
    self, RecordingContext, RecordingPort, RecordingResult, ReplayEpisode, ReplayPlan, ReplayPort,
    ReplayTrajectory, SetRecordingPausedCommand, SetReplayPausedCommand, StartRecordingCommand,
    StartReplayCommand, StopRecordingCommand, TeleoperationPair,
};
use crate::backend::application::robot_control::{self, RobotControlCommand, RobotControlPort};
use crate::backend::application::robots::{
    self, AutoConnectStatus, CameraAssignment, CreateRobotCommand, ExportedRobotDocument, NewRobot,
    RobotConnectionPort, RobotConnectionSpec, RobotRepository, RobotUpdate, SavedRobot,
    SerialPortConfiguration, UpdateRobotCommand,
};
use crate::backend::application::rollout::{
    self as rollout_application, RolloutMetrics as ApplicationRolloutMetrics,
    RolloutPhase as ApplicationRolloutPhase, RolloutRequest, RolloutSessionPort,
    RolloutSnapshot as ApplicationRolloutSnapshot,
};
use crate::backend::application::training::{
    self as training_application, ActTrainingSettings, DeleteTrainingRunCommand,
    StartTrainingCommand, TrainingCheckpoint as ApplicationTrainingCheckpoint,
    TrainingCheckpointStatus, TrainingMetric as ApplicationTrainingMetric,
    TrainingPolicy as ApplicationTrainingPolicy, TrainingPort,
    TrainingReadiness as ApplicationTrainingReadiness, TrainingRun as ApplicationTrainingRun,
    TrainingRunActivity, TrainingRunPhase as ApplicationTrainingRunPhase, TrainingRunStatus,
    TrainingSnapshot as ApplicationTrainingSnapshot, TrainingSource as ApplicationTrainingSource,
};
use crate::backend::recording::episodes::{
    ArmCondition, EpisodeError, EpisodeStore, EpisodeWriter, TimelineMetadata, TimelineSample,
    TimelineTraces, build_arms,
};
use crate::backend::robotics::cameras::{CameraManager, Frame};
use crate::backend::robotics::hardware::{HardwareHandle, RobotSpec};
use crate::backend::robotics::video::{
    CameraCaptureSpec, VideoAsset, VideoAssetMetadata, VideoCapture, VideoEncoderSettings,
};
use crate::backend::robots::workspace::{RobotWorkspace, WorkspaceError};
use crate::backend::rollout::manager::RolloutManager;
use crate::backend::storage::manager::StorageManager;
use crate::backend::storage::settings::BackendStateStore;
use crate::backend::storage::settings::StudioPreferencesStore;
use crate::backend::training::manager::TrainingManager;

/// Boundary marker separating parts of an MJPEG response.
const MJPEG_BOUNDARY: &str = "spiral-robotics-frame";

/// How often live telemetry is pushed to the browser.
const TELEMETRY_INTERVAL: Duration = Duration::from_millis(50);

/// How often the setup-level snapshot is rebuilt and diffed.
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(400);

/// A short debounce keeps a just-started recording from being mistaken for an
/// automatic stop before the next telemetry snapshot is published.
const VIDEO_RECORDING_MONITOR_INTERVAL: Duration = Duration::from_millis(200);
const VIDEO_RECORDING_STOP_TICKS: u8 = 5;

/// Everything a request handler can reach.
#[derive(Clone)]
pub(crate) struct AppState {
    pub workspace: Arc<RobotWorkspace>,
    pub episodes: Arc<EpisodeStore>,
    pub hardware: Arc<HardwareHandle>,
    pub cameras: Arc<CameraManager>,
    pub backend_state: Arc<BackendStateStore>,
    pub preferences: Arc<StudioPreferencesStore>,
    pub storage: Arc<StorageManager>,
    pub training: Arc<TrainingManager>,
    pub rollout: Arc<RolloutManager>,
    pub bootstrap: BootstrapState,
    pub catalog_lease: Arc<CatalogMutationLease>,
    pub recording_video: Arc<tokio::sync::Mutex<Option<VideoCapture>>>,
    pub video_encoder: VideoEncoderSettings,
}

/// Shared application lifecycle published to the browser and consulted by
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
        self.current.send_replace(BackendReadiness {
            state: BackendReadinessState::Bootstrapping,
            catalog_available: false,
            detail: detail.into(),
        });
    }

    pub(crate) fn ready(&self) {
        self.current.send_replace(BackendReadiness {
            state: BackendReadinessState::Ready,
            catalog_available: true,
            detail: "Studio services are ready".to_owned(),
        });
    }

    pub(crate) fn degraded(&self, catalog_available: bool, detail: impl Into<String>) {
        self.current.send_replace(BackendReadiness {
            state: BackendReadinessState::Degraded,
            catalog_available,
            detail: detail.into(),
        });
    }

    fn catalog_unavailable(&self) -> Option<String> {
        let current = self.current();
        (!current.catalog_available).then(|| current.detail)
    }
}

pub(crate) async fn monitor_video_recording(state: AppState) {
    let mut ticker = tokio::time::interval(VIDEO_RECORDING_MONITOR_INTERVAL);
    let mut missing_episode = String::new();
    let mut missing_ticks = 0_u8;
    loop {
        ticker.tick().await;
        let capture_episode = state
            .recording_video
            .lock()
            .await
            .as_ref()
            .map(|capture| capture.episode_id().to_owned());
        let watched_episode = state.catalog_lease.recording_episode().or(capture_episode);
        let Some(capture_episode) = watched_episode else {
            missing_episode.clear();
            missing_ticks = 0;
            continue;
        };
        let hardware_episode = state
            .hardware
            .state()
            .telemetry
            .recording
            .map(|recording| recording.episode);
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
    let recording = state.catalog_lease.claim_recording();
    let capture = state.recording_video.lock().await.take();
    if recording.is_none() && capture.is_none() {
        return;
    }
    let episode_id = recording
        .as_ref()
        .map(|recording| recording.episode.clone())
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
        let episodes = state.episodes.clone();
        let storage = state.storage.clone();
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
            state.cameras.note_issue(format!(
                "episode media could not be committed to SpiralDB: {error}"
            ));
        }
    }
    if let Some(recording) = recording {
        let released = state.catalog_lease.release(recording.token);
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
            .cameras
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
        let status = if crate::backend::storage::repository::is_unavailable_error(&message) {
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

impl From<ApplicationError> for Failure {
    fn from(error: ApplicationError) -> Self {
        let status = match error {
            ApplicationError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            ApplicationError::NotFound(_) => StatusCode::NOT_FOUND,
            ApplicationError::Conflict(_) => StatusCode::CONFLICT,
            ApplicationError::External(_) => StatusCode::BAD_GATEWAY,
            ApplicationError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(status, error.to_string())
    }
}

fn episode_application_error(error: EpisodeError) -> ApplicationError {
    let message = error.to_string();
    match error {
        EpisodeError::InvalidId(_) | EpisodeError::Invalid(_) => {
            ApplicationError::InvalidInput(message)
        }
        EpisodeError::NotFound(_) => ApplicationError::NotFound(message),
    }
}

fn workspace_application_error(error: WorkspaceError) -> ApplicationError {
    let message = error.to_string();
    match error {
        WorkspaceError::InvalidId(_) | WorkspaceError::Invalid(_) => {
            ApplicationError::InvalidInput(message)
        }
        WorkspaceError::NotFound(_) => ApplicationError::NotFound(message),
        WorkspaceError::AlreadyExists(_) => ApplicationError::Conflict(message),
        WorkspaceError::Config(_) => ApplicationError::Internal(message),
    }
}

/// Build the studio's API routes.
pub(crate) fn api_router(state: AppState) -> Router {
    let catalog_routes = Router::new()
        .merge(data::catalog_router())
        .merge(devices::catalog_router())
        .merge(recordings::router())
        .merge(training_rollout::router())
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
    match state.bootstrap.catalog_unavailable() {
        Some(detail) => Err(Failure::unavailable(format!(
            "Studio catalog is not ready: {detail}"
        ))),
        None => Ok(next.run(request).await),
    }
}

/// Assemble the current setup-level view of the studio.
pub(crate) fn build_snapshot(state: &AppState) -> StudioSnapshot {
    let hardware = state.hardware.state();
    let readiness = state.bootstrap.current();
    let robots = if readiness.catalog_available {
        let robot_adapter = devices::RobotHttpAdapter::new(state);
        robots::list(&robot_adapter)
            .into_iter()
            .filter_map(|saved| {
                let connection = hardware
                    .connections
                    .get(&saved.id)
                    .cloned()
                    .unwrap_or_default();
                let mut summary = state.workspace.get(&saved.id).ok()?.summary(connection);
                // Limits come from the motors, not the saved configuration, so they
                // are folded in here after loading the SpiralDB row.
                if let Some(limits) = hardware.limits.get(&saved.id) {
                    for motor in &mut summary.motors {
                        if let Some(limits) = limits.get(&motor.name) {
                            motor.limits = *limits;
                        }
                    }
                }
                Some(summary)
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut issues = if readiness.catalog_available {
        state.workspace.issues()
    } else {
        Vec::new()
    };
    issues.extend(state.cameras.issues());
    // A take that stopped itself has to be visible after the counters that were
    // describing it have gone.
    issues.extend(hardware.recording_issue);

    StudioSnapshot {
        readiness,
        storage: "SpiralDB".to_owned(),
        robots,
        cameras: device_discovery::list_cameras(&state.cameras)
            .into_iter()
            .map(|camera| CameraStatus {
                id: camera.id,
                name: camera.name,
                width: camera.width,
                height: camera.height,
                fps: camera.fps,
                streaming: camera.streaming,
                frames: camera.frames,
                error: camera.error,
            })
            .collect(),
        mode: hardware.mode,
        issues,
    }
}

async fn get_state(State(state): State<AppState>) -> Json<StudioSnapshot> {
    Json(build_snapshot(&state))
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
        let mut telemetry_timer = tokio::time::interval(TELEMETRY_INTERVAL);
        let (initial_training, mut training) = state.training.subscription();
        let initial_rollout = RolloutManager::snapshot(state.rollout.as_ref());
        let mut rollout = state.rollout.subscribe();
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

        loop {
            tokio::select! {
                _ = snapshot_timer.tick() => {
                    let snapshot = build_snapshot(&state);
                    if last_snapshot.as_ref() == Some(&snapshot) {
                        continue;
                    }
                    last_snapshot = Some(snapshot.clone());
                    if let Ok(event) = Event::default().event("state").json_data(&snapshot) {
                        yield Ok(event);
                    }
                }
                _ = telemetry_timer.tick() => {
                    let telemetry = state.hardware.state().telemetry;
                    if telemetry.sequence == last_sequence {
                        continue;
                    }
                    last_sequence = telemetry.sequence;
                    if let Ok(event) = Event::default().event("telemetry").json_data(&telemetry) {
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
                            let (snapshot, replacement) = state.training.subscription();
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

#[cfg(test)]
mod tests;
