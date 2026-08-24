use super::*;
use spiral_robotics_studio_application::catalog_lease::{
    CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease,
};

/// Camera discovery and preview do not depend on SpiralDB, so they remain
/// useful while catalog bootstrap is still running or needs repair.
pub(super) fn bootstrap_router() -> Router<AppState> {
    Router::new()
        .route("/cameras", get(list_cameras))
        .route("/cameras/refresh", post(refresh_cameras))
        .route("/cameras/stream.mjpeg", get(camera_stream))
        .route("/cameras/snapshot.jpg", get(camera_snapshot))
}

pub(super) fn catalog_router() -> Router<AppState> {
    Router::new()
        .route("/ports", get(list_ports))
        .route("/ports/identify", post(identify_port))
        .route("/robots", get(list_robots).post(create_robot))
        .route(
            "/robots/import",
            post(import_robot_configuration).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route(
            "/robots/{id}",
            get(get_robot).patch(update_robot).delete(delete_robot),
        )
        .route(
            "/robots/{id}/configuration",
            get(export_robot_configuration),
        )
        .route("/robots/{id}/connect", post(connect_robot))
        .route("/robots/{id}/disconnect", post(disconnect_robot))
        .route("/robots/{id}/hold", post(hold_robot))
        .route("/robots/{id}/release", post(release_robot))
        .route("/robots/{id}/goal", post(set_goal))
        .route("/robots/{id}/calibration/start", post(start_calibration))
        .route("/robots/{id}/calibration/finish", post(finish_calibration))
        .route("/robots/{id}/calibration/cancel", post(cancel_calibration))
        .route(
            "/robots/{id}/calibration/reapply",
            post(reapply_calibration),
        )
        .route("/mode", post(set_mode))
}

struct SystemSerialPortInventory;

impl SerialPortInventory for SystemSerialPortInventory {
    async fn available_port_paths(&self) -> Result<Vec<String>, ApplicationError> {
        tokio::task::spawn_blocking(available_ports)
            .await
            .map_err(|error| ApplicationError::Internal(error.to_string()))?
            .map(|ports| ports.into_iter().map(|port| port.port_name).collect())
            .map_err(|error| ApplicationError::Internal(error.to_string()))
    }
}

struct SerialPortQueryAdapter<'a> {
    state: &'a AppState,
}

impl SerialPortQueryPort for SerialPortQueryAdapter<'_> {
    async fn serial_ports(&self) -> Result<Vec<SerialPortDescriptor>, ApplicationError> {
        let ports = tokio::task::spawn_blocking(available_ports)
            .await
            .map_err(|error| ApplicationError::Internal(error.to_string()))?
            .map_err(|error| {
                ApplicationError::Internal(format!("could not list serial ports: {error}"))
            })?;
        let hardware = self.state.hardware.state();
        let in_use = self
            .state
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
        Ok(ports
            .into_iter()
            .map(|port| SerialPortDescriptor {
                kind: port_kind(&port.port_type).to_owned(),
                description: port_description(&port.port_type),
                used_by: in_use.get(&port.port_name).cloned(),
                path: port.port_name,
            })
            .collect())
    }
}

#[derive(Clone)]
pub(super) struct CameraDiscoveryAdapter(pub(super) Arc<CameraManager>);

impl CameraDiscoveryPort for CameraDiscoveryAdapter {
    fn cameras(&self) -> Vec<DiscoveredCamera> {
        self.0
            .statuses()
            .into_iter()
            .map(discovered_camera)
            .collect()
    }

    async fn refresh_cameras(&self) -> Result<Vec<DiscoveredCamera>, ApplicationError> {
        let cameras = self.0.clone();
        tokio::task::spawn_blocking(move || cameras.refresh_blocking())
            .await
            .map_err(|error| ApplicationError::Internal(error.to_string()))?;
        Ok(self
            .0
            .statuses()
            .into_iter()
            .map(discovered_camera)
            .collect())
    }
}

pub(super) fn discovered_camera(camera: CameraStatus) -> DiscoveredCamera {
    DiscoveredCamera {
        id: camera.id,
        name: camera.name,
        width: camera.width,
        height: camera.height,
        fps: camera.fps,
        streaming: camera.streaming,
        frames: camera.frames,
        error: camera.error,
    }
}

