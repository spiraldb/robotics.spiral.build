//! The studio's HTTP surface.
//!
//! Handlers stay thin on purpose: they translate JSON into a command for the
//! control thread or an operation on the robot workspace, and translate errors
//! back into a status code. Every rule about what is safe to command lives in
//! [`crate::backend::robotics::hardware`], where it applies no matter who is calling.

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, body::Body};
use bytes::{BufMut, Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use spiral_robotics::serial_ports::{
    available_ports, disconnected_ports, port_description, port_kind, port_names,
};
use spiral_robotics_core::api::{
    ApiError, CameraBinding, CameraStatus, ConnectionTestResponse, CreateRobotRequest,
    DataSettings, EpisodeSummary, ExportRequest, ExportResponse, GoalRequest, IdentifyPortRequest,
    IdentifyPortResponse, ImportResponse, JudgeEpisodeRequest, ModeSelection, PublicationStatus,
    RobotSummary, RolloutSnapshot, SerialPortSummary, SetRecordingPausedRequest,
    SetReplayPausedRequest, SpiralQueryRequest, SpiralQueryResponse, StartRecordingRequest,
    StartReplayRequest, StartRolloutRequest, StartSpiralReplayRequest, StartTrainingRequest,
    StopRecordingRequest, StudioSnapshot, TorqueRequest, TrainingRun, TrainingSnapshot,
    UpdateDataSettingsRequest, UpdateRobotRequest, default_teleoperation_pair,
};
use spiral_robotics_core::calibration::build_calibration;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

use crate::backend::recording::dataset::{
    MAX_ARCHIVE_COMPRESSED_BYTES, export_lerobot_archive_to_file_for_dataset,
    import_lerobot_archive_file_into_dataset,
};
use crate::backend::recording::episodes::{
    ArmCondition, EpisodeError, EpisodeStore, TimelineMetadata, TimelineSample, TimelineTraces,
    build_arms,
};
use crate::backend::robotics::cameras::{CameraManager, Frame};
use crate::backend::robotics::hardware::{HardwareHandle, RobotSpec};
use crate::backend::robotics::video::{
    CameraCaptureSpec, VideoAsset, VideoAssetMetadata, VideoCapture, VideoEncoderSettings,
    load_assets,
};
use crate::backend::robots::workspace::{
    RobotRecord, RobotWorkspace, WorkspaceError, validate_camera_role,
};
use crate::backend::rollout::manager::RolloutManager;
use crate::backend::storage::manager::StorageManager;
use crate::backend::storage::settings::DataSettingsStore;
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
    pub data_settings: Arc<DataSettingsStore>,
    pub storage: Arc<StorageManager>,
    pub training: Arc<TrainingManager>,
    pub rollout: Arc<RolloutManager>,
    pub recording_video: Arc<tokio::sync::Mutex<Option<VideoCapture>>>,
    pub video_encoder: VideoEncoderSettings,
    pub simulated: bool,
}

/// Finish camera recording when the control thread closes a take on its own
/// (for example, after an arm disconnects). The normal stop endpoint takes the
/// capture out of the slot first, so the two paths cannot publish each other's
/// video.
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
        let Some(capture_episode) = capture_episode else {
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

/// Close and stage any capture whose corresponding control-loop recording has
/// already ended. Also used during graceful backend shutdown.
pub(crate) async fn finish_stale_video_capture(state: &AppState) {
    let capture = state.recording_video.lock().await.take();
    let Some(capture) = capture else {
        return;
    };
    let episode_id = capture.episode_id().to_owned();
    let videos = finish_video_capture(state, capture).await;
    if let Err(error) = state.episodes.reload() {
        tracing::warn!(episode = %episode_id, "could not reload automatically stopped episode: {error}");
    }
    state.storage.stage_local(episode_id, videos);
}

async fn finish_video_capture(state: &AppState, capture: VideoCapture) -> Vec<VideoAsset> {
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

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_GATEWAY, message.into())
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (self.0, Json(ApiError::new(self.1))).into_response()
    }
}

