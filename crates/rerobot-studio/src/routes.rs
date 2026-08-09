//! The studio's HTTP surface.
//!
//! Handlers stay thin on purpose: they translate JSON into a command for the
//! control thread or an operation on the robot workspace, and translate errors
//! back into a status code. Every rule about what is safe to command lives in
//! [`crate::hardware`], where it applies no matter who is calling.

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, body::Body};
use bytes::{BufMut, Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use rerobot::serial_ports::{
    available_ports, disconnected_ports, port_description, port_kind, port_names,
};
use rerobot_core::api::{
    ApiError, CameraBinding, CameraStatus, CreateRobotRequest, EpisodeSummary, ExportRequest,
    ExportResponse, GoalRequest, IdentifyPortRequest, IdentifyPortResponse, ImportResponse,
    JudgeEpisodeRequest, ModeSelection, RobotSummary, SerialPortSummary, StartRecordingRequest,
    StartReplayRequest, StopRecordingRequest, StudioSnapshot, TorqueRequest, UpdateRobotRequest,
};
use rerobot_core::calibration::build_calibration;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::camera_manager::{CameraManager, Frame};
use crate::dataset::{export_lerobot_archive, import_lerobot_archive};
use crate::episodes::{EpisodeError, EpisodeStore, build_arms};
use crate::hardware::{HardwareHandle, RobotSpec};
use crate::workspace::{RobotRecord, RobotWorkspace, WorkspaceError, validate_camera_role};

/// Boundary marker separating parts of an MJPEG response.
const MJPEG_BOUNDARY: &str = "rerobotframe";

/// How often live telemetry is pushed to the browser.
const TELEMETRY_INTERVAL: Duration = Duration::from_millis(50);

/// How often the setup-level snapshot is rebuilt and diffed.
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(400);

/// Everything a request handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub workspace: Arc<RobotWorkspace>,
    pub episodes: Arc<EpisodeStore>,
    pub hardware: Arc<HardwareHandle>,
    pub cameras: Arc<CameraManager>,
    pub simulated: bool,
}

/// A failed request, rendered as `{"error": "..."}`.
#[derive(Debug)]
pub struct Failure(StatusCode, String);

