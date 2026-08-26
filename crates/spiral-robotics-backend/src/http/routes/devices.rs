use super::*;
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease};
use spiral_robotics_storage::episodes::ReappliedEpisodePositions;

/// Camera discovery and preview do not depend on SpiralDB, so they remain
/// useful while catalog bootstrap is still running or needs repair.
impl AppState {
    pub(super) fn device_bootstrap_router() -> Router<Self> {
        Router::new()
            .route("/cameras/refresh", post(Self::refresh_cameras))
            .route("/cameras/stream.mjpeg", get(Self::camera_stream))
    }

    pub(super) fn device_catalog_router() -> Router<Self> {
        Router::new()
            .route("/ports", get(Self::list_ports))
            .route("/ports/identify", post(Self::identify_port))
            .route("/robots", get(Self::list_robots).post(Self::create_robot))
            .route(
                "/robots/import",
                post(Self::import_robot_configuration).layer(DefaultBodyLimit::max(1024 * 1024)),
            )
            .route(
                "/robots/{id}",
                get(Self::get_robot)
                    .patch(Self::update_robot)
                    .delete(Self::delete_robot),
            )
            .route("/robots/{id}/connect", post(Self::connect_robot))
            .route("/robots/{id}/disconnect", post(Self::disconnect_robot))
            .route("/robots/{id}/hold", post(Self::hold_robot))
            .route("/robots/{id}/release", post(Self::release_robot))
            .route("/robots/{id}/goal", post(Self::set_robot_goal))
            .route(
                "/robots/{id}/calibration/start",
                post(Self::start_calibration),
            )
            .route(
                "/robots/{id}/calibration/finish",
                post(Self::finish_calibration),
            )
            .route(
                "/robots/{id}/calibration/cancel",
                post(Self::cancel_calibration),
            )
            .route(
                "/robots/{id}/calibration/reapply",
                post(Self::reapply_calibration),
            )
            .route("/mode", post(Self::set_mode))
    }
}

struct SystemSerialPortInventory;

impl SerialPortInventory for SystemSerialPortInventory {
    async fn available_port_paths(&self) -> Result<Vec<String>, OperationError> {
        tokio::task::spawn_blocking(SerialPortCatalog::discover)
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?
            .map(|ports| {
                ports
                    .into_ports()
                    .into_iter()
                    .map(|port| port.port_name)
                    .collect()
            })
            .map_err(|error| OperationError::Internal(error.to_string()))
    }
}

struct SerialPortQueryAdapter<'a> {
    state: &'a AppState,
}

impl SerialPortQueryPort for SerialPortQueryAdapter<'_> {
    async fn serial_ports(&self) -> Result<Vec<SerialPortSummary>, OperationError> {
        let ports = tokio::task::spawn_blocking(SerialPortCatalog::discover)
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?
            .map_err(|error| {
                OperationError::Internal(format!("could not list serial ports: {error}"))
            })?;
        let hardware = self.state.hardware().state();
        let in_use = self
            .state
            .workspace()
            .list()
            .into_iter()
            .filter(|record| {
                hardware
                    .connections()
                    .get(record.id())
                    .is_some_and(|connection| connection.connected())
            })
            .filter_map(|record| Some((record.port()?.path().to_owned(), record.id().to_owned())))
            .collect::<std::collections::BTreeMap<_, _>>();
        Ok(ports
            .into_ports()
            .into_iter()
            .map(|port| {
                SerialPortSummary::new(
                    port.port_name.clone(),
                    port.port_type.kind(),
                    port.port_type.description(),
                )
                .with_owner(in_use.get(&port.port_name).cloned())
            })
            .collect())
    }
}

#[derive(Clone)]
pub(super) struct CameraDiscoveryAdapter(Arc<CameraManager>);

impl CameraDiscoveryAdapter {
    pub(super) fn new(camera_manager: Arc<CameraManager>) -> Self {
        Self(camera_manager)
    }
}

impl CameraDiscoveryPort for CameraDiscoveryAdapter {
    fn cameras(&self) -> Vec<CameraStatus> {
        self.0.statuses()
    }