impl From<EpisodeError> for Failure {
    fn from(error: EpisodeError) -> Self {
        let status = match &error {
            EpisodeError::InvalidId(_) | EpisodeError::Invalid(_) => StatusCode::BAD_REQUEST,
            EpisodeError::NotFound(_) => StatusCode::NOT_FOUND,
            EpisodeError::AlreadyExists(_) => StatusCode::CONFLICT,
            EpisodeError::Io { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(status, error.to_string())
    }
}

impl From<WorkspaceError> for Failure {
    fn from(error: WorkspaceError) -> Self {
        let status = match &error {
            WorkspaceError::InvalidId(_) | WorkspaceError::Invalid(_) => StatusCode::BAD_REQUEST,
            WorkspaceError::NotFound(_) => StatusCode::NOT_FOUND,
            WorkspaceError::AlreadyExists(_) | WorkspaceError::Conflict(_) => StatusCode::CONFLICT,
            WorkspaceError::Config(_) | WorkspaceError::Io { .. } => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
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

/// Build the studio's API routes.
pub(crate) fn api_router(state: AppState) -> Router {
    Router::new()
        .route("/state", get(get_state))
        .route("/events", get(events))
        .route("/ports", get(list_ports))
        .route("/ports/identify", post(identify_port))
        .route("/cameras", get(list_cameras))
        .route("/cameras/refresh", post(refresh_cameras))
        .route("/cameras/stream.mjpeg", get(camera_stream))
        .route("/cameras/snapshot.jpg", get(camera_snapshot))
        .route("/robots", get(list_robots).post(create_robot))
        .route(
            "/robots/import",
            post(import_robot_configuration).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/robots/{id}", get(get_robot))
        .route("/robots/{id}", patch(update_robot))
        .route("/robots/{id}", delete(delete_robot))
        .route(
            "/robots/{id}/configuration",
            get(export_robot_configuration),
        )
        .route("/robots/{id}/connect", post(connect_robot))
        .route("/robots/{id}/disconnect", post(disconnect_robot))
        .route("/robots/{id}/torque", post(set_torque))
        .route("/robots/{id}/goal", post(set_goal))
        .route("/robots/{id}/calibration/start", post(start_calibration))
        .route("/robots/{id}/calibration/finish", post(finish_calibration))
        .route("/robots/{id}/calibration/cancel", post(cancel_calibration))
        .route("/mode", post(set_mode))
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
        .route("/episodes/{id}", delete(delete_episode))
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
        .route(
            "/settings/data",
            get(get_data_settings).patch(update_data_settings),
        )
        .route("/settings/data/test/spiral", post(test_spiral))
        .route("/publications", get(list_publications))
        .route(
            "/spiral/publications/{episode}",
            post(sync_existing_episode_to_spiral),
        )
        .route("/spiral/query", post(spiral_query))
        .route("/spiral/replay", post(start_spiral_replay))
        .route("/spiral/video.mp4", get(spiral_video))
        .route(
            "/training/runs",
            get(list_training_runs).post(start_training),
        )
        .route("/training/runs/{id}/stop", post(stop_training))
        .route("/rollout", get(get_rollout).post(start_rollout))
        .route("/rollout/stop", post(stop_rollout))
        .with_state(state)
}

/// Assemble the current setup-level view of the studio.
pub(crate) fn build_snapshot(state: &AppState) -> StudioSnapshot {
    let hardware = state.hardware.state();
    let robots = state
        .workspace
        .list()
        .into_iter()
        .map(|record| {
            let connection = hardware
                .connections
                .get(&record.id)
                .cloned()
                .unwrap_or_default();
            let mut summary = record.summary(connection);
            // Limits come from the motors, not the file, so they are folded in
            // here rather than inside a summary built purely from what was
            // written on disk.
            if let Some(limits) = hardware.limits.get(&record.id) {
                for motor in &mut summary.motors {
                    if let Some(limits) = limits.get(&motor.name) {
                        motor.limits = *limits;
                    }
                }
            }
            summary
        })
        .collect();

    let mut issues = state.workspace.issues();
    issues.extend(state.cameras.issues());
    issues.extend(state.episodes.issues());
    issues.extend(
        state
            .storage
            .statuses()
            .into_iter()
            .filter(|status| status.state == "failed")
            .map(|status| format!("{} publication failed", status.episode_id)),
    );
    // A take that stopped itself has to be visible after the counters that were
    // describing it have gone.
    issues.extend(hardware.recording_issue);

    // The take being captured right now has a part file on disk, which the store
    // correctly reports as a recording that has not finished. It has not — it is
    // still going, and the live counters are what describe it. Listing it here
    // as well would draw it twice and count it as a casualty while it is still
    // being written.
    let recording = hardware
        .telemetry
        .recording
        .as_ref()
        .map(|progress| progress.episode.clone());
    let episodes = state
        .episodes
        .list()
        .into_iter()
        .filter(|episode| Some(&episode.id) != recording.as_ref())
        .collect();

    StudioSnapshot {
        workspace: state.workspace.directory().display().to_string(),
        simulated: state.simulated,
        robots,
        cameras: state.cameras.statuses(),
        mode: hardware.mode,
        episodes,
        episode_directory: state.episodes.directory().display().to_string(),
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
        let initial_rollout = state.rollout.snapshot();
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

async fn list_training_runs(State(state): State<AppState>) -> Json<TrainingSnapshot> {
    Json(state.training.snapshot())
}

async fn start_training(
    State(state): State<AppState>,
    Json(request): Json<StartTrainingRequest>,
) -> Result<(StatusCode, Json<TrainingRun>), Failure> {
    let run = state.training.start(request).await?;
    Ok((StatusCode::CREATED, Json(run)))
}

async fn stop_training(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    state.training.stop(&id)?;
    Ok(StatusCode::ACCEPTED)
}

async fn get_rollout(State(state): State<AppState>) -> Json<RolloutSnapshot> {
    Json(state.rollout.snapshot())
}

async fn start_rollout(
    State(state): State<AppState>,
    Json(request): Json<StartRolloutRequest>,
) -> Result<(StatusCode, Json<RolloutSnapshot>), Failure> {
    let training = state.training.snapshot();
    let run = training
        .runs
        .iter()
        .find(|run| run.id == request.run_id)
        .ok_or_else(|| Failure::bad_request("the selected training run is no longer available"))?;
    if run.phase.active() {
        return Err(Failure::bad_request(
            "stop or finish training before starting a rollout",
        ));
    }
    if !run
        .checkpoints
        .iter()
        .any(|checkpoint| checkpoint.step == request.checkpoint_step && checkpoint.inference_ready)
    {
        return Err(Failure::bad_request(
            "the selected checkpoint is no longer available",
        ));
    }
    let snapshot = state.rollout.start(request).await?;
    Ok((StatusCode::ACCEPTED, Json(snapshot)))
}

async fn stop_rollout(State(state): State<AppState>) -> Result<StatusCode, Failure> {
    state.rollout.stop().await?;
    Ok(StatusCode::ACCEPTED)
}

async fn list_ports(
    State(state): State<AppState>,
) -> Result<Json<Vec<SerialPortSummary>>, Failure> {
    let ports = tokio::task::spawn_blocking(available_ports)
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .map_err(|error| {
            Failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not list serial ports: {error}"),
            )
        })?;

    let hardware = state.hardware.state();
    let in_use = state
        .workspace
        .list()
        .into_iter()
        .filter(|record| {
            hardware
                .connections
                .get(&record.id)
                .is_some_and(|connection| connection.connected)
        })
        .filter_map(|record| Some((record.port()?.path, record.id)))
        .collect::<std::collections::BTreeMap<_, _>>();

    Ok(Json(
        ports
            .into_iter()
            .map(|port| SerialPortSummary {
                kind: port_kind(&port.port_type).to_owned(),
                description: port_description(&port.port_type),
                used_by: in_use.get(&port.port_name).cloned(),
                path: port.port_name,
            })
            .collect(),
    ))
}

async fn identify_port(
    Json(request): Json<IdentifyPortRequest>,
) -> Result<Json<IdentifyPortResponse>, Failure> {
    let after = tokio::task::spawn_blocking(available_ports)
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .map_err(|error| {
            Failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not list serial ports: {error}"),
            )
        })?;

    let removed = disconnected_ports(&request.before, &port_names(&after));
    let (port, message) = match removed.as_slice() {
        [port] => (
            Some(port.clone()),
            format!("Found the motor bus on {port}. Plug the cable back in."),
        ),
        [] => (
            None,
            "No port disappeared. Make sure you unplugged the arm, then try again.".to_owned(),
        ),
        ports => (
            None,
            format!(
                "{} ports disappeared ({}). Unplug only the arm you are setting up.",
                ports.len(),
                ports.join(", ")
            ),
        ),
    };

    Ok(Json(IdentifyPortResponse {
        port,
        removed,
        message,
    }))
}

async fn list_cameras(State(state): State<AppState>) -> Json<Vec<CameraStatus>> {
    Json(state.cameras.statuses())
}

async fn refresh_cameras(
    State(state): State<AppState>,
) -> Result<Json<Vec<CameraStatus>>, Failure> {
    let cameras = state.cameras.clone();
    tokio::task::spawn_blocking(move || cameras.refresh_blocking())
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Json(state.cameras.statuses()))
}

/// A camera is addressed by query string because its identifier may be a device
/// path containing slashes.
#[derive(Debug, Deserialize)]
struct CameraQuery {
    id: String,
}

async fn camera_stream(
    State(state): State<AppState>,
    Query(query): Query<CameraQuery>,
) -> Result<Response, Failure> {
    let receiver = state
        .cameras
        .subscribe(&query.id)
        .map_err(|error| Failure(StatusCode::NOT_FOUND, error))?;

    let frames = tokio_stream::wrappers::WatchStream::new(receiver)
        .filter_map(|frame| async move { frame })
        .scan(0_u64, |last, frame| {
            let fresh = frame.sequence != *last;
            *last = frame.sequence;
            futures_util::future::ready(Some(fresh.then_some(frame)))
        })
        .filter_map(|frame| async move { frame })
        .map(|frame| Ok::<Bytes, Infallible>(multipart_part(&frame)));

    Ok(Response::builder()
        .header(
            header::CONTENT_TYPE,
            format!("multipart/x-mixed-replace; boundary={MJPEG_BOUNDARY}"),
        )
        .header(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
        .header(header::PRAGMA, "no-cache")
        .header(header::EXPIRES, "0")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from_stream(frames))
        .expect("a valid MJPEG response"))
}

async fn camera_snapshot(
    State(state): State<AppState>,
    Query(query): Query<CameraQuery>,
) -> Result<Response, Failure> {
    let mut receiver = state
        .cameras
        .subscribe(&query.id)
        .map_err(|error| Failure(StatusCode::NOT_FOUND, error))?;

    // The stream may have just started, so wait briefly for the first frame.
    let available = receiver.borrow_and_update().clone();
    let frame = match available {
        Some(frame) => Some(frame),
        None => tokio::time::timeout(Duration::from_secs(5), async {
            receiver.changed().await.ok()?;
            receiver.borrow_and_update().clone()
        })
        .await
        .ok()
        .flatten(),
    };

    let frame = frame.ok_or_else(|| {
        Failure(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("camera {:?} has not produced a frame yet", query.id),
        )
    })?;

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(frame.bytes.clone()))
        .expect("a valid JPEG response"))
}

/// Wrap one JPEG as a part of a `multipart/x-mixed-replace` response.
fn multipart_part(frame: &Frame) -> Bytes {
    let mut part = BytesMut::with_capacity(frame.bytes.len() + 96);
    part.put_slice(b"--");
    part.put_slice(MJPEG_BOUNDARY.as_bytes());
    part.put_slice(b"\r\nContent-Type: image/jpeg\r\nContent-Length: ");
    part.put_slice(frame.bytes.len().to_string().as_bytes());
    part.put_slice(b"\r\n\r\n");
    part.put_slice(&frame.bytes);
    part.put_slice(b"\r\n");
    part.freeze()
}

async fn list_robots(State(state): State<AppState>) -> Json<Vec<RobotSummary>> {
    Json(build_snapshot(&state).robots)
}

async fn create_robot(
    State(state): State<AppState>,
    Json(request): Json<CreateRobotRequest>,
) -> Result<(StatusCode, Json<RobotSummary>), Failure> {
    let record = state.workspace.create(
        request.id.trim(),
        request.robot_type.as_deref(),
        request.port.as_deref(),
        request.baud_rate,
    )?;
    Ok((
        StatusCode::CREATED,
        Json(record.summary(Default::default())),
    ))
}

/// Add a complete unified robot JSON document as a new workspace instance.
async fn import_robot_configuration(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<RobotSummary>), Failure> {
    if body.is_empty() {
        return Err(Failure::bad_request(
            "choose a robot configuration JSON file to import",
        ));
    }
    let record = state.workspace.import_configuration(&body)?;
    Ok((
        StatusCode::CREATED,
        Json(record.summary(Default::default())),
    ))
}

/// Download the complete canonical JSON document for one robot.
async fn export_robot_configuration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, Failure> {
    let record = state.workspace.get(&id)?;
    let json = record
        .config
        .to_pretty_json()
        .map_err(WorkspaceError::from)?;
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}.json\"", record.id),
        )
        .body(Body::from(json))
        .expect("static robot configuration response headers are valid"))
}