/// Adapts cached JSON robot documents and the live control thread to plain
/// application commands. JSON preservation, wire summaries, and concrete
/// hardware specifications remain at this outer edge.
pub(super) struct RobotHttpAdapter<'a> {
    state: &'a AppState,
    import_document: Option<Bytes>,
    saved_records: Option<Vec<RobotRecord>>,
}

impl<'a> RobotHttpAdapter<'a> {
    pub(super) fn new(state: &'a AppState) -> Self {
        Self {
            state,
            import_document: None,
            saved_records: None,
        }
    }

    pub(super) fn prefetched(state: &'a AppState) -> Self {
        Self {
            state,
            import_document: None,
            saved_records: Some(state.workspace.list()),
        }
    }

    fn import(state: &'a AppState, document: Bytes) -> Self {
        Self {
            state,
            import_document: Some(document),
            saved_records: None,
        }
    }

    pub(super) fn summary(
        &self,
        robot: &str,
        connection: ConnectionStatus,
    ) -> Result<RobotSummary, WorkspaceError> {
        let record = match &self.saved_records {
            Some(records) => records
                .iter()
                .find(|record| record.id == robot)
                .cloned()
                .ok_or_else(|| WorkspaceError::NotFound(robot.to_owned()))?,
            None => self.state.workspace.get(robot)?,
        };
        Ok(record.summary(connection))
    }
}

impl RobotRepository for RobotHttpAdapter<'_> {
    type CameraConfiguration = Map<String, Value>;

    fn create(&self, robot: NewRobot) -> Result<SavedRobot, ApplicationError> {
        let (port, baud_rate) = robot
            .port
            .map(|port| (Some(port.path), Some(port.baud_rate)))
            .unwrap_or_default();
        let record = self
            .state
            .workspace
            .create(
                &robot.id,
                robot.robot_type.as_deref(),
                port.as_deref(),
                baud_rate,
            )
            .map_err(workspace_application_error)?;
        Ok(SavedRobot { id: record.id })
    }

    fn import_configuration(&self) -> Result<SavedRobot, ApplicationError> {
        let configuration = self.import_document.as_ref().ok_or_else(|| {
            ApplicationError::Internal("missing robot import document".to_owned())
        })?;
        let record = self
            .state
            .workspace
            .import_configuration(configuration)
            .map_err(workspace_application_error)?;
        Ok(SavedRobot { id: record.id })
    }

    fn export_configuration(&self, robot: &str) -> Result<ExportedRobotDocument, ApplicationError> {
        self.state
            .workspace
            .get(robot)
            .and_then(|record| record.config.to_pretty_json().map_err(WorkspaceError::from))
            .map(|document| ExportedRobotDocument {
                bytes: document.into_bytes(),
            })
            .map_err(workspace_application_error)
    }

    fn camera_configuration(
        &self,
        robot: &str,
        assignments: &[CameraAssignment],
    ) -> Result<Self::CameraConfiguration, ApplicationError> {
        let record = self
            .state
            .workspace
            .get(robot)
            .map_err(workspace_application_error)?;
        Ok(build_camera_object(
            &record.camera_object(),
            assignments,
            &self.state.cameras.statuses(),
        ))
    }

    fn update(
        &self,
        command: RobotUpdate<Map<String, Value>>,
    ) -> Result<SavedRobot, ApplicationError> {
        let port = command.port.map(|port| PortSettings {
            path: port.path,
            baud_rate: port.baud_rate,
        });
        let record = self
            .state
            .workspace
            .update(
                &command.id,
                command.robot_type.as_deref(),
                port.as_ref(),
                command.cameras,
            )
            .map_err(workspace_application_error)?;
        Ok(SavedRobot { id: record.id })
    }

    fn ensure_exists(&self, robot: &str) -> Result<(), ApplicationError> {
        self.state
            .workspace
            .get(robot)
            .map(|_| ())
            .map_err(workspace_application_error)
    }

    fn delete(&self, robot: &str) -> Result<(), ApplicationError> {
        self.state
            .workspace
            .delete(robot)
            .map_err(workspace_application_error)
    }

    fn saved_robot_ids(&self) -> Vec<String> {
        match &self.saved_records {
            Some(records) => records.iter().map(|record| record.id.clone()).collect(),
            None => self
                .state
                .workspace
                .list()
                .into_iter()
                .map(|record| record.id)
                .collect(),
        }
    }

    fn connection_spec(&self, robot: &str) -> Result<RobotConnectionSpec, ApplicationError> {
        let record = self
            .state
            .workspace
            .get(robot)
            .map_err(workspace_application_error)?;
        let port = record.port().ok_or_else(|| {
            ApplicationError::InvalidInput(format!(
                "robot {:?} has no serial port assigned; set one before connecting",
                record.id
            ))
        })?;
        let motors = record.motors().map_err(workspace_application_error)?;
        let calibration = record.calibration().map_err(workspace_application_error)?;
        Ok(RobotConnectionSpec {
            id: record.id,
            port: SerialPortConfiguration {
                path: port.path,
                baud_rate: port.baud_rate,
            },
            motors,
            calibration,
        })
    }
}

