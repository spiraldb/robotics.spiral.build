use super::*;

struct CatalogReservation {
    lease: Arc<CatalogMutationLease>,
    token: CatalogLeaseToken,
}

impl Drop for CatalogReservation {
    fn drop(&mut self) {
        let released = self.lease.release(self.token);
        debug_assert!(released, "catalog transfer releases its exact lease once");
    }
}

/// Bootstrap settings remain available even when the configured catalog does
/// not, so an operator can repair or select the Spiral destination.
pub(super) fn bootstrap_router() -> Router<AppState> {
    Router::new()
        .route(
            "/settings/data",
            get(get_data_settings).patch(update_data_settings),
        )
        .route("/settings/data/test/spiral", post(test_spiral))
        .route("/settings/data/test/training", post(test_training_runtime))
}

pub(super) fn catalog_router() -> Router<AppState> {
    Router::new()
        .route("/spiral/query", post(spiral_query))
        .route("/spiral/replay", post(start_spiral_replay))
        .route("/spiral/video.mp4", get(spiral_video))
        .route("/project/export", get(export_project))
        .route(
            "/project/import",
            post(import_project).layer(DefaultBodyLimit::max(
                crate::backend::storage::bundle::MAX_PROJECT_BUNDLE_BYTES,
            )),
        )
        .route("/episodes/export", get(export_episodes))
        .route(
            "/episodes/import",
            post(import_episodes).layer(DefaultBodyLimit::max(
                crate::backend::storage::bundle::MAX_EPISODE_BUNDLE_BYTES,
            )),
        )
}

#[derive(Debug, Deserialize)]
struct EpisodeTransferQuery {
    dataset_id: String,
}

async fn export_project(State(state): State<AppState>) -> Result<Response, Failure> {
    let storage = state.storage.clone();
    let bytes = tokio::task::spawn_blocking(move || storage.export_project_bundle())
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "application/vnd.spiral-robotics.project",
        )
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=spiral-robotics-project.spiral-project",
        )
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from(bytes))
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

async fn import_project(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<spiral_robotics_contracts::api::ProjectImportSummary>, Failure> {
    let token = state
        .catalog_lease
        .try_acquire(CatalogLeaseOwner::ProjectImport)
        .ok_or_else(|| {
            Failure(
                StatusCode::CONFLICT,
                "Wait for the current recording or catalog change before importing a project"
                    .to_owned(),
            )
        })?;
    let _reservation = CatalogReservation {
        lease: state.catalog_lease.clone(),
        token,
    };
    if body.is_empty() {
        return Err(Failure::bad_request(
            "choose a .spiral-project file to import",
        ));
    }
    require_project_import_idle(&state)?;
    disconnect_all_robots(&state).await?;
    let storage = state.storage.clone();
    let summary = tokio::task::spawn_blocking(move || storage.import_project_bundle(&body))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .map_err(Failure::import)?;
    super::auto_connect_saved_robots(state.clone()).await;
    state.preferences.replace(
        state
            .storage
            .load_studio_preferences()
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?,
    );
    state
        .training
        .reload_from_storage()
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?;
    state.training.refresh_readiness().await;
    state
        .rollout
        .reload_from_storage()
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?;
    Ok(Json(summary))
}

async fn export_episodes(
    State(state): State<AppState>,
    Query(query): Query<EpisodeTransferQuery>,
) -> Result<Response, Failure> {
    if !spiral_robotics_contracts::spiral::is_dataset_id(&query.dataset_id) {
        return Err(Failure::bad_request(
            "dataset ID must contain 1–96 letters, numbers, '.', '_' or '-'",
        ));
    }
    let dataset_id = query.dataset_id;
    let filename = format!("{dataset_id}.spiral-episodes");
    let storage = state.storage.clone();
    let bytes = tokio::task::spawn_blocking(move || storage.export_episode_bundle(&dataset_id))
        .await
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .map_err(Failure::bad_request)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "application/vnd.spiral-robotics.episodes",
        )
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from(bytes))
        .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

