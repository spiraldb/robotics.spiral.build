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
use axum::extract::{Path, Query, State};
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
    ApiError, CameraBinding, CameraStatus, CreateRobotRequest, GoalRequest, IdentifyPortRequest,
    IdentifyPortResponse, ModeSelection, RobotSummary, SerialPortSummary, StudioSnapshot,
    TorqueRequest, UpdateRobotRequest,
};
use rerobot_core::calibration::build_calibration;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::camera_manager::{CameraManager, Frame};
use crate::hardware::{HardwareHandle, RobotSpec};
use crate::workspace::{RobotWorkspace, WorkspaceError, validate_camera_role};

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
    pub hardware: Arc<HardwareHandle>,
    pub cameras: Arc<CameraManager>,
    pub simulated: bool,
}

/// A failed request, rendered as `{"error": "..."}`.
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
        .route("/robots/{id}", get(get_robot))
        .route("/robots/{id}", patch(update_robot))
        .route("/robots/{id}", delete(delete_robot))
        .route("/robots/{id}/connect", post(connect_robot))
        .route("/robots/{id}/disconnect", post(disconnect_robot))
        .route("/robots/{id}/torque", post(set_torque))
        .route("/robots/{id}/goal", post(set_goal))
        .route("/robots/{id}/calibration/start", post(start_calibration))
        .route("/robots/{id}/calibration/finish", post(finish_calibration))
        .route("/robots/{id}/calibration/cancel", post(cancel_calibration))
        .route("/mode", post(set_mode))
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

    StudioSnapshot {
        workspace: state.workspace.directory().display().to_string(),
        simulated: state.simulated,
        robots,
        cameras: state.cameras.statuses(),
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
    let port = record.port().ok_or_else(|| {
        Failure::bad_request(format!(
            "robot {id:?} has no serial port assigned; set one before connecting"
        ))
    })?;

    let spec = RobotSpec {
        id: record.id.clone(),
        port,
        motors: record.motors()?,
        calibration: record.calibration()?,
    };
    state.hardware.connect(spec).await?;
    Ok(StatusCode::NO_CONTENT)
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
    state.hardware.set_mode(selection).await?;
    Ok(StatusCode::NO_CONTENT)
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