impl RobotConnectionPort for RobotHttpAdapter<'_> {
    fn is_connected(&self, robot: &str) -> bool {
        self.state
            .hardware
            .state()
            .connections
            .get(robot)
            .is_some_and(|connection| connection.connected)
    }

    async fn connect(&self, specification: RobotConnectionSpec) -> Result<(), ApplicationError> {
        self.state
            .hardware
            .connect(RobotSpec {
                id: specification.id,
                port: PortSettings {
                    path: specification.port.path,
                    baud_rate: specification.port.baud_rate,
                },
                motors: specification.motors,
                calibration: specification.calibration,
            })
            .await
            .map_err(ApplicationError::conflict)
    }

    async fn disconnect(&self, robot: &str) -> Result<(), ApplicationError> {
        self.state
            .hardware
            .disconnect(robot)
            .await
            .map_err(ApplicationError::conflict)
    }

    async fn forget(&self, robot: &str) -> Result<(), ApplicationError> {
        self.state
            .hardware
            .forget(robot)
            .await
            .map_err(ApplicationError::conflict)
    }
}

impl RobotCalibrationRepository for RobotWorkspace {
    fn motor_specs(&self, robot: &str) -> Result<BTreeMap<String, MotorSpec>, ApplicationError> {
        self.get(robot)
            .and_then(|record| record.motors())
            .map_err(workspace_application_error)
    }

    fn save_calibration(
        &self,
        robot: &str,
        motors: &BTreeMap<String, MotorSpec>,
        calibration: &Calibration,
    ) -> Result<(), ApplicationError> {
        RobotWorkspace::save_calibration(self, robot, motors, calibration)
            .map(|_| ())
            .map_err(workspace_application_error)
    }
}

struct CalibrationHistoryHttpAdapter<'a> {
    state: &'a AppState,
}

impl CalibrationHistoryPort for CalibrationHistoryHttpAdapter<'_> {
    fn reserve_catalog_mutation(&self) -> Result<Option<CatalogLeaseToken>, ApplicationError> {
        reserve_calibration_history_mutation(&self.state.catalog_lease, || {
            (
                self.state.workflow.is_active(),
                self.state.workflow_agent.is_active(),
            )
        })
    }

    fn release_catalog_mutation(&self, token: CatalogLeaseToken) {
        let released = self.state.catalog_lease.release(token);
        debug_assert!(
            released,
            "calibration history refresh releases its exact catalog lease once"
        );
    }

    fn activity(&self) -> CalibrationHistoryActivity {
        let hardware = self.state.hardware.state();
        CalibrationHistoryActivity {
            recording: hardware
                .telemetry
                .recording
                .map(|recording| recording.episode),
            replaying: match hardware.mode.selection {
                ModeSelection::Replay { episode, .. } => Some(episode),
                _ => None,
            },
            training_active: self.state.training.snapshot().runs.iter().any(|run| {
                matches!(
                    run.phase,
                    spiral_robotics_contracts::api::TrainingRunPhase::Starting
                        | spiral_robotics_contracts::api::TrainingRunPhase::Running
                        | spiral_robotics_contracts::api::TrainingRunPhase::Stopping
                )
            }),
        }
    }

    fn current_calibration(&self, robot: &str) -> Result<Option<Calibration>, ApplicationError> {
        let record = self
            .state
            .workspace
            .get(robot)
            .map_err(workspace_application_error)?;
        let motors = record.motors().map_err(workspace_application_error)?;
        let calibration = record.calibration().map_err(workspace_application_error)?;
        if let Some(calibration) = &calibration {
            calibration
                .validate_for_motor_specs(&motors)
            .map_err(|error| {
                ApplicationError::InvalidInput(format!(
                    "robot {robot:?} has calibration that does not match its configured motors: {error}"
                ))
            })?;
        }
        Ok(calibration)
    }

    async fn reapply_positions(
        &self,
        robot: String,
        calibration: Calibration,
    ) -> Result<ReappliedPositions, ApplicationError> {
        let episodes = self.state.episodes.clone();
        let result =
            tokio::task::spawn_blocking(move || episodes.reapply_calibration(&robot, &calibration))
                .await
                .map_err(|error| ApplicationError::Internal(error.to_string()))?
                .map_err(episode_application_error)?;
        Ok(ReappliedPositions {
            updated_episode_ids: result.updated_episode_ids,
            frames_updated: result.frames_updated,
            episodes_unchanged: result.episodes_unchanged,
            episodes_skipped: result.episodes_skipped,
            warnings: result.warnings,
        })
    }
}