async fn import_episodes(
    State(state): State<AppState>,
    Query(query): Query<EpisodeTransferQuery>,
    body: Bytes,
) -> Result<Json<spiral_robotics_contracts::api::EpisodeImportSummary>, Failure> {
    if !spiral_robotics_contracts::spiral::is_dataset_id(&query.dataset_id) {
        return Err(Failure::bad_request(
            "dataset ID must contain 1–96 letters, numbers, '.', '_' or '-'",
        ));
    }
    let token = state
        .catalog_lease
        .try_acquire(CatalogLeaseOwner::EpisodeImport)
        .ok_or_else(|| {
            Failure::conflict(
                "Wait for the current recording or catalog change before importing episodes",
            )
        })?;
    let _reservation = CatalogReservation {
        lease: state.catalog_lease.clone(),
        token,
    };
    require_import_idle(&state, "episodes")?;
    if body.is_empty() {
        return Err(Failure::bad_request(
            "choose a .spiral-episodes file to import",
        ));
    }
    let storage = state.storage.clone();
    let dataset_id = query.dataset_id;
    let summary =
        tokio::task::spawn_blocking(move || storage.import_episode_bundle(&dataset_id, &body))
            .await
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
            .map_err(Failure::import)?;
    Ok(Json(summary))
}

fn require_import_idle(state: &AppState, subject: &str) -> Result<(), Failure> {
    let hardware = state.hardware.state();
    let connected = hardware
        .connections
        .values()
        .any(|connection| connection.connected);
    if import_activity_is_busy(state, &hardware) || connected {
        return Err(Failure::conflict(format!(
            "Disconnect robots and stop recording, replay, rollout, and training before importing {subject}"
        )));
    }
    Ok(())
}

fn require_project_import_idle(state: &AppState) -> Result<(), Failure> {
    let hardware = state.hardware.state();
    if import_activity_is_busy(state, &hardware) {
        return Err(Failure::conflict(
            "Stop recording, replay, rollout, and training before importing a project",
        ));
    }
    Ok(())
}

fn import_activity_is_busy(state: &AppState, hardware: &HardwareState) -> bool {
    hardware.telemetry.recording.is_some()
        || !matches!(hardware.mode.selection, ModeSelection::Idle)
        || state
            .training
            .snapshot()
            .runs
            .iter()
            .any(|run| run.phase.active())
        || matches!(
            rollout_application::snapshot(&super::training_rollout::RolloutHttpAdapter(
                state.rollout.clone(),
            ))
            .phase,
            ApplicationRolloutPhase::Loading
                | ApplicationRolloutPhase::WarmingUp
                | ApplicationRolloutPhase::Running
                | ApplicationRolloutPhase::Stopping
        )
}

async fn disconnect_all_robots(state: &AppState) -> Result<(), Failure> {
    let robot_ids = state
        .hardware
        .state()
        .connections
        .into_iter()
        .filter_map(|(robot, connection)| connection.connected.then_some(robot))
        .collect::<Vec<_>>();
    for robot in robot_ids {
        state.hardware.disconnect(&robot).await.map_err(|error| {
            Failure::conflict(format!(
                "Could not safely disconnect robot {robot:?} before importing the project: {error}"
            ))
        })?;
    }
    Ok(())
}

pub(super) struct DataHttpAdapter<'a> {
    state: &'a AppState,
}