    async fn refresh_cameras(&self) -> Result<Vec<CameraStatus>, OperationError> {
        let cameras = self.0.clone();
        tokio::task::spawn_blocking(move || cameras.refresh_blocking())
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?;
        Ok(self.0.statuses())
    }
}

/// Adapts cached JSON robot documents and the live control thread to plain
/// capability commands. JSON preservation, wire summaries, and concrete
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
            saved_records: Some(state.workspace().list()),
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
                .find(|record| record.id() == robot)
                .cloned()
                .ok_or_else(|| WorkspaceError::NotFound(robot.to_owned()))?,
            None => self.state.workspace().get(robot)?,
        };
        Ok(record.summary(connection))
    }
}

impl RobotStore for RobotHttpAdapter<'_> {
    type CameraConfiguration = Map<String, Value>;

    fn create(&self, robot: NewRobot) -> Result<SavedRobot, OperationError> {
        let (id, robot_type, port) = robot.into_parts();
        let port = port.map(|port| PortSettings::new(port.path(), port.baud_rate()));
        let record = self
            .state
            .workspace()
            .create(
                crate::robots::workspace::RobotWorkspaceCreate::new(id, robot_type).with_port(port),
            )
            .map_err(OperationError::from)?;
        Ok(SavedRobot::new(record.id()))
    }

    fn import_configuration(&self) -> Result<SavedRobot, OperationError> {
        let configuration = self
            .import_document
            .as_ref()
            .ok_or_else(|| OperationError::Internal("missing robot import document".to_owned()))?;
        let record = self
            .state
            .workspace()
            .import_configuration(configuration)
            .map_err(OperationError::from)?;
        Ok(SavedRobot::new(record.id()))
    }

    fn camera_configuration(
        &self,
        robot: &str,
        assignments: &[CameraAssignment],
    ) -> Result<Self::CameraConfiguration, OperationError> {
        let record = self
            .state
            .workspace()
            .get(robot)
            .map_err(OperationError::from)?;
        Ok(record
            .camera_object()
            .with_camera_assignments(assignments, &self.state.cameras().statuses()))
    }

    fn update(
        &self,
        command: RobotUpdate<Map<String, Value>>,
    ) -> Result<SavedRobot, OperationError> {
        let (id, robot_type, port, cameras) = command.into_parts();
        let port = port.map(|port| PortSettings::new(port.path(), port.baud_rate()));
        let record = self
            .state
            .workspace()
            .update(
                crate::robots::workspace::RobotWorkspaceUpdate::new(id, robot_type)
                    .with_port(port)
                    .with_cameras(cameras),
            )
            .map_err(OperationError::from)?;
        Ok(SavedRobot::new(record.id()))
    }

    fn ensure_exists(&self, robot: &str) -> Result<(), OperationError> {
        self.state
            .workspace()
            .get(robot)
            .map(|_| ())
            .map_err(OperationError::from)
    }

    fn delete(&self, robot: &str) -> Result<(), OperationError> {
        self.state
            .workspace()
            .delete(robot)
            .map_err(OperationError::from)
    }

    fn saved_robot_ids(&self) -> Vec<String> {
        match &self.saved_records {
            Some(records) => records
                .iter()
                .map(|record| record.id().to_owned())
                .collect(),
            None => self
                .state
                .workspace()
                .list()
                .into_iter()
                .map(|record| record.id().to_owned())
                .collect(),
        }
    }

    fn connection_spec(&self, robot: &str) -> Result<RobotConnectionSpec, OperationError> {
        let record = self
            .state
            .workspace()
            .get(robot)
            .map_err(OperationError::from)?;
        let port = record.port().ok_or_else(|| {
            OperationError::InvalidInput(format!(
                "robot {:?} has no serial port assigned; set one before connecting",
                record.id()
            ))
        })?;
        let motors = record.motors().map_err(OperationError::from)?;
        let calibration = record.calibration().map_err(OperationError::from)?;
        Ok(RobotConnectionSpec::new(
            record.id(),
            SerialPortConfiguration::new(port.path(), port.baud_rate()),
            motors,
        )
        .with_calibration(calibration))
    }
}