async fn get_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RobotSummary>, Failure> {
    let connection = state
        .hardware
        .state()
        .connections
        .get(&id)
        .cloned()
        .unwrap_or_default();
    Ok(Json(state.workspace.get(&id)?.summary(connection)))
}

async fn update_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRobotRequest>,
) -> Result<Json<RobotSummary>, Failure> {
    let record = state.workspace.get(&id)?;
    let cameras = request
        .cameras
        .as_deref()
        .map(|bindings| {
            build_camera_object(&record.camera_object(), bindings, &state.cameras.statuses())
        })
        .transpose()
        .map_err(Failure::bad_request)?;

    if let Some(port) = &request.port {
        if port.path.trim().is_empty() || port.path.contains('\0') || port.baud_rate == 0 {
            return Err(Failure::bad_request(
                "a port needs a non-empty path without NUL bytes and a positive baud rate",
            ));
        }
        // The open bus was created from the stored port. Letting the file say
        // one thing while the hardware does another is how a UI starts lying.
        if state
            .hardware
            .state()
            .connections
            .get(&id)
            .is_some_and(|connection| connection.connected)
        {
            return Err(Failure(
                StatusCode::CONFLICT,
                format!("disconnect robot {id:?} before changing its serial port"),
            ));
        }
    }

    let record = state.workspace.update(
        &id,
        request.robot_type.as_deref(),
        request.port.as_ref(),
        cameras,
    )?;
    let connection = state
        .hardware
        .state()
        .connections
        .get(&id)
        .cloned()
        .unwrap_or_default();
    Ok(Json(record.summary(connection)))
}

/// Merge camera assignments into the robot's stored `cameras` object.
///
/// Fields this build does not understand are kept for cameras that keep their
/// role name, and the resolution of a known device is refreshed so the file
/// records what will actually be captured.
fn build_camera_object(
    existing: &Map<String, Value>,
    bindings: &[CameraBinding],
    discovered: &[CameraStatus],
) -> Result<Map<String, Value>, String> {
    let mut seen = BTreeSet::new();
    let mut cameras = Map::new();

    for binding in bindings {
        let name = binding.name.trim();
        validate_camera_role(name).map_err(|error| error.to_string())?;
        if !seen.insert(name.to_owned()) {
            return Err(format!("camera role {name:?} is used more than once"));
        }
        if binding.id.trim().is_empty() {
            return Err(format!("camera role {name:?} needs a device id"));
        }

        let mut fields = match existing.get(name) {
            Some(Value::Object(fields)) => fields.clone(),
            _ => Map::new(),
        };
        fields
            .entry("type".to_owned())
            .or_insert_with(|| Value::String("opencv".to_owned()));
        fields.insert("id".to_owned(), Value::String(binding.id.clone()));
        if let Some(camera) = discovered.iter().find(|camera| camera.id == binding.id) {
            fields.insert("width".to_owned(), Value::from(camera.width));
            fields.insert("height".to_owned(), Value::from(camera.height));
            fields.insert("fps".to_owned(), Value::from(camera.fps));
        }
        cameras.insert(name.to_owned(), Value::Object(fields));
    }

    Ok(cameras)
}

async fn delete_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    // Refuse before releasing anything, so a failed delete leaves the robot
    // exactly as it was.
    state.workspace.delete(&id)?;
    // A deleted robot must not stay open on the bus, and its connection
    // history must not outlive it.
    let _ = state.hardware.forget(&id).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn connect_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    let record = state.workspace.get(&id)?;
    let spec = robot_spec(&record)?;
    state.hardware.connect(spec).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn robot_spec(record: &RobotRecord) -> Result<RobotSpec, Failure> {
    let port = record.port().ok_or_else(|| {
        Failure::bad_request(format!(
            "robot {:?} has no serial port assigned; set one before connecting",
            record.id
        ))
    })?;
    Ok(RobotSpec {
        id: record.id.clone(),
        port,
        motors: record.motors()?,
        calibration: record.calibration()?,
    })
}