impl DataCatalogPort for DataHttpAdapter<'_> {
    fn settings(&self) -> DataSettingsView {
        let configured_endpoint = self.state.backend_state.spiral_endpoint();
        let configured_spfs_endpoint = self.state.backend_state.spiral_spfs_endpoint();
        let configured_project = self.state.backend_state.project_id();
        let configured_client_id = self.state.backend_state.client_id();
        let credentials_configured = self.state.backend_state.credentials().is_some();
        application_data_settings(
            self.state
                .storage
                .data_settings(&self.state.preferences.get()),
            configured_endpoint.as_deref(),
            configured_spfs_endpoint.as_deref(),
            configured_project.as_deref(),
            (!configured_client_id.is_empty()).then_some(configured_client_id.as_str()),
            credentials_configured,
        )
    }

    fn update_settings(
        &self,
        command: UpdateDataSettingsCommand,
    ) -> Result<DataSettingsView, ApplicationError> {
        let catalog_was_available = self.state.bootstrap.current().catalog_available;
        let endpoint_changed = self.state.storage.spiral_endpoint() != command.spiral_endpoint
            || self.state.storage.spiral_spfs_endpoint() != command.spiral_spfs_endpoint;
        let project_changed = self.state.storage.project_id() != command.project_id;
        let current_credentials = self.state.backend_state.credentials();
        let credentials = match command.client_secret.as_deref() {
            Some("") => None,
            Some(secret) => Some(crate::backend::storage::settings::SpiralCredentials {
                client_id: command.client_id.clone(),
                client_secret: secret.to_owned(),
            }),
            None if command.client_id.is_empty() => None,
            None => current_credentials
                .clone()
                .filter(|credentials| credentials.client_id == command.client_id)
                .ok_or_else(|| {
                    ApplicationError::InvalidInput(
                        "Enter the client secret when changing the Spiral client ID".to_owned(),
                    )
                })
                .map(Some)?,
        };
        let credentials_changed = current_credentials != credentials;
        let staged = self
            .state
            .preferences
            .stage_update(
                spiral_robotics_contracts::api::TrainingRuntimeSettings {
                    endpoint: command.training_endpoint,
                },
                VideoSettings {
                    enabled: command.video_enabled,
                },
            )
            .map_err(ApplicationError::InvalidInput)?;
        self.state
            .backend_state
            .save_connection(
                &command.project_id,
                &command.spiral_endpoint,
                &command.spiral_spfs_endpoint,
                credentials.as_ref(),
            )
            .map_err(ApplicationError::Internal)?;
        if endpoint_changed || project_changed || credentials_changed {
            self.state
                .storage
                .configure_connection(
                    command.spiral_endpoint.clone(),
                    command.spiral_spfs_endpoint.clone(),
                    &command.project_id,
                    credentials,
                )
                .map_err(ApplicationError::InvalidInput)?;
        }
        self.state
            .storage
            .save_studio_preferences(&staged)
            .map_err(ApplicationError::External)?;
        if endpoint_changed || project_changed || credentials_changed || !catalog_was_available {
            self.state
                .training
                .reload_from_storage()
                .map_err(ApplicationError::External)?;
            self.state
                .rollout
                .reload_from_storage()
                .map_err(ApplicationError::External)?;
        }
        self.state.preferences.replace(staged);
        Ok(self.settings())
    }

    async fn refresh_training_readiness(&self) {
        self.state.training.refresh_readiness().await;
    }

    async fn test_connection(&self) -> ConnectionCheck {
        let response = self.state.storage.test_spiral().await;
        ConnectionCheck {
            ok: response.ok,
            message: response.message,
        }
    }

    async fn query(
        &self,
        command: QueryCommand,
    ) -> Result<ApplicationQueryResult, ApplicationError> {
        self.state
            .storage
            .query(&command.statement, command.max_rows)
            .await
            .map(application_query_result)
            .map_err(ApplicationError::External)
    }

    async fn load_replay_trajectory(
        &self,
        episode: &str,
        stream: &str,
    ) -> Result<SpiralReplayTrajectory, ApplicationError> {
        let (sample_hz, frames) = self
            .state
            .storage
            .trajectory(episode, stream)
            .await
            .map_err(ApplicationError::External)?;
        Ok(SpiralReplayTrajectory { sample_hz, frames })
    }

    async fn video_asset(&self, query: VideoAssetQuery) -> Result<Vec<u8>, ApplicationError> {
        self.state
            .storage
            .video_asset(&query.episode, &query.stream, query.segment)
            .await
            .map_err(ApplicationError::External)
    }
}

pub(super) fn application_data_settings(
    mut settings: DataSettings,
    configured_endpoint: Option<&str>,
    configured_spfs_endpoint: Option<&str>,
    configured_project: Option<&str>,
    configured_client_id: Option<&str>,
    credentials_configured: bool,
) -> DataSettingsView {
    if let Some(configured_endpoint) = configured_endpoint {
        settings.spiral.endpoint = configured_endpoint.to_owned();
    }
    if let Some(configured_spfs_endpoint) = configured_spfs_endpoint {
        settings.spiral.spfs_endpoint = configured_spfs_endpoint.to_owned();
    }
    settings.spiral.endpoint_restart_required = false;
    if let Some(configured_project) = configured_project {
        settings.spiral.project_id = configured_project.to_owned();
    }
    if let Some(configured_client_id) = configured_client_id {
        settings.spiral.client_id = configured_client_id.to_owned();
    }
    settings.spiral.client_secret_configured = credentials_configured;
    DataSettingsView {
        spiral: SpiralDestinationSettings {
            endpoint: settings.spiral.endpoint,
            spfs_endpoint: settings.spiral.spfs_endpoint,
            project_id: settings.spiral.project_id,
            client_id: settings.spiral.client_id,
            client_secret_configured: settings.spiral.client_secret_configured,
            namespace: settings.spiral.namespace,
            endpoint_restart_required: settings.spiral.endpoint_restart_required,
        },
        training_endpoint: settings.training.endpoint,
        video_enabled: settings.video.enabled,
    }
}