impl RobotConnectionPort for RobotHttpAdapter<'_> {
    fn is_connected(&self, robot: &str) -> bool {
        self.state
            .hardware()
            .state()
            .connections()
            .get(robot)
            .is_some_and(|connection| connection.connected())
    }

    async fn connect(&self, specification: RobotConnectionSpec) -> Result<(), OperationError> {
        let (id, port, motors, calibration) = specification.into_parts();
        self.state
            .hardware()
            .connect(
                RobotSpec::new(id, PortSettings::new(port.path(), port.baud_rate()), motors)
                    .with_calibration(calibration),
            )
            .await
            .map_err(OperationError::conflict)
    }

    async fn disconnect(&self, robot: &str) -> Result<(), OperationError> {
        self.state
            .hardware()
            .disconnect(robot)
            .await
            .map_err(OperationError::conflict)
    }

    async fn forget(&self, robot: &str) -> Result<(), OperationError> {
        self.state
            .hardware()
            .forget(robot)
            .await
            .map_err(OperationError::conflict)
    }
}

impl RobotCalibrationStore for RobotWorkspace {
    fn motor_specs(&self, robot: &str) -> Result<BTreeMap<String, MotorSpec>, OperationError> {
        self.get(robot)
            .and_then(|record| record.motors())
            .map_err(OperationError::from)
    }

    fn save_calibration(
        &self,
        robot: &str,
        update: CalibrationUpdate<'_>,
    ) -> Result<(), OperationError> {
        RobotWorkspace::save_calibration(self, robot, update)
            .map(|_| ())
            .map_err(OperationError::from)
    }
}

struct CalibrationHistoryHttpAdapter<'a> {
    state: &'a AppState,
}