/// Try every usable saved robot once when the studio starts.
///
/// A failed bus stays disconnected with its error attached; one absent arm
/// never prevents the web interface or another arm from starting.
pub(crate) async fn auto_connect_saved_robots(state: AppState) {
    for record in state.workspace.list() {
        let spec = match robot_spec(&record) {
            Ok(spec) => spec,
            Err(error) => {
                tracing::info!(robot = %record.id, "saved robot is not ready for auto-connect: {}", error.1);
                continue;
            }
        };
        match state.hardware.connect(spec).await {
            Ok(()) => tracing::info!(robot = %record.id, "auto-connected saved robot"),
            Err(error) => tracing::warn!(robot = %record.id, "auto-connect failed: {error}"),
        }
    }
}

async fn disconnect_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    state.hardware.disconnect(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_torque(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<TorqueRequest>,
) -> Result<StatusCode, Failure> {
    state.hardware.set_torque(&id, request.enabled).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<GoalRequest>,
) -> Result<StatusCode, Failure> {
    if request.normalized.is_empty() {
        return Err(Failure::bad_request("no joints were given a target"));
    }
    state.hardware.set_goals(&id, request.normalized).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_calibration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    state.hardware.start_calibration(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn finish_calibration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RobotSummary>, Failure> {
    let record = state.workspace.get(&id)?;
    let motors = record.motors()?;
    // Every step up to the write can fail, and the sweep keeps running until
    // one of them has actually succeeded. An operator who spent a minute
    // moving joints should not have to redo it because the file was busy.
    let input = state.hardware.calibration_input(&id).await?;
    let calibration = build_calibration(&motors, &input)
        .map_err(|error| Failure::bad_request(error.to_string()))?;
    let record = state
        .workspace
        .save_calibration(&id, &motors, &calibration)?;

    // Stored. Nothing after this point may report the operation as failed.
    let _ = state.hardware.cancel_calibration(&id).await;
    if let Err(error) = state.hardware.apply_calibration(&id, calibration).await {
        // Only reachable if the arm went away between the sweep and the write;
        // the file is correct and the next connect picks it up.
        tracing::warn!("saved calibration for {id:?} but could not apply it live: {error}");
    }

    let connection = state
        .hardware
        .state()
        .connections
        .get(&id)
        .cloned()
        .unwrap_or_default();
    Ok(Json(record.summary(connection)))
}

async fn cancel_calibration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    state.hardware.cancel_calibration(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_mode(
    State(state): State<AppState>,
    Json(selection): Json<ModeSelection>,
) -> Result<StatusCode, Failure> {
    // Replay carries a trajectory that has to be read off disk, so it has its
    // own endpoint. Accepting it here would mean either loading the episode
    // behind the caller's back or entering a mode with nothing to play.
    if matches!(selection, ModeSelection::Replay { .. }) {
        return Err(Failure::bad_request(
            "start a replay with POST /api/episodes/replay, which loads the take first",
        ));
    }
    if matches!(selection, ModeSelection::Policy { .. }) {
        return Err(Failure::bad_request(
            "start policy control with POST /api/rollout, which validates and warms the checkpoint first",
        ));
    }
    state.hardware.set_mode(selection).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_episodes(State(state): State<AppState>) -> Json<Vec<EpisodeSummary>> {
    Json(state.episodes.list())
}

async fn get_data_settings(State(state): State<AppState>) -> Json<DataSettings> {
    Json(state.data_settings.public())
}

async fn update_data_settings(
    State(state): State<AppState>,
    Json(request): Json<UpdateDataSettingsRequest>,
) -> Result<Json<DataSettings>, Failure> {
    let settings = state
        .data_settings
        .update(request)
        .map_err(Failure::bad_request)?;
    state.training.refresh_readiness().await;
    Ok(Json(settings))
}

async fn test_spiral(State(state): State<AppState>) -> Json<ConnectionTestResponse> {
    Json(state.storage.test_spiral().await)
}

async fn list_publications(State(state): State<AppState>) -> Json<Vec<PublicationStatus>> {
    Json(state.storage.statuses())
}

async fn sync_existing_episode_to_spiral(
    State(state): State<AppState>,
    Path(episode): Path<String>,
) -> Result<StatusCode, Failure> {
    state
        .storage
        .sync_spiral_existing(episode)
        .map_err(Failure::bad_request)?;
    Ok(StatusCode::ACCEPTED)
}

async fn spiral_query(
    State(state): State<AppState>,
    Json(request): Json<SpiralQueryRequest>,
) -> Result<Json<SpiralQueryResponse>, Failure> {
    state
        .storage
        .query(&request.sql, request.max_rows)
        .await
        .map(Json)
        .map_err(Failure::bad_gateway)
}

async fn start_spiral_replay(
    State(state): State<AppState>,
    Json(request): Json<StartSpiralReplayRequest>,
) -> Result<StatusCode, Failure> {
    let (sample_hz, frames) = state
        .storage
        .trajectory(&request.episode, &request.stream)
        .await
        .map_err(Failure::bad_gateway)?;
    state
        .hardware
        .start_replay(&request.robot, &request.episode, frames, sample_hz)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct SpiralVideoQuery {
    episode: String,
    stream: String,
    segment: Option<u32>,
}

async fn spiral_video(
    State(state): State<AppState>,
    Query(query): Query<SpiralVideoQuery>,
) -> Result<Response, Failure> {
    let bytes = state
        .storage
        .video_asset(&query.episode, &query.stream, query.segment)
        .await
        .map_err(Failure::bad_gateway)?;
    let mut response = Body::from(bytes).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("video/mp4"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

/// Begin a take.
///
/// Record is the session-level command: it chooses the configured pair, enters
/// teleoperation, and only then opens the take. If opening the take fails, the
/// mode change is rolled back so a failed recording cannot leave an unnoticed
/// teleoperation session running.
async fn start_recording(
    State(state): State<AppState>,
    Json(mut request): Json<StartRecordingRequest>,
) -> Result<Json<EpisodeSummary>, Failure> {
    if state.hardware.state().telemetry.recording.is_some() {
        return Err(Failure::from(
            "a take is already recording; stop it first".to_owned(),
        ));
    }
    request.dataset_id = validated_dataset_id(&request.dataset_id)?;

    let (leader, follower) = default_teleoperation_pair(&build_snapshot(&state).robots)
        .ok_or_else(|| {
            Failure::bad_request("connect and calibrate two compatible arms before recording")
        })?;
    if request.arms.is_empty() {
        request.arms = vec![leader.clone(), follower.clone()];
    }

    let selection = ModeSelection::Teleoperate { leader, follower };
    let source = describe_mode(&selection);
    state.hardware.set_mode(selection).await?;

    match begin_recording(&state, request, source).await {
        Ok(summary) => Ok(summary),
        Err(error) => match state.hardware.set_mode(ModeSelection::Idle).await {
            Ok(()) => Err(error),
            Err(stop_error) => Err(Failure(
                error.0,
                format!(
                    "{} Teleoperation also could not stop: {stop_error}",
                    error.1
                ),
            )),
        },
    }
}

/// Open the take once the control thread is already teleoperating.
///
/// The arms and their joint order come from what the control thread is already
/// publishing, so a take's frames are laid out exactly like the telemetry the
/// operator was watching while it was captured.
async fn begin_recording(
    state: &AppState,
    request: StartRecordingRequest,
    source: String,
) -> Result<Json<EpisodeSummary>, Failure> {
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

    let wanted = if request.arms.is_empty() {
        connected.iter().map(|robot| robot.id.clone()).collect()
    } else {
        for id in &request.arms {
            if !connected.iter().any(|robot| &robot.id == id) {
                return Err(Failure::bad_request(format!(
                    "robot {id:?} is not connected, so it cannot be recorded"
                )));
            }
        }
        request.arms.clone()
    };
    if wanted.is_empty() {
        return Err(Failure::bad_request(
            "connect at least one arm before recording a take",
        ));
    }
    if state.recording_video.lock().await.is_some() {
        return Err(Failure(
            StatusCode::CONFLICT,
            "the previous episode's video is still being finalized; try again shortly".to_owned(),
        ));
    }

    let arms = wanted
        .iter()
        .filter_map(|id| {
            let robot = connected.iter().find(|robot| &robot.id == id)?;
            Some((
                id.clone(),
                robot
                    .joints
                    .iter()
                    .map(|joint| joint.name.clone())
                    .collect::<Vec<_>>(),
            ))
        })
        .collect::<Vec<_>>();
    let mut arms = build_arms(&arms).map_err(Failure::from)?;
    for arm in &mut arms {
        arm.robot_type = state
            .workspace
            .get(&arm.id)
            .ok()
            .and_then(|robot| robot.robot_type());
    }

    let note = request
        .note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .map(str::to_owned);
    // Rounded because this is the configured target, which is a whole number of
    // hertz; it is reconstructed from a nanosecond period, and writing
    // `60.0000024000001` into every take's header would be noise recorded as
    // precision.
    let sample_hz = if hardware.telemetry.target_hz > 0.0 {
        (hardware.telemetry.target_hz * 1_000.0).round() / 1_000.0
    } else {
        crate::backend::robotics::hardware::CONTROL_HZ as f64
    };

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
            request.dataset_id.clone(),
            source.clone(),
            note,
            sample_hz,
            arms.clone(),
            condition,
        )
        .map_err(Failure::from)?;
    let id = writer.id().to_owned();
    let started_ms = writer.started_ms();
    let video_settings = state.data_settings.get().video;
    let capture = if video_settings.enabled {
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
            &id,
            cameras,
            &state.video_encoder.executable,
            &state.video_encoder.codec,
        )
        .await
        .map_err(Failure::bad_request)?
    } else {
        None
    };
    // Hand the open take to the control thread. If it refuses, dropping the
    // writer leaves a part file, which is honest: nothing was captured, and the
    // next listing says the take never finished.
    if let Err(error) = state.hardware.start_recording(writer, arms.clone()).await {
        if let Some(capture) = capture {
            let _ = capture.finish().await;
        }
        return Err(Failure::from(error));
    }
    *state.recording_video.lock().await = capture;

    Ok(Json(EpisodeSummary {
        id,
        dataset_id: request.dataset_id,
        file: String::new(),
        arms,
        frames: 0,
        duration_seconds: 0.0,
        recording_gaps: Vec::new(),
        sample_hz,
        started_ms,
        source,
        max_follow_error_degrees: None,
        discarded: false,
        problem: None,
    }))
}

async fn stop_recording(
    State(state): State<AppState>,
    Json(request): Json<StopRecordingRequest>,
) -> Result<Json<EpisodeSummary>, Failure> {
    // Both exits from a take are exits from teleoperation as well. Attempt the
    // safety transition even if finalizing the file fails; the operator should
    // never have to repair storage before motion commands can stop.
    // Claim the video first. If the control thread publishes its stopped state
    // while this request is awaiting the reply, the automatic monitor must not
    // race this explicit stop and stage a second copy without its video.
    let capture = state.recording_video.lock().await.take();
    let recording = state.hardware.stop_recording(request.keep).await;
    let idle = state.hardware.set_mode(ModeSelection::Idle).await;
    let capture_episode = capture
        .as_ref()
        .map(|capture| capture.episode_id().to_owned());
    let videos = match capture {
        Some(capture) => finish_video_capture(&state, capture).await,
        None => Vec::new(),
    };

    match (recording, idle) {
        (Ok(summary), Ok(())) => {
            // The take only exists under its final name now, so the cache has
            // to catch up before anything lists it.
            state.episodes.reload().map_err(Failure::from)?;
            state.storage.stage_local(summary.id.clone(), videos);
            Ok(Json(summary))
        }
        (Ok(summary), Err(mode_error)) => {
            // The file did finish, so make it visible even though the safety
            // transition needs attention.
            state.episodes.reload().map_err(Failure::from)?;
            state.storage.stage_local(summary.id, videos);
            Err(Failure::from(format!(
                "the recording ended, but teleoperation could not stop: {mode_error}"
            )))
        }
        (Err(recording_error), Ok(())) => {
            if let Some(episode) = capture_episode {
                state.storage.stage_local(episode, videos);
            }
            Err(Failure::from(recording_error))
        }
        (Err(recording_error), Err(mode_error)) => {
            if let Some(episode) = capture_episode {
                state.storage.stage_local(episode, videos);
            }
            Err(Failure::from(format!(
                "{recording_error}; teleoperation also could not stop: {mode_error}"
            )))
        }
    }
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
    if request.paused {
        if let Some(capture) = state.recording_video.lock().await.as_ref() {
            capture.request_pause();
        }
        let paused = state.hardware.set_recording_paused(true).await;
        if let Some(capture) = state.recording_video.lock().await.as_mut() {
            for failure in capture.finish_pause().await {
                state
                    .cameras
                    .note_issue(format!("paused episode video was not saved: {failure}"));
            }
        }
        // Failure stays fail-safe: camera capture remains stopped and the
        // controller retains its paused/idle state instead of silently
        // desynchronizing media and joint samples.
        paused.map_err(Failure::from)?;
        // The writer barrier has made the `.part` file readable; refreshing
        // now exposes it to timeline/sample/trace requests by its stable ID.
        state.episodes.reload().map_err(Failure::from)?;
    } else {
        let active_seconds = state.hardware.set_recording_paused(false).await?;
        if let Some(capture) = state.recording_video.lock().await.as_mut()
            && let Err(error) = capture.resume(active_seconds)
        {
            state
                .cameras
                .note_issue(format!("episode video could not resume: {error}"));
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn judge_episode(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<JudgeEpisodeRequest>,
) -> Result<Json<EpisodeSummary>, Failure> {
    let episodes = state.episodes.clone();
    let summary = tokio::task::spawn_blocking(move || episodes.judge(&id, request.discarded))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    Ok(Json(summary))
}

async fn delete_episode(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    // A take being recorded right now is not a file to delete out from under
    // the writer.
    let hardware = state.hardware.state();
    if hardware
        .telemetry
        .recording
        .is_some_and(|recording| recording.episode == id)
    {
        return Err(Failure(
            StatusCode::CONFLICT,
            format!("take {id:?} is still recording; stop it first"),
        ));
    }
    if matches!(
        &hardware.mode.selection,
        ModeSelection::Replay { episode, .. } if episode == &id
    ) {
        return Err(Failure(
            StatusCode::CONFLICT,
            format!("take {id:?} is driving an arm; stop replay before deleting it"),
        ));
    }
    let video_directory = state.episodes.directory().join("videos").join(&id);
    let episodes = state.episodes.clone();
    tokio::task::spawn_blocking(move || episodes.delete(&id))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    // Camera files and their retry manifest are part of the local episode.
    // Keep deletion successful if a stale sidecar cannot be removed, but make
    // the leak visible to the backend operator.
    if let Err(error) = tokio::fs::remove_dir_all(&video_directory).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            video_directory = %video_directory.display(),
            "episode JSONL was deleted but its video sidecar could not be removed: {error}"
        );
    }
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
    let episodes = state.episodes.clone();
    let trace = tokio::task::spawn_blocking(move || episodes.trace(&id, query.buckets))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    Ok(Json(trace))
}

async fn episode_timeline_traces(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TraceQuery>,
) -> Result<Json<TimelineTraces>, Failure> {
    let episodes = state.episodes.clone();
    let traces = tokio::task::spawn_blocking(move || episodes.timeline_traces(&id, query.buckets))
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
    let episodes = state.episodes.clone();
    let result = tokio::task::spawn_blocking(move || {
        let samples = episodes.timeline_metadata(&id).map_err(Failure::from)?;
        // Timeline metadata refreshes a stale `.part` cache entry if the
        // recording finalized during this request. Read the summary after it
        // so the response carries the same durable filename and footer state.
        let episode = episodes.get(&id).map_err(Failure::from)?;
        let videos = load_assets(episodes.directory(), &id)
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?
            .iter()
            .map(VideoAsset::metadata)
            .collect();
        Ok::<_, Failure>(EpisodeTimelineResponse {
            episode,
            samples,
            videos,
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
    let episodes = state.episodes.clone();
    let sample = tokio::task::spawn_blocking(move || episodes.sample_at(&id, query.at))
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
    if !request.seconds.is_finite() || request.seconds < 0.0 {
        return Err(Failure::bad_request(
            "historical seek time must be a finite, non-negative number of seconds",
        ));
    }
    let hardware = state.hardware.state();
    let paused_current = hardware
        .telemetry
        .recording
        .as_ref()
        .is_some_and(|recording| recording.paused && recording.episode == id);
    if let Some(reason) = recording_seek_blocker(hardware.telemetry.recording.as_ref(), &id) {
        return Err(Failure(StatusCode::CONFLICT, reason));
    }
    if !matches!(
        hardware.mode.selection,
        ModeSelection::Idle | ModeSelection::Replay { .. }
    ) {
        return Err(Failure(
            StatusCode::CONFLICT,
            "stop the active control mode before moving a physical arm through history".to_owned(),
        ));
    }

    let snapshot = build_snapshot(&state);
    let target = snapshot
        .robots
        .iter()
        .find(|robot| robot.id == request.robot)
        .ok_or_else(|| Failure::bad_request(format!("no robot named {:?}", request.robot)))?;
    if !robot_is_follower(target) {
        return Err(Failure::bad_request(format!(
            "historical seek only drives an explicitly configured follower; robot {:?} is {:?}",
            request.robot, target.robot_type
        )));
    }
    if !target.connection.connected {
        return Err(Failure(
            StatusCode::CONFLICT,
            format!("connect follower {:?} before seeking", request.robot),
        ));
    }
    if !target.calibrated {
        return Err(Failure(
            StatusCode::CONFLICT,
            format!("calibrate follower {:?} before seeking", request.robot),
        ));
    }
    if let Some(problem) = &target.problem {
        return Err(Failure(
            StatusCode::CONFLICT,
            format!("follower {:?} cannot be driven: {problem}", request.robot),
        ));
    }

    let summary = state.episodes.get(&id).map_err(Failure::from)?;
    if let Some(problem) = &summary.problem {
        let live_partial = paused_current
            && summary.file.ends_with(".jsonl.part")
            && problem == "recording did not finish";
        if !live_partial {
            return Err(Failure::bad_request(format!(
                "take {id:?} can be inspected but cannot drive hardware: {problem}"
            )));
        }
    }
    let source = recorded_arm_for_target(&summary, &request.robot, request.source_arm.as_deref())?;
    let ticket = state.episodes.reserve_seek(&request.robot);
    let episodes = state.episodes.clone();
    let sample_id = id.clone();
    let seconds = request.seconds;
    let sample = tokio::task::spawn_blocking(move || episodes.sample_at(&sample_id, seconds))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;

    let response = |state, sample: &TimelineSample| HistoricalSeekResponse {
        state,
        episode: id.clone(),
        robot: request.robot.clone(),
        source_arm: source.clone(),
        requested_seconds: request.seconds,
        resolved_seconds: sample.seconds,
        frame: sample.frame,
    };
    let arm = sample
        .arms
        .iter()
        .find(|arm| arm.arm == source)
        .ok_or_else(|| {
            Failure::bad_request(format!(
                "take {id:?} did not record an arm named {source:?}"
            ))
        })?;
    let pose = arm
        .normalized
        .iter()
        .filter_map(|(joint, value)| Some((joint.clone(), (*value)?)))
        .collect::<std::collections::BTreeMap<_, _>>();
    if pose.is_empty() {
        return Err(Failure::bad_request(format!(
            "take {id:?} has no calibrated positions for {source:?} at {:.3}s",
            sample.seconds
        )));
    }

    let _gate = state.episodes.lock_seek().await;
    if !state.episodes.seek_is_current(&request.robot, ticket) {
        return Ok(Json(response("coalesced", &sample)));
    }
    state
        .hardware
        .start_physical_seek(&request.robot, &id, pose, summary.sample_hz)
        .await?;
    Ok(Json(response("seeking", &sample)))
}

fn recording_seek_blocker(
    recording: Option<&spiral_robotics_core::api::RecordingProgress>,
    episode: &str,
) -> Option<String> {
    let recording = recording?;
    if !recording.paused {
        return Some(
            "pause the active recording before moving a physical arm through history".to_owned(),
        );
    }
    (recording.episode != episode).then(|| {
        format!(
            "paused take {:?} is the only episode available for physical scrubbing",
            recording.episode
        )
    })
}

fn robot_is_follower(robot: &RobotSummary) -> bool {
    robot_type_is_follower(robot.robot_type.as_deref())
}

fn robot_type_is_follower(robot_type: Option<&str>) -> bool {
    robot_type.is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
}

fn recorded_arm_for_target(
    summary: &EpisodeSummary,
    target: &str,
    requested: Option<&str>,
) -> Result<String, Failure> {
    if let Some(arm) = requested {
        if summary.arm(arm).is_none() {
            return Err(Failure::bad_request(format!(
                "take {:?} did not record an arm named {arm:?}",
                summary.id
            )));
        }
        return Ok(arm.to_owned());
    }
    if summary.arm(target).is_some() {
        return Ok(target.to_owned());
    }
    match summary.arms.as_slice() {
        [only] => Ok(only.id.clone()),
        arms => Err(Failure::bad_request(format!(
            "take {:?} recorded {} arms ({}); say which one to use",
            summary.id,
            arms.len(),
            arms.iter()
                .map(|arm| arm.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ByteRange {
    start: u64,
    end: u64,
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
    state.episodes.get(&id).map_err(Failure::from)?;
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

fn parse_byte_range(value: &str, length: u64) -> Option<ByteRange> {
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
    if !request.start_seconds.is_finite() || request.start_seconds < 0.0 {
        return Err(Failure::bad_request(
            "replay start time must be a finite, non-negative number of seconds",
        ));
    }
    let summary = state
        .episodes
        .get(&request.episode)
        .map_err(Failure::from)?;
    // Prefer the arm's own recorded trajectory, then the take's only arm. A
    // take of two arms replayed onto one has to say which, because guessing
    // would drive an arm along the other arm's path.
    let source = recorded_arm_for_target(&summary, &request.robot, request.source_arm.as_deref())?;

    let episodes = state.episodes.clone();
    let episode = request.episode.clone();
    let trajectory = tokio::task::spawn_blocking(move || {
        episodes.trajectory_at(&episode, &source, request.start_seconds)
    })
    .await
    .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;

    state
        .hardware
        .start_replay_at_frame(
            &request.robot,
            &request.episode,
            trajectory.frames,
            summary.sample_hz,
            trajectory.start_frame,
            trajectory.resolved_start_seconds,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_replay_paused(
    State(state): State<AppState>,
    Json(request): Json<SetReplayPausedRequest>,
) -> Result<StatusCode, Failure> {
    state.hardware.set_replay_paused(request.paused).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn export_episodes(
    State(state): State<AppState>,
    Json(request): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, Failure> {
    let destination = resolve_export_directory(state.episodes.directory(), &request.directory)?;
    let episodes = state.episodes.clone();
    let response = tokio::task::spawn_blocking(move || episodes.export(&destination))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    Ok(Json(response))
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
    let dataset_id = query
        .dataset_id
        .as_deref()
        .map(validated_dataset_id)
        .transpose()?;
    let temporary = tempfile::NamedTempFile::new().map_err(|error| {
        Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not create a temporary export file: {error}"),
        )
    })?;
    let destination = temporary.reopen().map_err(|error| {
        Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not open the temporary export file: {error}"),
        )
    })?;
    let episodes = state.episodes.clone();
    let summary = tokio::task::spawn_blocking(move || {
        export_lerobot_archive_to_file_for_dataset(&episodes, dataset_id.as_deref(), destination)
    })
    .await
    .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    let length = temporary
        .as_file()
        .metadata()
        .map_err(|error| {
            Failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not inspect the temporary export file: {error}"),
            )
        })?
        .len();
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
    let dataset_id = validated_dataset_id(query.dataset_id.as_deref().unwrap_or("default"))?;
    let episodes = state.episodes.clone();
    let source = temporary.reopen().map_err(|error| {
        Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not reopen the uploaded ZIP: {error}"),
        )
    })?;
    let response = tokio::task::spawn_blocking(move || {
        let _temporary = temporary;
        import_lerobot_archive_file_into_dataset(&episodes, source, &dataset_id)
    })
    .await
    .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    Ok(Json(response))
}

fn validated_dataset_id(value: &str) -> Result<String, Failure> {
    let value = value.trim();
    if !spiral_robotics_core::spiral::is_dataset_id(value) {
        return Err(Failure::bad_request(
            "dataset ID must use 1–96 letters, numbers, '.', '_' or '-' characters",
        ));
    }
    Ok(value.to_owned())
}

/// Resolve an export destination, refusing anything that escapes the workspace.
///
/// A relative path lands beside the episode directory, which is where a dataset
/// belongs. An absolute path is the operator saying exactly where they want it,
/// and is taken at its word; what is refused is the shape that looks relative
/// and is not — a `..` that walks out of the directory it appears to be under.
fn resolve_export_directory(
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

/// The studio's own words for what produced a take.
fn describe_mode(selection: &ModeSelection) -> String {
    match selection {
        ModeSelection::Idle => "Idle — moved by hand".to_owned(),
        ModeSelection::Manual { robot } => format!("Manual — {robot}"),
        ModeSelection::Teleoperate { leader, follower } => {
            format!("Teleoperation — {leader} → {follower}")
        }
        ModeSelection::Replay { robot, episode } => format!("Replay — {episode} on {robot}"),
        ModeSelection::Policy { robot, session } => format!("Policy — {session} on {robot}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording_progress(
        episode: &str,
        paused: bool,
    ) -> spiral_robotics_core::api::RecordingProgress {
        spiral_robotics_core::api::RecordingProgress {
            episode: episode.to_owned(),
            dataset_id: "default".to_owned(),
            frames: 12,
            elapsed_seconds: 0.2,
            wall_elapsed_seconds: 0.2,
            paused_seconds: 0.0,
            paused,
            arms: vec!["leader".to_owned(), "follower".to_owned()],
            max_follow_error_degrees: None,
            error: None,
        }
    }

    #[test]
    fn only_the_matching_paused_recording_is_physically_seekable() {
        let active = recording_progress("episode_000001", false);
        assert!(recording_seek_blocker(Some(&active), &active.episode).is_some());

        let paused = recording_progress("episode_000001", true);
        assert_eq!(recording_seek_blocker(Some(&paused), &paused.episode), None);
        assert!(recording_seek_blocker(Some(&paused), "episode_000002").is_some());
        assert_eq!(recording_seek_blocker(None, "episode_000001"), None);
    }

    fn status(id: &str) -> CameraStatus {
        CameraStatus {
            id: id.to_owned(),
            name: "Test camera".to_owned(),
            width: 1280,
            height: 720,
            fps: 30,
            streaming: false,
            frames: 0,
            error: None,
        }
    }

    fn binding(name: &str, id: &str) -> CameraBinding {
        CameraBinding {
            mount: None,
            name: name.to_owned(),
            id: id.to_owned(),
        }
    }

    #[test]
    fn camera_assignments_keep_unknown_fields_and_refresh_resolution() {
        let existing = Map::from_iter([(
            "wrist".to_owned(),
            serde_json::json!({
                "type": "opencv",
                "id": "/dev/video9",
                "width": 320,
                "rotation": 180
            }),
        )]);

        let cameras = build_camera_object(
            &existing,
            &[binding("wrist", "/dev/video0")],
            &[status("/dev/video0")],
        )
        .expect("build cameras");

        assert_eq!(cameras["wrist"]["id"], "/dev/video0");
        assert_eq!(cameras["wrist"]["width"], 1280);
        assert_eq!(cameras["wrist"]["height"], 720);
        assert_eq!(cameras["wrist"]["fps"], 30);
        assert_eq!(cameras["wrist"]["rotation"], 180, "unknown fields survive");
    }

    #[test]
    fn a_camera_mount_survives_being_reassigned() {
        // Where a camera is bolted is written by hand into the robot file; the
        // studio has no control that sets it, so every camera edit the studio
        // does make has to leave it exactly where it was.
        let existing = Map::from_iter([(
            "wrist".to_owned(),
            serde_json::json!({
                "id": "/dev/video0",
                "mount": { "frame": "gripper_link", "xyz": [0.02, 0.0, 0.04] }
            }),
        )]);

        let cameras = build_camera_object(&existing, &[binding("wrist", "/dev/video9")], &[])
            .expect("build cameras");

        assert_eq!(cameras["wrist"]["id"], "/dev/video9");
        assert_eq!(cameras["wrist"]["mount"]["frame"], "gripper_link");
        assert_eq!(cameras["wrist"]["mount"]["xyz"][2], 0.04);
    }

    #[test]
    fn an_unknown_device_still_records_the_assignment() {
        let cameras = build_camera_object(&Map::new(), &[binding("top", "/dev/video7")], &[])
            .expect("build cameras");

        assert_eq!(cameras["top"]["id"], "/dev/video7");
        assert_eq!(cameras["top"]["type"], "opencv");
        assert!(cameras["top"].get("width").is_none());
    }

    #[test]
    fn removing_a_camera_drops_it_from_the_stored_object() {
        let existing = Map::from_iter([
            ("wrist".to_owned(), serde_json::json!({"id": "/dev/video0"})),
            ("top".to_owned(), serde_json::json!({"id": "/dev/video1"})),
        ]);

        let cameras =
            build_camera_object(&existing, &[binding("top", "/dev/video1")], &[]).expect("build");

        assert_eq!(cameras.len(), 1);
        assert!(cameras.contains_key("top"));
    }

    #[test]
    fn duplicate_or_empty_camera_roles_are_refused() {
        assert!(
            build_camera_object(
                &Map::new(),
                &[
                    binding("wrist", "/dev/video0"),
                    binding("wrist", "/dev/video1")
                ],
                &[],
            )
            .expect_err("duplicate role")
            .contains("more than once")
        );
        // A role becomes a key in the robot file and a label in the UI, so it
        // has to be a plain identifier rather than arbitrary text.
        for role in ["  ", "../../etc", "wrist cam"] {
            assert!(
                build_camera_object(&Map::new(), &[binding(role, "/dev/video0")], &[])
                    .expect_err("bad role")
                    .contains("camera role"),
                "accepted {role:?}"
            );
        }
        assert!(
            build_camera_object(&Map::new(), &[binding("wrist", " ")], &[])
                .expect_err("blank device")
                .contains("device id")
        );
    }

    #[test]
    fn multipart_parts_carry_an_accurate_length() {
        let frame = Frame {
            bytes: Bytes::from_static(&[0xff, 0xd8, 0x00, 0xff, 0xd9]),
            sequence: 1,
            captured_at: std::time::Instant::now(),
        };

        let part = multipart_part(&frame);
        let text = String::from_utf8_lossy(&part);

        assert!(text.starts_with("--spiral-robotics-frame\r\nContent-Type: image/jpeg\r\n"));
        assert!(text.contains("Content-Length: 5\r\n\r\n"));
        assert!(part.ends_with(b"\r\n"));
        assert_eq!(part.len(), 5 + part.len() - 5);
    }

    #[test]
    fn an_export_directory_may_not_climb_out_of_the_workspace() {
        let episodes = std::path::Path::new("/bench/spiral-robotics/episodes");

        // A plain name lands beside the episode directory, where a dataset goes.
        assert_eq!(
            resolve_export_directory(episodes, "pick-place-v1").unwrap(),
            std::path::PathBuf::from("/bench/spiral-robotics/pick-place-v1")
        );
        // An absolute path is the operator being explicit.
        assert_eq!(
            resolve_export_directory(episodes, "/data/sets/a").unwrap(),
            std::path::PathBuf::from("/data/sets/a")
        );
        // What is refused is the shape that reads as relative and is not.
        for escape in ["../../etc", "sets/../../..", ".."] {
            let error = resolve_export_directory(episodes, escape).expect_err(escape);
            assert_eq!(error.0, StatusCode::BAD_REQUEST);
            assert!(error.1.contains(".."), "{}", error.1);
        }
        assert_eq!(
            resolve_export_directory(episodes, "   ")
                .expect_err("blank")
                .0,
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn a_take_records_which_mode_produced_it() {
        assert_eq!(
            describe_mode(&ModeSelection::Teleoperate {
                leader: "leader".to_owned(),
                follower: "follower".to_owned(),
            }),
            "Teleoperation — leader → follower"
        );
        // An idle take is still a take: the operator moved the arm by hand.
        assert_eq!(describe_mode(&ModeSelection::Idle), "Idle — moved by hand");
    }

    #[test]
    fn local_video_ranges_cover_browser_seek_forms() {
        assert_eq!(
            parse_byte_range("bytes=10-19", 100),
            Some(ByteRange { start: 10, end: 19 })
        );
        assert_eq!(
            parse_byte_range("bytes=90-", 100),
            Some(ByteRange { start: 90, end: 99 })
        );
        assert_eq!(
            parse_byte_range("bytes=-10", 100),
            Some(ByteRange { start: 90, end: 99 })
        );
        assert_eq!(
            parse_byte_range("bytes=90-200", 100),
            Some(ByteRange { start: 90, end: 99 })
        );
        assert_eq!(parse_byte_range("bytes=100-", 100), None);
        assert_eq!(parse_byte_range("bytes=20-10", 100), None);
        assert_eq!(parse_byte_range("bytes=0-1,4-5", 100), None);
        assert_eq!(parse_byte_range("items=0-1", 100), None);
        assert_eq!(parse_byte_range("bytes=0-1", 0), None);
    }

    #[test]
    fn historical_seek_role_check_never_treats_a_leader_as_a_follower() {
        assert!(robot_type_is_follower(Some("so101_follower")));
        assert!(robot_type_is_follower(Some("SO101_FOLLOWER")));
        assert!(!robot_type_is_follower(Some("so101_leader")));
        assert!(!robot_type_is_follower(None));
    }

    #[test]
    fn episode_errors_map_to_the_right_status_codes() {
        let cases = [
            (
                EpisodeError::InvalidId("../x".to_owned()),
                StatusCode::BAD_REQUEST,
            ),
            (
                EpisodeError::NotFound("episode_000001".to_owned()),
                StatusCode::NOT_FOUND,
            ),
            (
                EpisodeError::AlreadyExists("/tmp/set".into()),
                StatusCode::CONFLICT,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(Failure::from(error).0, expected);
        }
    }

    #[test]
    fn workspace_errors_map_to_the_right_status_codes() {
        let cases = [
            (
                WorkspaceError::InvalidId("../x".to_owned()),
                StatusCode::BAD_REQUEST,
            ),
            (
                WorkspaceError::NotFound("arm".to_owned()),
                StatusCode::NOT_FOUND,
            ),
            (
                WorkspaceError::AlreadyExists("arm".to_owned()),
                StatusCode::CONFLICT,
            ),
            (
                WorkspaceError::Conflict("changed".to_owned()),
                StatusCode::CONFLICT,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(Failure::from(error).0, expected);
        }
        // A control-thread refusal is a conflict, not a crash.
        assert_eq!(
            Failure::from("robot is not connected".to_owned()).0,
            StatusCode::CONFLICT
        );
    }
}