pub(super) fn contract_data_settings(settings: DataSettingsView) -> DataSettings {
    DataSettings {
        spiral: ContractSpiralSettings {
            endpoint: settings.spiral.endpoint,
            spfs_endpoint: settings.spiral.spfs_endpoint,
            project_id: settings.spiral.project_id,
            client_id: settings.spiral.client_id,
            client_secret_configured: settings.spiral.client_secret_configured,
            namespace: settings.spiral.namespace,
            endpoint_restart_required: settings.spiral.endpoint_restart_required,
        },
        training: spiral_robotics_contracts::api::TrainingRuntimeSettings {
            endpoint: settings.training_endpoint,
        },
        video: VideoSettings {
            enabled: settings.video_enabled,
        },
    }
}

pub(super) fn application_query_result(result: SpiralQueryResponse) -> ApplicationQueryResult {
    ApplicationQueryResult {
        columns: result
            .columns
            .into_iter()
            .map(|column| ApplicationQueryColumn {
                name: column.name,
                data_type: column.data_type,
                nullable: column.nullable,
                semantic_type: column.semantic_type,
            })
            .collect(),
        rows: result
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(name, value)| (name, application_query_value(value)))
                    .collect()
            })
            .collect(),
        truncated: result.truncated,
        elapsed_ms: result.elapsed_ms,
    }
}

fn application_query_value(value: Value) -> QueryValue {
    match value {
        Value::Null => QueryValue::Null,
        Value::Bool(value) => QueryValue::Bool(value),
        Value::Number(value) => value.as_i64().map_or_else(
            || {
                value.as_u64().map_or_else(
                    || QueryValue::Float(value.as_f64().unwrap_or(f64::NAN)),
                    QueryValue::Unsigned,
                )
            },
            QueryValue::Signed,
        ),
        Value::String(value) => QueryValue::String(value),
        Value::Array(values) => {
            QueryValue::Array(values.into_iter().map(application_query_value).collect())
        }
        Value::Object(values) => QueryValue::Object(
            values
                .into_iter()
                .map(|(name, value)| (name, application_query_value(value)))
                .collect(),
        ),
    }
}

fn contract_query_value(value: QueryValue) -> Value {
    match value {
        QueryValue::Null => Value::Null,
        QueryValue::Bool(value) => Value::Bool(value),
        QueryValue::Signed(value) => Value::from(value),
        QueryValue::Unsigned(value) => Value::from(value),
        QueryValue::Float(value) => Value::from(value),
        QueryValue::String(value) => Value::String(value),
        QueryValue::Array(values) => {
            Value::Array(values.into_iter().map(contract_query_value).collect())
        }
        QueryValue::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(name, value)| (name, contract_query_value(value)))
                .collect(),
        ),
    }
}

pub(super) fn contract_query_result(result: ApplicationQueryResult) -> SpiralQueryResponse {
    SpiralQueryResponse {
        columns: result
            .columns
            .into_iter()
            .map(|column| spiral_robotics_contracts::api::QueryColumn {
                name: column.name,
                data_type: column.data_type,
                nullable: column.nullable,
                semantic_type: column.semantic_type,
            })
            .collect(),
        rows: result
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(name, value)| (name, contract_query_value(value)))
                    .collect()
            })
            .collect(),
        truncated: result.truncated,
        elapsed_ms: result.elapsed_ms,
    }
}