fn reserve_calibration_history_mutation(
    lease: &CatalogMutationLease,
    activity: impl FnOnce() -> (bool, bool),
) -> Result<Option<CatalogLeaseToken>, ApplicationError> {
    let Some(token) = lease.try_acquire(CatalogLeaseOwner::CalibrationReapply) else {
        return Ok(None);
    };
    let (workflow_active, assistant_active) = activity();
    let conflict = if workflow_active {
        Some(
            "Wait for the active Workflow run to finish before reapplying calibration to episode history.",
        )
    } else if assistant_active {
        Some(
            "Wait for the active Workflow Assistant turn to finish before reapplying calibration to episode history.",
        )
    } else {
        None
    };
    if let Some(message) = conflict {
        let released = lease.release(token);
        debug_assert!(
            released,
            "rejected calibration-history admission releases its exact catalog lease"
        );
        return Err(ApplicationError::Conflict(message.to_owned()));
    }
    Ok(Some(token))
}

async fn list_ports(
    State(state): State<AppState>,
) -> Result<Json<Vec<SerialPortSummary>>, Failure> {
    Ok(Json(
        SerialPortQueryAdapter { state: &state }
            .serial_ports()
            .await?
            .into_iter()
            .map(|port| SerialPortSummary {
                kind: port.kind,
                description: port.description,
                used_by: port.used_by,
                path: port.path,
            })
            .collect(),
    ))
}

async fn identify_port(
    Json(request): Json<IdentifyPortRequest>,
) -> Result<Json<IdentifyPortResponse>, Failure> {
    let result = IdentifySerialPortCommand {
        before: request.before,
    }
    .execute(&SystemSerialPortInventory)
    .await?;
    Ok(Json(IdentifyPortResponse {
        port: result.port,
        removed: result.removed,
        message: result.guidance,
    }))
}

async fn list_cameras(State(state): State<AppState>) -> Json<Vec<CameraStatus>> {
    Json(
        CameraDiscoveryAdapter(state.cameras.clone())
            .cameras()
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
    )
}

