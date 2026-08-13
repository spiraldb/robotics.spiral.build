use super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new()
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
    async fn serial_ports(&self) -> Result<Vec<SerialPortView>, ApplicationError> {
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
            .map(|port| SerialPortView {
                kind: port_kind(&port.port_type).to_owned(),
                description: port_description(&port.port_type),
                used_by: in_use.get(&port.port_name).cloned(),
                path: port.port_name,
            })
            .collect())
    }
}

impl CameraDiscoveryPort for Arc<CameraManager> {
    fn cameras(&self) -> Vec<DiscoveredCamera> {
        self.statuses().into_iter().map(discovered_camera).collect()
    }

    async fn refresh_cameras(&self) -> Result<Vec<DiscoveredCamera>, ApplicationError> {
        let cameras = self.clone();
        tokio::task::spawn_blocking(move || cameras.refresh_blocking())
            .await
            .map_err(|error| ApplicationError::Internal(error.to_string()))?;
        Ok(self.statuses().into_iter().map(discovered_camera).collect())
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
}

impl<'a> RobotHttpAdapter<'a> {
    pub(super) fn new(state: &'a AppState) -> Self {
        Self {
            state,
            import_document: None,
        }
    }

    fn import(state: &'a AppState, document: Bytes) -> Self {
        Self {
            state,
            import_document: Some(document),
        }
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
        self.state
            .workspace
            .list()
            .into_iter()
            .map(|record| record.id)
            .collect()
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

impl RobotControlPort for HardwareHandle {
    async fn hold(&self, robot: &str) -> Result<(), String> {
        HardwareHandle::hold(self, robot).await
    }

    async fn release(&self, robot: &str) -> Result<(), String> {
        HardwareHandle::release(self, robot).await
    }

    async fn set_joint_goals(
        &self,
        robot: &str,
        normalized: std::collections::BTreeMap<
            String,
            spiral_robotics_core::calibration::NormalizedPosition,
        >,
    ) -> Result<(), String> {
        HardwareHandle::set_goals(
            self,
            robot,
            normalized
                .into_iter()
                .map(|(joint, value)| (joint, value.get()))
                .collect(),
        )
        .await
    }
}

impl ModeControlPort for HardwareHandle {
    async fn set_operator_mode(&self, mode: OperatorMode) -> Result<(), ApplicationError> {
        let selection = match mode {
            OperatorMode::Idle => ModeSelection::Idle,
            OperatorMode::Manual { robot } => ModeSelection::Manual { robot },
            OperatorMode::Teleoperate { leader, follower } => {
                ModeSelection::Teleoperate { leader, follower }
            }
        };
        HardwareHandle::set_mode(self, selection)
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

impl CalibrationControlPort for HardwareHandle {
    async fn start_calibration(&self, robot: &str) -> Result<(), ApplicationError> {
        HardwareHandle::start_calibration(self, robot)
            .await
            .map_err(ApplicationError::conflict)
    }

    async fn calibration_input(&self, robot: &str) -> Result<CalibrationInput, ApplicationError> {
        HardwareHandle::calibration_input(self, robot)
            .await
            .map_err(ApplicationError::conflict)
    }

    async fn cancel_calibration(&self, robot: &str) -> Result<(), ApplicationError> {
        HardwareHandle::cancel_calibration(self, robot)
            .await
            .map_err(ApplicationError::conflict)
    }

    async fn apply_calibration(
        &self,
        robot: &str,
        calibration: Calibration,
    ) -> Result<(), ApplicationError> {
        HardwareHandle::apply_calibration(self, robot, calibration)
            .await
            .map_err(ApplicationError::conflict)
    }
}

async fn list_ports(
    State(state): State<AppState>,
) -> Result<Json<Vec<SerialPortSummary>>, Failure> {
    Ok(Json(
        device_discovery::list_serial_ports(&SerialPortQueryAdapter { state: &state })
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
    let result = device_discovery::identify_serial_port(
        &SystemSerialPortInventory,
        IdentifySerialPortCommand {
            before: request.before,
        },
    )
    .await?;
    Ok(Json(IdentifyPortResponse {
        port: result.port,
        removed: result.removed,
        message: result.guidance,
    }))
}

async fn list_cameras(State(state): State<AppState>) -> Json<Vec<CameraStatus>> {
    Json(
        device_discovery::list_cameras(&state.cameras)
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
        device_discovery::refresh_cameras(&state.cameras)
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
    let adapter = RobotHttpAdapter::new(&state);
    Json(
        robots::list(&adapter)
            .into_iter()
            .filter_map(|robot| present_saved_robot(&state, robot).ok())
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

async fn set_mode(
    State(state): State<AppState>,
    Json(selection): Json<ModeSelection>,
) -> Result<StatusCode, Failure> {
    // Replay carries a trajectory that has to be read off disk, so it has its
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