fn data_settings_command(request: UpdateDataSettingsRequest) -> UpdateDataSettingsCommand {
    UpdateDataSettingsCommand {
        spiral_endpoint: request.spiral_endpoint,
        spiral_spfs_endpoint: request.spiral_spfs_endpoint,
        project_id: request.project_id,
        client_id: request.client_id,
        client_secret: if request.clear_client_secret {
            Some(String::new())
        } else if request.client_secret.trim().is_empty() {
            None
        } else {
            Some(request.client_secret)
        },
        training_endpoint: request.training.endpoint,
        video_enabled: request.video.enabled,
    }
}

fn spiral_replay_command(request: StartSpiralReplayRequest) -> StartSpiralReplayCommand {
    StartSpiralReplayCommand {
        episode: request.episode,
        robot: request.robot,
        stream: request.stream,
    }
}

async fn get_data_settings(State(state): State<AppState>) -> Json<DataSettings> {
    Json(contract_data_settings(data_application::get_settings(
        &DataHttpAdapter { state: &state },
    )))
}

#[derive(Debug, PartialEq, Eq)]
enum DestinationChangeAdmission {
    Ready,
    DisconnectOrphaned(Vec<String>),
    Busy,
}

fn destination_change_admission(
    hardware: &HardwareState,
    current_robot_ids: Option<&BTreeSet<String>>,
    training_is_active: bool,
    rollout_is_active: bool,
) -> DestinationChangeAdmission {
    if hardware.telemetry.recording.is_some()
        || !matches!(hardware.mode.selection, ModeSelection::Idle)
        || training_is_active
        || rollout_is_active
    {
        return DestinationChangeAdmission::Busy;
    }

    let connected = hardware
        .connections
        .iter()
        .filter(|(_, connection)| connection.connected)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    if connected.is_empty() {
        return DestinationChangeAdmission::Ready;
    }

    match current_robot_ids {
        Some(ids) if connected.iter().any(|id| ids.contains(id)) => {
            DestinationChangeAdmission::Busy
        }
        Some(ids) => DestinationChangeAdmission::DisconnectOrphaned(
            connected
                .into_iter()
                .filter(|id| !ids.contains(id))
                .collect(),
        ),
        None => DestinationChangeAdmission::DisconnectOrphaned(connected),
    }
}

async fn update_data_settings(
    State(state): State<AppState>,
    Json(request): Json<UpdateDataSettingsRequest>,
) -> Result<Json<DataSettings>, Failure> {
    let requested_endpoint =
        crate::backend::storage::settings::normalize_spiral_endpoint(&request.spiral_endpoint)
            .map_err(Failure::bad_request)?;
    let requested_spfs_endpoint =
        crate::backend::storage::settings::normalize_spiral_spfs_endpoint(
            &request.spiral_spfs_endpoint,
        )
        .map_err(Failure::bad_request)?;
    let requested_training_endpoint =
        crate::backend::storage::settings::normalize_training_runtime_endpoint(
            &request.training.endpoint,
        )
        .map_err(Failure::bad_request)?;
    let endpoint_changed = state.storage.spiral_endpoint() != requested_endpoint
        || state.storage.spiral_spfs_endpoint() != requested_spfs_endpoint;
    let project_changed = state.storage.project_id() != request.project_id.trim();
    let training_endpoint_changed =
        state.preferences.get().training.endpoint != requested_training_endpoint;
    let training_is_active = state
        .training
        .snapshot()
        .runs
        .iter()
        .any(|run| run.phase.active());
    if training_endpoint_changed && training_is_active {
        return Err(Failure::conflict(
            "Stop the active training job before changing its remote worker endpoint",
        ));
    }
    let _reservation = if project_changed || endpoint_changed {
        let token = state
            .catalog_lease
            .try_acquire(CatalogLeaseOwner::ProjectSwitch)
            .ok_or_else(|| {
                Failure::conflict(
                    "Wait for the current recording or catalog change before changing the Spiral destination",
                )
            })?;
        let reservation = CatalogReservation {
            lease: state.catalog_lease.clone(),
            token,
        };
        let hardware = state.hardware.state();
        let rollout_is_active = matches!(
            rollout_application::snapshot(&super::training_rollout::RolloutHttpAdapter(
                state.rollout.clone(),
            ))
            .phase,
            ApplicationRolloutPhase::Loading
                | ApplicationRolloutPhase::WarmingUp
                | ApplicationRolloutPhase::Running
                | ApplicationRolloutPhase::Stopping
        );
        let current_robot_ids = state.workspace.try_list().ok().map(|records| {
            records
                .into_iter()
                .map(|record| record.id)
                .collect::<BTreeSet<_>>()
        });
        match destination_change_admission(
            &hardware,
            current_robot_ids.as_ref(),
            training_is_active,
            rollout_is_active,
        ) {
            DestinationChangeAdmission::Ready => {}
            DestinationChangeAdmission::Busy => {
                return Err(Failure::conflict(
                    "Stop recording, replay, rollout, and training, then disconnect visible robots before changing the Spiral destination",
                ));
            }
            DestinationChangeAdmission::DisconnectOrphaned(robot_ids) => {
                for robot_id in robot_ids {
                    state.hardware.disconnect(&robot_id).await.map_err(|error| {
                        Failure::conflict(format!(
                            "Could not safely disconnect hidden robot {robot_id:?} before changing the Spiral destination: {error}"
                        ))
                    })?;
                }
            }
        }
        Some(reservation)
    } else {
        None
    };
    let settings = data_application::update_settings(
        &DataHttpAdapter { state: &state },
        data_settings_command(request),
    )
    .await?;
    if !endpoint_changed {
        state.bootstrap.ready();
        tokio::spawn(super::auto_connect_saved_robots(state.clone()));
    }
    Ok(Json(contract_data_settings(settings)))
}