async fn refresh_cameras(
    State(state): State<AppState>,
) -> Result<Json<Vec<CameraStatus>>, Failure> {
    Ok(Json(
        CameraDiscoveryAdapter(state.cameras.clone())
            .refresh_cameras()
            .await?
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
    ))
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
pub(super) fn multipart_part(frame: &Frame) -> Bytes {
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
    let adapter = RobotHttpAdapter::prefetched(&state);
    let hardware = state.hardware.state();
    Json(
        robots::list(&adapter)
            .into_iter()
            .filter_map(|robot| {
                let connection = hardware
                    .connections
                    .get(&robot.id)
                    .cloned()
                    .unwrap_or_default();
                adapter.summary(&robot.id, connection).ok()
            })
            .collect(),
    )
}

pub(super) fn present_saved_robot(
    state: &AppState,
    robot: SavedRobot,
) -> Result<RobotSummary, Failure> {
    let connection = state
        .hardware
        .state()
        .connections
        .get(&robot.id)
        .cloned()
        .unwrap_or_default();
    Ok(state.workspace.get(&robot.id)?.summary(connection))
}

async fn create_robot(
    State(state): State<AppState>,
    Json(request): Json<CreateRobotRequest>,
) -> Result<(StatusCode, Json<RobotSummary>), Failure> {
    let adapter = RobotHttpAdapter::new(&state);
    let robot = robots::create(
        &adapter,
        CreateRobotCommand {
            id: request.id,
            robot_type: request.robot_type,
            port: request.port,
            baud_rate: request.baud_rate,
        },
    )?;
    log_auto_connect_outcome(
        robots::auto_connect_robot(&adapter, &adapter, robot.id.clone()).await,
    );
    Ok((
        StatusCode::CREATED,
        Json(present_saved_robot(&state, robot)?),
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
    let adapter = RobotHttpAdapter::import(&state, body);
    let robot = robots::import_configuration(&adapter)?;
    log_auto_connect_outcome(
        robots::auto_connect_robot(&adapter, &adapter, robot.id.clone()).await,
    );
    Ok((
        StatusCode::CREATED,
        Json(present_saved_robot(&state, robot)?),
    ))
}

/// Download the complete canonical JSON document for one robot.
async fn export_robot_configuration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, Failure> {
    let adapter = RobotHttpAdapter::new(&state);
    let document = robots::export_configuration(&adapter, &id)?;
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{id}.json\""),
        )
        .body(Body::from(document.bytes))
        .expect("static robot configuration response headers are valid"))
}

async fn get_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RobotSummary>, Failure> {
    let robot = robots::get(&RobotHttpAdapter::new(&state), id)?;
    let connection = state
        .hardware
        .state()
        .connections
        .get(&robot.id)
        .cloned()
        .unwrap_or_default();
    Ok(Json(state.workspace.get(&robot.id)?.summary(connection)))
}

async fn update_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRobotRequest>,
) -> Result<Json<RobotSummary>, Failure> {
    let adapter = RobotHttpAdapter::new(&state);
    let should_auto_connect = request.port.is_some();
    let robot = robots::update(
        &adapter,
        &adapter,
        UpdateRobotCommand {
            id: id.clone(),
            robot_type: request.robot_type,
            port: request.port.map(|port| SerialPortConfiguration {
                path: port.path,
                baud_rate: port.baud_rate,
            }),
            cameras: request.cameras.map(|bindings| {
                bindings
                    .into_iter()
                    .map(|binding| CameraAssignment {
                        role: binding.name,
                        device_id: binding.id,
                    })
                    .collect()
            }),
        },
    )?;
    if should_auto_connect {
        log_auto_connect_outcome(
            robots::auto_connect_robot(&adapter, &adapter, robot.id.clone()).await,
        );
    }
    Ok(Json(present_saved_robot(&state, robot)?))
}

/// Merge camera assignments into the robot's stored `cameras` object.
///
/// Fields this build does not understand are kept for cameras that keep their
/// role name, and the resolution of a known device is refreshed so the file
/// records what will actually be captured.
pub(super) fn build_camera_object(
    existing: &Map<String, Value>,
    assignments: &[CameraAssignment],
    discovered: &[CameraStatus],
) -> Map<String, Value> {
    let mut cameras = Map::new();

    for assignment in assignments {
        let mut fields = match existing.get(&assignment.role) {
            Some(Value::Object(fields)) => fields.clone(),
            _ => Map::new(),
        };
        fields
            .entry("type".to_owned())
            .or_insert_with(|| Value::String("opencv".to_owned()));
        fields.insert("id".to_owned(), Value::String(assignment.device_id.clone()));
        if let Some(camera) = discovered
            .iter()
            .find(|camera| camera.id == assignment.device_id)
        {
            fields.insert("width".to_owned(), Value::from(camera.width));
            fields.insert("height".to_owned(), Value::from(camera.height));
            fields.insert("fps".to_owned(), Value::from(camera.fps));
        }
        cameras.insert(assignment.role.clone(), Value::Object(fields));
    }

    cameras
}