impl Failure {
    fn bad_request(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
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
pub fn api_router(state: AppState) -> Router {
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
        .route("/episodes/record/stop", post(stop_recording))
        .route("/episodes/replay", post(start_replay))
        .route("/episodes/export", post(export_episodes))
        .route("/episodes/export.zip", get(download_episodes))
        .route(
            "/episodes/import",
            post(import_episodes).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route("/episodes/{id}", delete(delete_episode))
        .route("/episodes/{id}/judge", post(judge_episode))
        .route("/episodes/{id}/trace", get(episode_trace))
        .with_state(state)
}

/// Assemble the current setup-level view of the studio.
pub fn build_snapshot(state: &AppState) -> StudioSnapshot {
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
            record.summary(connection)
        })
        .collect();

    let mut issues = state.workspace.issues();
    issues.extend(state.cameras.issues());
    issues.extend(state.episodes.issues());
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
        let mut last_snapshot: Option<StudioSnapshot> = None;
        let mut last_sequence = 0_u64;

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
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
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
        .header(header::CACHE_CONTROL, "no-store")
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
pub async fn auto_connect_saved_robots(state: AppState) {
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
    state.hardware.set_mode(selection).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_episodes(State(state): State<AppState>) -> Json<Vec<EpisodeSummary>> {
    Json(state.episodes.list())
}

/// Begin a take.
///
/// The arms and their joint order come from what the control thread is already
/// publishing, so a take's frames are laid out exactly like the telemetry the
/// operator was watching while it was captured.
async fn start_recording(
    State(state): State<AppState>,
    Json(request): Json<StartRecordingRequest>,
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
    let arms = build_arms(&arms).map_err(Failure::from)?;

    let note = request
        .note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .map(str::to_owned);
    let source = describe_mode(&hardware.mode.selection);
    // Rounded because this is the configured target, which is a whole number of
    // hertz; it is reconstructed from a nanosecond period, and writing
    // `60.0000024000001` into every take's header would be noise recorded as
    // precision.
    let sample_hz = if hardware.telemetry.target_hz > 0.0 {
        (hardware.telemetry.target_hz * 1_000.0).round() / 1_000.0
    } else {
        crate::hardware::CONTROL_HZ as f64
    };

    let writer = state
        .episodes
        .begin(source, note, sample_hz, arms.clone())
        .map_err(Failure::from)?;
    let id = writer.id().to_owned();
    let started_ms = writer.started_ms();
    // Hand the open take to the control thread. If it refuses, dropping the
    // writer leaves a part file, which is honest: nothing was captured, and the
    // next listing says the take never finished.
    state.hardware.start_recording(writer, arms.clone()).await?;

    Ok(Json(EpisodeSummary {
        id,
        file: String::new(),
        arms,
        frames: 0,
        duration_seconds: 0.0,
        sample_hz,
        started_ms,
        source: describe_mode(&hardware.mode.selection),
        max_follow_error_degrees: None,
        discarded: false,
        problem: None,
    }))
}

async fn stop_recording(
    State(state): State<AppState>,
    Json(request): Json<StopRecordingRequest>,
) -> Result<Json<EpisodeSummary>, Failure> {
    let summary = state.hardware.stop_recording(request.keep).await?;
    // The take only exists under its final name now, so the cache has to catch
    // up before anything lists it.
    state.episodes.reload().map_err(Failure::from)?;
    Ok(Json(summary))
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
    if state
        .hardware
        .state()
        .telemetry
        .recording
        .is_some_and(|recording| recording.episode == id)
    {
        return Err(Failure(
            StatusCode::CONFLICT,
            format!("take {id:?} is still recording; stop it first"),
        ));
    }
    let episodes = state.episodes.clone();
    tokio::task::spawn_blocking(move || episodes.delete(&id))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
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

async fn start_replay(
    State(state): State<AppState>,
    Json(request): Json<StartReplayRequest>,
) -> Result<StatusCode, Failure> {
    let summary = state
        .episodes
        .get(&request.episode)
        .map_err(Failure::from)?;
    // Prefer the arm's own recorded trajectory, then the take's only arm. A
    // take of two arms replayed onto one has to say which, because guessing
    // would drive an arm along the other arm's path.
    let source = match &request.source_arm {
        Some(arm) => arm.clone(),
        None if summary.arm(&request.robot).is_some() => request.robot.clone(),
        None => match summary.arms.as_slice() {
            [only] => only.id.clone(),
            arms => {
                return Err(Failure::bad_request(format!(
                    "take {:?} recorded {} arms ({}); say which one to replay",
                    request.episode,
                    arms.len(),
                    arms.iter()
                        .map(|arm| arm.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        },
    };

    let episodes = state.episodes.clone();
    let episode = request.episode.clone();
    let frames = tokio::task::spawn_blocking(move || episodes.trajectory(&episode, &source))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;

    state
        .hardware
        .start_replay(&request.robot, &request.episode, frames, summary.sample_hz)
        .await?;
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
async fn download_episodes(State(state): State<AppState>) -> Result<Response, Failure> {
    let episodes = state.episodes.clone();
    let archive = tokio::task::spawn_blocking(move || export_lerobot_archive(&episodes))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    let mut response = Body::from(archive.bytes).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/zip"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_static("attachment; filename=\"rerobot-lerobot-v3-dataset.zip\""),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-rerobot-episodes"),
        header::HeaderValue::from_str(&archive.episodes.to_string())
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-rerobot-frames"),
        header::HeaderValue::from_str(&archive.frames.to_string())
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-rerobot-skipped"),
        header::HeaderValue::from_str(&archive.skipped.to_string())
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    Ok(response)
}

/// Convert a browser-uploaded LeRobot ZIP into replayable local episodes.
async fn import_episodes(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<ImportResponse>, Failure> {
    if body.is_empty() {
        return Err(Failure::bad_request(
            "choose a LeRobot dataset ZIP to import",
        ));
    }
    let episodes = state.episodes.clone();
    let response = tokio::task::spawn_blocking(move || import_lerobot_archive(&episodes, &body))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))??;
    Ok(Json(response))
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };

        let part = multipart_part(&frame);
        let text = String::from_utf8_lossy(&part);

        assert!(text.starts_with("--rerobotframe\r\nContent-Type: image/jpeg\r\n"));
        assert!(text.contains("Content-Length: 5\r\n\r\n"));
        assert!(part.ends_with(b"\r\n"));
        assert_eq!(part.len(), 5 + part.len() - 5);
    }

    #[test]
    fn an_export_directory_may_not_climb_out_of_the_workspace() {
        let episodes = std::path::Path::new("/bench/rerobot/episodes");

        // A plain name lands beside the episode directory, where a dataset goes.
        assert_eq!(
            resolve_export_directory(episodes, "pick-place-v1").unwrap(),
            std::path::PathBuf::from("/bench/rerobot/pick-place-v1")
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