async fn test_spiral(State(state): State<AppState>) -> Json<ConnectionTestResponse> {
    let result = data_application::test_connection(&DataHttpAdapter { state: &state }).await;
    Json(ConnectionTestResponse {
        ok: result.ok,
        message: result.message,
    })
}

async fn test_training_runtime(State(state): State<AppState>) -> Json<ConnectionTestResponse> {
    Json(state.training.test_runtime().await)
}

async fn spiral_query(
    State(state): State<AppState>,
    Json(request): Json<SpiralQueryRequest>,
) -> Result<Json<SpiralQueryResponse>, Failure> {
    let result = data_application::query(
        &DataHttpAdapter { state: &state },
        QueryCommand {
            statement: request.sql,
            max_rows: request.max_rows,
        },
    )
    .await?;
    Ok(Json(contract_query_result(result)))
}

async fn start_spiral_replay(
    State(state): State<AppState>,
    Json(request): Json<StartSpiralReplayRequest>,
) -> Result<StatusCode, Failure> {
    data_application::start_spiral_replay(
        &DataHttpAdapter { state: &state },
        state.hardware.as_ref(),
        spiral_replay_command(request),
    )
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
    let bytes = data_application::video_asset(
        &DataHttpAdapter { state: &state },
        VideoAssetQuery {
            episode: query.episode,
            stream: query.stream,
            segment: query.segment,
        },
    )
    .await
    .map_err(Failure::from)?;
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

#[cfg(test)]
mod destination_change_tests {
    use super::*;

    fn idle_hardware_with_connected_robot(id: &str) -> HardwareState {
        let mut hardware = HardwareState::default();
        hardware.connections.insert(
            id.to_owned(),
            ConnectionStatus {
                connected: true,
                error: None,
            },
        );
        hardware
    }

    #[test]
    fn unavailable_catalog_can_detach_idle_hidden_robots_during_cutover() {
        let hardware = idle_hardware_with_connected_robot("leader");

        assert_eq!(
            destination_change_admission(&hardware, None, false, false),
            DestinationChangeAdmission::DisconnectOrphaned(vec!["leader".to_owned()]),
        );
    }

    #[test]
    fn visible_connected_robot_still_blocks_destination_cutover() {
        let hardware = idle_hardware_with_connected_robot("leader");
        let current_robot_ids = BTreeSet::from(["leader".to_owned()]);

        assert_eq!(
            destination_change_admission(&hardware, Some(&current_robot_ids), false, false,),
            DestinationChangeAdmission::Busy,
        );
    }

    #[test]
    fn active_work_never_uses_the_hidden_robot_recovery_path() {
        let hardware = idle_hardware_with_connected_robot("leader");

        assert_eq!(
            destination_change_admission(&hardware, None, true, false),
            DestinationChangeAdmission::Busy,
        );
    }
}