async fn delete_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    let adapter = RobotHttpAdapter::new(&state);
    robots::delete(&adapter, &adapter, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn connect_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    let adapter = RobotHttpAdapter::new(&state);
    robots::connect(&adapter, &adapter, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Try every usable saved robot once when the studio starts.
///
/// A failed bus stays disconnected with its error attached; one absent arm
/// never prevents the web interface or another arm from starting.
pub(crate) async fn auto_connect_saved_robots(state: AppState) {
    let adapter = RobotHttpAdapter::new(&state);
    for outcome in robots::auto_connect(&adapter, &adapter).await {
        log_auto_connect_outcome(outcome);
    }
}

fn log_auto_connect_outcome(outcome: AutoConnectOutcome) {
    match outcome.status {
        AutoConnectStatus::Connected => {
            tracing::info!(robot = %outcome.robot, "auto-connected saved robot")
        }
        AutoConnectStatus::Skipped(error) => {
            tracing::info!(robot = %outcome.robot, "saved robot is not ready for auto-connect: {error}")
        }
        AutoConnectStatus::Failed(error) => {
            tracing::warn!(robot = %outcome.robot, "auto-connect failed: {error}")
        }
    }
}

async fn disconnect_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    let adapter = RobotHttpAdapter::new(&state);
    robots::disconnect(&adapter, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn hold_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    robot_control::execute(
        state.hardware.as_ref(),
        RobotControlCommand::Hold { robot: id },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn release_robot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    robot_control::execute(
        state.hardware.as_ref(),
        RobotControlCommand::Release { robot: id },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<GoalRequest>,
) -> Result<StatusCode, Failure> {
    robot_control::execute(
        state.hardware.as_ref(),
        RobotControlCommand::MoveJoints {
            robot: id,
            normalized: request.normalized,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_calibration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    calibration::start(state.hardware.as_ref(), &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn finish_calibration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RobotSummary>, Failure> {
    let output =
        calibration::finish(state.workspace.as_ref(), state.hardware.as_ref(), &id).await?;
    if let Some(warning) = output.live_warning {
        tracing::warn!(robot = %id, "saved calibration but live refresh was incomplete: {warning}");
    }

    let record = state.workspace.get(&id)?;
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
    calibration::cancel(state.hardware.as_ref(), &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reapply_calibration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CalibrationReapplyResponse>, Failure> {
    let output =
        calibration::reapply_history(&CalibrationHistoryHttpAdapter { state: &state }, &id).await?;
    Ok(Json(CalibrationReapplyResponse {
        episodes_updated: output.episodes_updated,
        frames_updated: output.frames_updated,
        episodes_unchanged: output.episodes_unchanged,
        episodes_skipped: output.episodes_skipped,
        warnings: output.warnings,
    }))
}

async fn set_mode(
    State(state): State<AppState>,
    Json(selection): Json<ModeSelection>,
) -> Result<StatusCode, Failure> {
    // Replay carries a trajectory that has to be read from SpiralDB, so it has its
    // own endpoint. Accepting it here would mean either loading the episode
    // behind the caller's back or entering a mode with nothing to play.
    let mode = match selection {
        ModeSelection::Idle => OperatorMode::Idle,
        ModeSelection::Manual { robot } => OperatorMode::Manual { robot },
        ModeSelection::Teleoperate { leader, follower } => {
            OperatorMode::Teleoperate { leader, follower }
        }
        ModeSelection::Replay { .. } => {
            return Err(Failure::bad_request(
                "start a replay with POST /api/episodes/replay, which loads the take first",
            ));
        }
        ModeSelection::Policy { .. } => {
            return Err(Failure::bad_request(
                "start policy control with POST /api/rollout, which validates and warms the checkpoint first",
            ));
        }
    };
    mode_control::set_mode(state.hardware.as_ref(), mode).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    #[test]
    fn active_workflow_or_assistant_blocks_history_rewrite_and_releases_admission() {
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
            let error = reserve_calibration_history_mutation(&lease, || activity)
                .expect_err("published activity must block a later history rewrite");
            assert!(error.to_string().contains(expected));

            let next = lease
                .try_acquire(next_owner)
                .expect("a rejected history rewrite must release its admission lease");
            assert!(lease.release(next));
        }
    }

    #[test]
    fn admitted_history_rewrite_blocks_later_workflow_and_assistant_admission() {
        let lease = CatalogMutationLease::default();
        let calibration = reserve_calibration_history_mutation(&lease, || (false, false))
            .expect("idle activity check")
            .expect("calibration-history admission");

        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowStart)
                .is_none(),
            "manual Workflow cannot pass an admitted history rewrite"
        );
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowAgentStart)
                .is_none(),
            "Assistant cannot pass an admitted history rewrite"
        );
        assert!(lease.release(calibration));
    }
}