impl CalibrationHistoryPort for CalibrationHistoryHttpAdapter<'_> {
    fn reserve_catalog_mutation(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
        self.state.reserve_calibration_history_mutation()
    }

    fn release_catalog_mutation(&self, token: CatalogLeaseToken) {
        let released = self.state.catalog_lease().release(token);
        debug_assert!(
            released,
            "calibration history refresh releases its exact catalog lease once"
        );
    }

    fn activity(&self) -> CalibrationHistoryActivity {
        let hardware = self.state.hardware().state();
        CalibrationHistoryActivity::new(
            hardware
                .telemetry()
                .recording()
                .as_ref()
                .map(|recording| recording.episode().to_owned()),
            match &hardware.mode().selection() {
                ModeSelection::Replay { episode, .. } => Some(episode.clone()),
                _ => None,
            },
            self.state.training().snapshot().runs().iter().any(|run| {
                matches!(
                    run.phase(),
                    spiral_robotics_api::training::TrainingRunPhase::Starting
                        | spiral_robotics_api::training::TrainingRunPhase::Running
                        | spiral_robotics_api::training::TrainingRunPhase::Stopping
                )
            }),
        )
    }

    fn current_calibration(&self, robot: &str) -> Result<Option<Calibration>, OperationError> {
        let record = self
            .state
            .workspace()
            .get(robot)
            .map_err(OperationError::from)?;
        let motors = record.motors().map_err(OperationError::from)?;
        let calibration = record.calibration().map_err(OperationError::from)?;
        if let Some(calibration) = &calibration {
            calibration
                .validate_for_motor_specs(&motors)
            .map_err(|error| {
                OperationError::InvalidInput(format!(
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
    ) -> Result<ReappliedEpisodePositions, OperationError> {
        let episodes = self.state.episodes().clone();
        tokio::task::spawn_blocking(move || episodes.reapply_calibration(&robot, &calibration))
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?
            .map_err(OperationError::from)
    }
}

impl AppState {
    fn reserve_calibration_history_mutation(
        &self,
    ) -> Result<Option<CatalogLeaseToken>, OperationError> {
        self.catalog_lease().reserve_calibration_history(
            self.workflow().is_active(),
            self.workflow_agent().is_active(),
        )
    }

    /// Try every usable saved robot once when the studio starts.
    ///
    /// A failed bus stays disconnected with its error attached; one absent arm
    /// never prevents the web interface or another arm from starting.
    pub(crate) async fn auto_connect_saved_robots(&self) {
        let adapter = RobotHttpAdapter::new(self);
        for outcome in RobotActions::new(&adapter, &adapter).auto_connect().await {
            outcome.log();
        }
    }
}

impl CatalogMutationLease {
    fn reserve_calibration_history(
        &self,
        workflow_active: bool,
        assistant_active: bool,
    ) -> Result<Option<CatalogLeaseToken>, OperationError> {
        let Some(token) = self.try_acquire(CatalogLeaseOwner::CalibrationReapply) else {
            return Ok(None);
        };
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
            let released = self.release(token);
            debug_assert!(
                released,
                "rejected calibration-history admission releases its exact catalog lease"
            );
            return Err(OperationError::Conflict(message.to_owned()));
        }
        Ok(Some(token))
    }
}

impl AppState {
    async fn list_ports(
        State(state): State<Self>,
    ) -> Result<Json<Vec<SerialPortSummary>>, Failure> {
        Ok(Json(
            SerialPortQueryAdapter { state: &state }.serial_ports().await?,
        ))
    }

    async fn identify_port(
        Json(request): Json<IdentifyPortRequest>,
    ) -> Result<Json<IdentifyPortResponse>, Failure> {
        Ok(Json(
            IdentifySerialPortCommand::new(request.before().to_vec())
                .execute(&SystemSerialPortInventory)
                .await?,
        ))
    }

    async fn refresh_cameras(
        State(state): State<Self>,
    ) -> Result<Json<Vec<CameraStatus>>, Failure> {
        Ok(Json(
            CameraDiscoveryAdapter::new(state.cameras().clone())
                .refresh_cameras()
                .await?,
        ))
    }
}

/// A camera is addressed by query string because its identifier may be a device
/// path containing slashes.
#[derive(Debug, Deserialize)]
struct CameraQuery {
    id: String,
}

impl AppState {
    async fn camera_stream(
        State(state): State<Self>,
        Query(query): Query<CameraQuery>,
    ) -> Result<Response, Failure> {
        let receiver = state
            .cameras()
            .subscribe(&query.id)
            .map_err(|error| Failure(StatusCode::NOT_FOUND, error))?;

        let frames = tokio_stream::wrappers::WatchStream::new(receiver)
            .filter_map(|frame| async move { frame })
            .scan(0_u64, |last, frame| {
                let fresh = frame.sequence() != *last;
                *last = frame.sequence();
                futures_util::future::ready(Some(fresh.then_some(frame)))
            })
            .filter_map(|frame| async move { frame })
            .map(|frame| Ok::<Bytes, Infallible>(frame.multipart_part()));

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

}

impl Frame {
    /// Wrap this JPEG as a part of a `multipart/x-mixed-replace` response.
    pub(super) fn multipart_part(&self) -> Bytes {
        let mut part = BytesMut::with_capacity(self.bytes().len() + 96);
        part.put_slice(b"--");
        part.put_slice(MJPEG_BOUNDARY.as_bytes());
        part.put_slice(b"\r\nContent-Type: image/jpeg\r\nContent-Length: ");
        part.put_slice(self.bytes().len().to_string().as_bytes());
        part.put_slice(b"\r\n\r\n");
        part.put_slice(self.bytes());
        part.put_slice(b"\r\n");
        part.freeze()
    }
}

impl AppState {
    async fn list_robots(State(state): State<Self>) -> Json<Vec<RobotSummary>> {
        let adapter = RobotHttpAdapter::prefetched(&state);
        let hardware = state.hardware().state();
        Json(
            RobotCatalogActions::new(&adapter)
                .list()
                .into_iter()
                .filter_map(|robot| {
                    let connection = hardware
                        .connections()
                        .get(robot.id())
                        .cloned()
                        .unwrap_or_default();
                    adapter.summary(robot.id(), connection).ok()
                })
                .collect(),
        )
    }

    pub(super) fn present_saved_robot(&self, robot: SavedRobot) -> Result<RobotSummary, Failure> {
        let connection = self
            .hardware()
            .state()
            .connections()
            .get(robot.id())
            .cloned()
            .unwrap_or_default();
        Ok(self.workspace().get(robot.id())?.summary(connection))
    }

    async fn create_robot(
        State(state): State<Self>,
        Json(request): Json<CreateRobotRequest>,
    ) -> Result<(StatusCode, Json<RobotSummary>), Failure> {
        let adapter = RobotHttpAdapter::new(&state);
        let robot = RobotCatalogActions::new(&adapter).create(
            CreateRobotCommand::new(
                request.id(),
                request.robot_type().map(str::to_owned),
                request.port().map(str::to_owned),
            )
            .with_baud_rate(request.baud_rate()),
        )?;
        RobotActions::new(&adapter, &adapter)
            .auto_connect_robot(robot.id().to_owned())
            .await
            .log();
        Ok((StatusCode::CREATED, Json(state.present_saved_robot(robot)?)))
    }

    /// Add a complete unified robot JSON document as a new workspace instance.
    async fn import_robot_configuration(
        State(state): State<Self>,
        body: Bytes,
    ) -> Result<(StatusCode, Json<RobotSummary>), Failure> {
        if body.is_empty() {
            return Err(Failure::bad_request(
                "choose a robot configuration JSON file to import",
            ));
        }
        let adapter = RobotHttpAdapter::import(&state, body);
        let robot = RobotCatalogActions::new(&adapter).import_configuration()?;
        RobotActions::new(&adapter, &adapter)
            .auto_connect_robot(robot.id().to_owned())
            .await
            .log();
        Ok((StatusCode::CREATED, Json(state.present_saved_robot(robot)?)))
    }

    async fn get_robot(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<Json<RobotSummary>, Failure> {
        let robot = RobotCatalogActions::new(&RobotHttpAdapter::new(&state)).get(id)?;
        let connection = state
            .hardware()
            .state()
            .connections()
            .get(robot.id())
            .cloned()
            .unwrap_or_default();
        Ok(Json(state.workspace().get(robot.id())?.summary(connection)))
    }

    async fn update_robot(
        State(state): State<Self>,
        Path(id): Path<String>,
        Json(request): Json<UpdateRobotRequest>,
    ) -> Result<Json<RobotSummary>, Failure> {
        let adapter = RobotHttpAdapter::new(&state);
        let should_auto_connect = request.port().is_some();
        let robot = RobotActions::new(&adapter, &adapter).update(
            UpdateRobotCommand::new(
                id.clone(),
                request.robot_type().map(str::to_owned),
                request
                    .port()
                    .map(|port| SerialPortConfiguration::new(port.path(), port.baud_rate())),
            )
            .with_cameras(request.cameras().map(|bindings| {
                bindings
                    .iter()
                    .map(|binding| CameraAssignment::new(binding.name(), binding.id()))
                    .collect()
            })),
        )?;
        if should_auto_connect {
            RobotActions::new(&adapter, &adapter)
                .auto_connect_robot(robot.id().to_owned())
                .await
                .log();
        }
        Ok(Json(state.present_saved_robot(robot)?))
    }
}

/// Merge camera assignments into the robot's stored `cameras` object.
///
/// Fields this build does not understand are kept for cameras that keep their
/// role name, and the resolution of a known device is refreshed so the file
/// records what will actually be captured.
pub(super) trait CameraConfigurationObject {
    fn with_camera_assignments(
        &self,
        assignments: &[CameraAssignment],
        discovered: &[CameraStatus],
    ) -> Map<String, Value>;
}

impl CameraConfigurationObject for Map<String, Value> {
    fn with_camera_assignments(
        &self,
        assignments: &[CameraAssignment],
        discovered: &[CameraStatus],
    ) -> Map<String, Value> {
        let existing = self;
        let mut cameras = Self::new();

        for assignment in assignments {
            let mut fields = match existing.get(assignment.role()) {
                Some(Value::Object(fields)) => fields.clone(),
                _ => Map::new(),
            };
            fields
                .entry("type".to_owned())
                .or_insert_with(|| Value::String("opencv".to_owned()));
            fields.insert(
                "id".to_owned(),
                Value::String(assignment.device_id().to_owned()),
            );
            if let Some(camera) = discovered
                .iter()
                .find(|camera| camera.id() == assignment.device_id())
            {
                fields.insert("width".to_owned(), Value::from(camera.width()));
                fields.insert("height".to_owned(), Value::from(camera.height()));
                fields.insert("fps".to_owned(), Value::from(camera.fps()));
            }
            cameras.insert(assignment.role().to_owned(), Value::Object(fields));
        }

        cameras
    }
}

impl AppState {
    async fn delete_robot(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<StatusCode, Failure> {
        let adapter = RobotHttpAdapter::new(&state);
        RobotActions::new(&adapter, &adapter).delete(&id).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn connect_robot(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<StatusCode, Failure> {
        let adapter = RobotHttpAdapter::new(&state);
        RobotActions::new(&adapter, &adapter).connect(&id).await?;
        Ok(StatusCode::NO_CONTENT)
    }
}

impl AutoConnectOutcome {
    fn log(self) {
        let (robot, status) = self.into_parts();
        match status {
            AutoConnectStatus::Connected => {
                tracing::info!(robot = %robot, "auto-connected saved robot")
            }
            AutoConnectStatus::Skipped(error) => {
                tracing::info!(robot = %robot, "saved robot is not ready for auto-connect: {error}")
            }
            AutoConnectStatus::Failed(error) => {
                tracing::warn!(robot = %robot, "auto-connect failed: {error}")
            }
        }
    }
}

impl AppState {
    async fn disconnect_robot(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<StatusCode, Failure> {
        let adapter = RobotHttpAdapter::new(&state);
        RobotConnectionActions::new(&adapter)
            .disconnect(&id)
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn hold_robot(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<StatusCode, Failure> {
        RobotControlActions::new(state.hardware().as_ref())
            .execute(RobotControlCommand::Hold { robot: id })
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn release_robot(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<StatusCode, Failure> {
        RobotControlActions::new(state.hardware().as_ref())
            .execute(RobotControlCommand::Release { robot: id })
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn set_robot_goal(
        State(state): State<Self>,
        Path(id): Path<String>,
        Json(request): Json<GoalRequest>,
    ) -> Result<StatusCode, Failure> {
        RobotControlActions::new(state.hardware().as_ref())
            .execute(RobotControlCommand::MoveJoints {
                robot: id,
                normalized: request.normalized().clone(),
            })
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn start_calibration(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<StatusCode, Failure> {
        CalibrationControlActions::new(state.hardware().as_ref())
            .start(&id)
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn finish_calibration(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<Json<RobotSummary>, Failure> {
        let output = CalibrationActions::new(state.workspace().as_ref(), state.hardware().as_ref())
            .finish(&id)
            .await?;
        if let Some(warning) = output.live_warning() {
            tracing::warn!(robot = %id, "saved calibration but live refresh was incomplete: {warning}");
        }

        let record = state.workspace().get(&id)?;
        let connection = state
            .hardware()
            .state()
            .connections()
            .get(&id)
            .cloned()
            .unwrap_or_default();
        Ok(Json(record.summary(connection)))
    }

    async fn cancel_calibration(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<StatusCode, Failure> {
        CalibrationControlActions::new(state.hardware().as_ref())
            .cancel(&id)
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn reapply_calibration(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<Json<CalibrationReapplyResponse>, Failure> {
        Ok(Json(
            CalibrationHistoryActions::new(&CalibrationHistoryHttpAdapter { state: &state })
                .reapply(&id)
                .await?,
        ))
    }

    async fn set_mode(
        State(state): State<Self>,
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
        ModeActions::new(state.hardware().as_ref())
            .set(mode)
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }
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
            let error = lease
                .reserve_calibration_history(activity.0, activity.1)
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
        let calibration = lease
            .reserve_calibration_history(false, false)
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
