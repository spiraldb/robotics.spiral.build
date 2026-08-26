use super::*;
use spiral_robotics_storage::ingestion::HuggingFaceDatasetDiscovery as _;

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
impl AppState {
    pub(super) fn data_bootstrap_router() -> Router<Self> {
        Router::new()
            .route(
                "/settings/data",
                get(Self::get_data_settings).patch(Self::update_data_settings),
            )
            .route("/settings/data/test/spiral", post(Self::test_spiral))
            .route(
                "/settings/data/test/training",
                post(Self::test_training_runtime),
            )
            .route(
                "/datasets/huggingface/inspect",
                post(Self::inspect_huggingface_dataset),
            )
    }

    pub(super) fn data_catalog_router() -> Router<Self> {
        Router::new()
            .route("/spiral/query", post(Self::spiral_query))
            .route("/spiral/relations", get(Self::spiral_relations))
            .route(
                "/spiral/project-relations",
                get(Self::project_data_relations),
            )
            .route(
                "/datasets/huggingface/import",
                post(Self::import_huggingface_dataset),
            )
            .route("/project/export", get(Self::export_project))
            .route(
                "/project/import",
                post(Self::import_project).layer(DefaultBodyLimit::max(
                    spiral_robotics_storage::bundle::ProjectBundle::MAX_BYTES,
                )),
            )
            .route(
                "/episodes/import",
                post(Self::import_episodes).layer(DefaultBodyLimit::max(
                    spiral_robotics_storage::bundle::EpisodeBundle::MAX_BYTES,
                )),
            )
    }
}

impl AppState {
    async fn spiral_relations(
        State(state): State<Self>,
    ) -> Result<Json<spiral_robotics_api::workflow::WorkflowRelationsResponse>, Failure> {
        state
            .storage()
            .workflow_relations()
            .await
            .map(Json)
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))
    }

    async fn project_data_relations(
        State(state): State<Self>,
    ) -> Result<Json<spiral_robotics_api::relations::ProjectDataRelationsResponse>, Failure> {
        state
            .storage()
            .project_data_relations()
            .await
            .map(Json)
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))
    }

    async fn inspect_huggingface_dataset(
        State(state): State<Self>,
        Json(request): Json<spiral_robotics_api::datasets::HuggingFaceDatasetRequest>,
    ) -> Result<Json<spiral_robotics_api::datasets::HuggingFaceDatasetInspection>, Failure> {
        spiral_robotics_api::datasets::HuggingFaceDatasetInspection::discover(
            request,
            state.storage().workflow_model_store().namespace(),
        )
        .await
        .map(Json)
        .map_err(Failure::hugging_face_inspection)
    }

    async fn import_huggingface_dataset(
        State(state): State<Self>,
        Json(request): Json<spiral_robotics_api::datasets::HuggingFaceDatasetRequest>,
    ) -> Result<Json<spiral_robotics_api::datasets::HuggingFaceDatasetImportSummary>, Failure> {
        let inspection = spiral_robotics_api::datasets::HuggingFaceDatasetInspection::discover(
            request,
            state.storage().workflow_model_store().namespace(),
        )
        .await
        .map_err(Failure::hugging_face_inspection)?;
        let token = state
            .catalog_lease()
            .try_acquire(CatalogLeaseOwner::DatasetImport)
            .ok_or_else(|| {
                Failure::conflict(
                    "Wait for the current recording or catalog change before importing a dataset",
                )
            })?;
        let _reservation = CatalogReservation {
            lease: state.catalog_lease().clone(),
            token,
        };
        state.require_import_idle("a dataset")?;
        let storage = state.storage().clone();
        tokio::task::spawn_blocking(move || storage.import_huggingface_dataset(inspection))
            .await
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
            .map(Json)
            .map_err(Failure::import)
    }
}

impl Failure {
    fn hugging_face_inspection(error: String) -> Self {
        let remote = error.starts_with("contacting Hugging Face")
            || error.starts_with("listing Hugging Face")
            || error.starts_with("decoding Hugging Face");
        if remote {
            Self(StatusCode::BAD_GATEWAY, error)
        } else {
            Self::bad_request(error)
        }
    }
}

#[derive(Debug, Deserialize)]
struct EpisodeTransferQuery {
    dataset_id: String,
}

impl AppState {
    async fn export_project(State(state): State<Self>) -> Result<Response, Failure> {
        let storage = state.storage().clone();
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
        State(state): State<Self>,
        body: Bytes,
    ) -> Result<Json<spiral_robotics_api::datasets::ProjectImportSummary>, Failure> {
        let token = state
            .catalog_lease()
            .try_acquire(CatalogLeaseOwner::ProjectImport)
            .ok_or_else(|| {
                Failure(
                    StatusCode::CONFLICT,
                    "Wait for the current recording or catalog change before importing a project"
                        .to_owned(),
                )
            })?;
        let _reservation = CatalogReservation {
            lease: state.catalog_lease().clone(),
            token,
        };
        if body.is_empty() {
            return Err(Failure::bad_request(
                "choose a .spiral-project file to import",
            ));
        }
        state.require_project_import_idle()?;
        state.disconnect_all_robots().await?;
        let storage = state.storage().clone();
        let summary = tokio::task::spawn_blocking(move || storage.import_project_bundle(&body))
            .await
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
            .map_err(Failure::import)?;
        state.auto_connect_saved_robots().await;
        state.preferences().replace(
            state
                .storage()
                .load_studio_preferences()
                .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?,
        );
        state
            .training()
            .reload_from_storage()
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?;
        state.training().refresh_readiness().await;
        state
            .rollout()
            .reload_from_storage()
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?;
        state
            .workflow()
            .reload_from_storage()
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?;
        state
            .workflow_agent()
            .reload_from_storage()
            .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error))?;
        Ok(Json(summary))
    }

    async fn import_episodes(
        State(state): State<Self>,
        Query(query): Query<EpisodeTransferQuery>,
        body: Bytes,
    ) -> Result<Json<spiral_robotics_api::datasets::EpisodeImportSummary>, Failure> {
        if !spiral_robotics_storage::StorageSchema::is_dataset_id(&query.dataset_id) {
            return Err(Failure::bad_request(
                "dataset ID must contain 1–96 letters, numbers, '.', '_' or '-'",
            ));
        }
        let token = state
            .catalog_lease()
            .try_acquire(CatalogLeaseOwner::EpisodeImport)
            .ok_or_else(|| {
                Failure::conflict(
                    "Wait for the current recording or catalog change before importing episodes",
                )
            })?;
        let _reservation = CatalogReservation {
            lease: state.catalog_lease().clone(),
            token,
        };
        state.require_import_idle("episodes")?;
        if body.is_empty() {
            return Err(Failure::bad_request(
                "choose a .spiral-episodes file to import",
            ));
        }
        let storage = state.storage().clone();
        let dataset_id = query.dataset_id;
        let summary =
            tokio::task::spawn_blocking(move || storage.import_episode_bundle(&dataset_id, &body))
                .await
                .map_err(|error| Failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
                .map_err(Failure::import)?;
        Ok(Json(summary))
    }
}

impl AppState {
    fn require_import_idle(&self, subject: &str) -> Result<(), Failure> {
        let hardware = self.hardware().state();
        let connected = hardware
            .connections()
            .values()
            .any(|connection| connection.connected());
        if self.import_activity_is_busy(&hardware) || connected {
            return Err(Failure::conflict(format!(
                "Disconnect robots and stop recording, replay, rollout, training, Workflow, or the Workflow Assistant before importing {subject}"
            )));
        }
        Ok(())
    }

    fn require_project_import_idle(&self) -> Result<(), Failure> {
        let hardware = self.hardware().state();
        if self.import_activity_is_busy(&hardware) {
            return Err(Failure::conflict(
                "Stop recording, replay, rollout, training, Workflow, or the Workflow Assistant before importing a project",
            ));
        }
        Ok(())
    }

    fn import_activity_is_busy(&self, hardware: &HardwareState) -> bool {
        hardware.telemetry().recording().is_some()
            || !matches!(hardware.mode().selection(), ModeSelection::Idle)
            || self
                .training()
                .snapshot()
                .runs()
                .iter()
                .any(|run| run.phase().active())
            || RolloutActions::new(&super::training_rollout::RolloutHttpAdapter::new(
                self.rollout().clone(),
            ))
            .snapshot()
            .phase()
            .active()
            || self.workflow().is_active()
            || self.workflow_agent().is_active()
    }

    async fn disconnect_all_robots(&self) -> Result<(), Failure> {
        let hardware = self.hardware().state();
        let robot_ids = hardware
            .connections()
            .iter()
            .filter(|(_, connection)| connection.connected())
            .map(|(robot, _)| robot.clone())
            .collect::<Vec<_>>();
        for robot in robot_ids {
            self.hardware().disconnect(&robot).await.map_err(|error| {
            Failure::conflict(format!(
                "Could not safely disconnect robot {robot:?} before importing the project: {error}"
            ))
        })?;
        }
        Ok(())
    }
}

pub(super) struct DataHttpAdapter<'a> {
    state: &'a AppState,
}

impl DataCatalogPort for DataHttpAdapter<'_> {
    fn settings(&self) -> DataSettingsSnapshot {
        let configured_endpoint = self.state.backend_state().spiral_endpoint();
        let configured_spfs_endpoint = self.state.backend_state().spiral_spfs_endpoint();
        let configured_project = self.state.backend_state().project_id();
        let configured_client_id = self.state.backend_state().client_id();
        let credentials_configured = self.state.backend_state().credentials().is_some();
        DataSettingsOverrides::new(
            configured_endpoint.as_deref(),
            configured_spfs_endpoint.as_deref(),
        )
        .with_identity(
            configured_project.as_deref(),
            (!configured_client_id.is_empty()).then_some(configured_client_id.as_str()),
        )
        .with_credentials(credentials_configured)
        .apply(
            self.state
                .storage()
                .data_settings(&self.state.preferences().get()),
        )
    }

    fn update_settings(
        &self,
        command: UpdateDataSettingsCommand,
    ) -> Result<DataSettingsSnapshot, OperationError> {
        let catalog_was_available = self.state.bootstrap().current().catalog_available();
        let endpoint_changed = self.state.storage().spiral_endpoint() != command.spiral_endpoint()
            || self.state.storage().spiral_spfs_endpoint() != command.spiral_spfs_endpoint();
        let project_changed = self.state.storage().project_id() != command.project_id();
        let current_credentials = self.state.backend_state().credentials();
        let credentials = match command.client_secret() {
            Some("") => None,
            Some(secret) => Some(spiral_robotics_storage::SpiralCredentials::new(
                command.client_id().to_owned(),
                secret.to_owned(),
            )),
            None if command.client_id().is_empty() => None,
            None => current_credentials
                .clone()
                .filter(|credentials| credentials.client_id() == command.client_id())
                .ok_or_else(|| {
                    OperationError::InvalidInput(
                        "Enter the client secret when changing the Spiral client ID".to_owned(),
                    )
                })
                .map(Some)?,
        };
        let credentials_changed = current_credentials != credentials;
        if (endpoint_changed || project_changed || credentials_changed)
            && self.state.workflow().state().snapshot().phase().active()
        {
            return Err(OperationError::Conflict(
                "Cancel the active Workflow run before changing the Spiral destination or credentials."
                    .to_owned(),
            ));
        }
        let staged = self
            .state
            .preferences()
            .stage_update(
                spiral_robotics_api::settings::TrainingRuntimeSettings::new(
                    command.training_endpoint(),
                ),
                VideoSettings::new(command.video_enabled()),
            )
            .map_err(OperationError::InvalidInput)?;
        let connection = spiral_robotics_storage::SpiralConnectionSettings::new(
            command.spiral_endpoint().to_owned(),
            command.spiral_spfs_endpoint().to_owned(),
        )
        .and_then(|settings| settings.with_project(command.project_id()))
        .map(|settings| settings.with_credentials(credentials.clone()))
        .map_err(OperationError::InvalidInput)?;
        self.state
            .backend_state()
            .save_connection(&connection)
            .map_err(OperationError::Internal)?;
        if endpoint_changed || project_changed || credentials_changed {
            self.state
                .storage()
                .configure_connection(connection)
                .map_err(OperationError::InvalidInput)?;
        }
        self.state
            .storage()
            .save_studio_preferences(&staged)
            .map_err(OperationError::External)?;
        if endpoint_changed || project_changed || credentials_changed || !catalog_was_available {
            self.state
                .training()
                .reload_from_storage()
                .map_err(OperationError::External)?;
            self.state
                .rollout()
                .reload_from_storage()
                .map_err(OperationError::External)?;
            self.state
                .workflow()
                .reload_from_storage()
                .map_err(OperationError::External)?;
            self.state
                .workflow_agent()
                .reload_from_storage()
                .map_err(OperationError::External)?;
        }
        self.state.preferences().replace(staged);
        Ok(self.settings())
    }

    async fn refresh_training_readiness(&self) {
        self.state.training().refresh_readiness().await;
    }

    async fn test_connection(&self) -> ConnectionCheck {
        let response = self.state.storage().test_spiral().await;
        ConnectionCheck::new(response.ok(), response.message())
    }

    async fn query(&self, command: QueryCommand) -> Result<CapabilityQueryResult, OperationError> {
        self.state
            .storage()
            .query(command.statement(), command.max_rows())
            .await
            .map(Into::into)
            .map_err(OperationError::External)
    }

}

pub(super) struct DataSettingsOverrides<'a> {
    endpoint: Option<&'a str>,
    spfs_endpoint: Option<&'a str>,
    project: Option<&'a str>,
    client_id: Option<&'a str>,
    credentials_configured: bool,
}

impl<'a> DataSettingsOverrides<'a> {
    pub(super) fn new(endpoint: Option<&'a str>, spfs_endpoint: Option<&'a str>) -> Self {
        Self {
            endpoint,
            spfs_endpoint,
            project: None,
            client_id: None,
            credentials_configured: false,
        }
    }

    pub(super) fn with_identity(
        mut self,
        project: Option<&'a str>,
        client_id: Option<&'a str>,
    ) -> Self {
        self.project = project;
        self.client_id = client_id;
        self
    }

    pub(super) fn with_credentials(mut self, configured: bool) -> Self {
        self.credentials_configured = configured;
        self
    }

    pub(super) fn apply(self, settings: DataSettings) -> DataSettingsSnapshot {
        let current = settings.spiral();
        let spiral = SpiralDestinationSettings::new(
            self.endpoint.unwrap_or(current.endpoint()),
            self.spfs_endpoint.unwrap_or(current.spfs_endpoint()),
            self.project.unwrap_or(current.project_id()),
        )
        .with_client(
            self.client_id.unwrap_or(current.client_id()),
            self.credentials_configured,
        )
        .with_namespace(current.namespace());
        DataSettingsSnapshot::new(
            spiral,
            settings.training().endpoint(),
            settings.video().enabled(),
        )
    }
}

impl From<DataSettingsSnapshot> for DataSettings {
    fn from(settings: DataSettingsSnapshot) -> Self {
        Self::new(
            ContractSpiralSettings::new(
                settings.spiral().endpoint(),
                settings.spiral().spfs_endpoint(),
                settings.spiral().project_id(),
            )
            .with_client(
                settings.spiral().client_id(),
                settings.spiral().client_secret_configured(),
            )
            .with_namespace(settings.spiral().namespace())
            .with_endpoint_restart_required(settings.spiral().endpoint_restart_required()),
            spiral_robotics_api::settings::TrainingRuntimeSettings::new(
                settings.training_endpoint(),
            ),
            VideoSettings::new(settings.video_enabled()),
        )
    }
}

impl From<SpiralQueryResponse> for CapabilityQueryResult {
    fn from(result: SpiralQueryResponse) -> Self {
        let truncated = result.truncated();
        let elapsed_ms = result.elapsed_ms();
        let columns = result
            .columns()
            .iter()
            .map(|column| {
                CapabilityQueryColumn::new(column.name(), column.data_type(), column.nullable())
                    .with_semantic_type(column.semantic_type().map(str::to_owned))
            })
            .collect();
        let rows = result
            .into_rows()
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(name, value)| (name, value.into()))
                    .collect()
            })
            .collect();
        Self::new(columns, rows).with_status(truncated, elapsed_ms)
    }
}

impl From<Value> for QueryValue {
    fn from(value: Value) -> Self {
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
            Value::Array(values) => QueryValue::Array(values.into_iter().map(Into::into).collect()),
            Value::Object(values) => QueryValue::Object(
                values
                    .into_iter()
                    .map(|(name, value)| (name, value.into()))
                    .collect(),
            ),
        }
    }
}

impl From<QueryValue> for Value {
    fn from(value: QueryValue) -> Self {
        match value {
            QueryValue::Null => Value::Null,
            QueryValue::Bool(value) => Value::Bool(value),
            QueryValue::Signed(value) => Value::from(value),
            QueryValue::Unsigned(value) => Value::from(value),
            QueryValue::Float(value) => Value::from(value),
            QueryValue::String(value) => Value::String(value),
            QueryValue::Array(values) => Value::Array(values.into_iter().map(Into::into).collect()),
            QueryValue::Object(values) => Value::Object(
                values
                    .into_iter()
                    .map(|(name, value)| (name, value.into()))
                    .collect(),
            ),
        }
    }
}

impl From<CapabilityQueryResult> for SpiralQueryResponse {
    fn from(result: CapabilityQueryResult) -> Self {
        let (columns, rows, truncated, elapsed_ms) = result.into_parts();
        Self::new(
            columns
                .into_iter()
                .map(|column| {
                    let (name, data_type, nullable, semantic_type) = column.into_parts();
                    spiral_robotics_api::relations::QueryColumn::new(name, data_type, nullable)
                        .with_semantic_type(semantic_type)
                })
                .collect(),
            rows.into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|(name, value)| (name, value.into()))
                        .collect()
                })
                .collect(),
            truncated,
        )
        .with_elapsed_ms(elapsed_ms)
    }
}

impl From<UpdateDataSettingsRequest> for UpdateDataSettingsCommand {
    fn from(request: UpdateDataSettingsRequest) -> Self {
        Self::new(
            request.spiral_endpoint(),
            request.project_id(),
            request.training().endpoint(),
        )
        .with_spfs_endpoint(request.spiral_spfs_endpoint())
        .with_client(request.client_id())
        .with_client_secret(if request.clear_client_secret() {
            Some(String::new())
        } else if request.client_secret().trim().is_empty() {
            None
        } else {
            Some(request.client_secret().to_owned())
        })
        .with_video_enabled(request.video().enabled())
    }
}

impl AppState {
    async fn get_data_settings(State(state): State<Self>) -> Json<DataSettings> {
        Json(DataHttpAdapter { state: &state }.settings().into())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DestinationChangeAdmission {
    Ready,
    DisconnectOrphaned(Vec<String>),
    Busy,
}

struct DestinationChangeContext {
    hardware_busy: bool,
    connected_robot_ids: Vec<String>,
    current_robot_ids: Option<BTreeSet<String>>,
}

impl DestinationChangeContext {
    fn new(hardware: &HardwareState, current_robot_ids: Option<&BTreeSet<String>>) -> Self {
        Self {
            hardware_busy: hardware.telemetry().recording().is_some()
                || !matches!(hardware.mode().selection(), ModeSelection::Idle),
            connected_robot_ids: hardware
                .connections()
                .iter()
                .filter(|(_, connection)| connection.connected())
                .map(|(id, _)| id.clone())
                .collect(),
            current_robot_ids: current_robot_ids.cloned(),
        }
    }

    fn with_active_work(mut self, active: bool) -> Self {
        self.hardware_busy |= active;
        self
    }

    fn admission(self) -> DestinationChangeAdmission {
        if self.hardware_busy {
            return DestinationChangeAdmission::Busy;
        }

        if self.connected_robot_ids.is_empty() {
            return DestinationChangeAdmission::Ready;
        }

        match self.current_robot_ids {
            Some(ids) if self.connected_robot_ids.iter().any(|id| ids.contains(id)) => {
                DestinationChangeAdmission::Busy
            }
            Some(ids) => DestinationChangeAdmission::DisconnectOrphaned(
                self.connected_robot_ids
                    .into_iter()
                    .filter(|id| !ids.contains(id))
                    .collect(),
            ),
            None => DestinationChangeAdmission::DisconnectOrphaned(self.connected_robot_ids),
        }
    }

    #[cfg(test)]
    fn idle_with_connection(id: &str) -> Self {
        Self {
            hardware_busy: false,
            connected_robot_ids: vec![id.to_owned()],
            current_robot_ids: None,
        }
    }
}

impl AppState {
    async fn update_data_settings(
        State(state): State<Self>,
        Json(request): Json<UpdateDataSettingsRequest>,
    ) -> Result<Json<DataSettings>, Failure> {
        let requested_endpoint =
            spiral_robotics_storage::ServiceEndpoint::spiral(request.spiral_endpoint())
                .map(spiral_robotics_storage::ServiceEndpoint::into_string)
                .map_err(Failure::bad_request)?;
        let requested_spfs_endpoint =
            spiral_robotics_storage::ServiceEndpoint::spiral_spfs(request.spiral_spfs_endpoint())
                .map(spiral_robotics_storage::ServiceEndpoint::into_string)
                .map_err(Failure::bad_request)?;
        let requested_training_endpoint =
            spiral_robotics_storage::ServiceEndpoint::training_runtime(
                request.training().endpoint(),
            )
            .map(spiral_robotics_storage::ServiceEndpoint::into_string)
            .map_err(Failure::bad_request)?;
        let endpoint_changed = state.storage().spiral_endpoint() != requested_endpoint
            || state.storage().spiral_spfs_endpoint() != requested_spfs_endpoint;
        let project_changed = state.storage().project_id() != request.project_id().trim();
        let credentials_change_requested = request.clear_client_secret()
            || !request.client_secret().trim().is_empty()
            || state.backend_state().client_id() != request.client_id();
        let training_endpoint_changed =
            state.preferences().get().training().endpoint() != requested_training_endpoint;
        let training_is_active = state
            .training()
            .snapshot()
            .runs()
            .iter()
            .any(|run| run.phase().active());
        if training_endpoint_changed && training_is_active {
            return Err(Failure::conflict(
                "Stop the active training job before changing its remote worker endpoint",
            ));
        }
        let _reservation = if project_changed || endpoint_changed || credentials_change_requested {
            let token = state
            .catalog_lease()
            .try_acquire(CatalogLeaseOwner::ProjectSwitch)
            .ok_or_else(|| {
                Failure::conflict(
                    "Wait for the current recording or catalog change before changing the Spiral destination",
                )
            })?;
            let reservation = CatalogReservation {
                lease: state.catalog_lease().clone(),
                token,
            };
            if state.workflow_agent().is_active() {
                return Err(Failure::conflict(
                    "Stop the active Workflow Assistant turn before changing the Spiral destination",
                ));
            }
            let hardware = state.hardware().state();
            let rollout_is_active =
                RolloutActions::new(&super::training_rollout::RolloutHttpAdapter::new(
                    state.rollout().clone(),
                ))
                .snapshot()
                .phase()
                .active();
            let current_robot_ids = state.workspace().try_list().ok().map(|records| {
                records
                    .into_iter()
                    .map(|record| record.id().to_owned())
                    .collect::<BTreeSet<_>>()
            });
            match DestinationChangeContext::new(&hardware, current_robot_ids.as_ref())
                .with_active_work(
                    training_is_active
                        || rollout_is_active
                        || state.workflow().state().snapshot().phase().active(),
                )
                .admission()
            {
                DestinationChangeAdmission::Ready => {}
                DestinationChangeAdmission::Busy => {
                    return Err(Failure::conflict(
                        "Stop recording, replay, rollout, training, and Workflow, then disconnect visible robots before changing the Spiral destination",
                    ));
                }
                DestinationChangeAdmission::DisconnectOrphaned(robot_ids) => {
                    for robot_id in robot_ids {
                        state.hardware().disconnect(&robot_id).await.map_err(|error| {
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
        let settings = UpdateDataSettingsCommand::from(request)
            .apply(&DataHttpAdapter { state: &state })
            .await?;
        if !endpoint_changed {
            state.bootstrap().ready();
            let reconnect_state = state.clone();
            tokio::spawn(async move { reconnect_state.auto_connect_saved_robots().await });
        }
        Ok(Json(settings.into()))
    }

    async fn test_spiral(State(state): State<Self>) -> Json<ConnectionTestResponse> {
        let result = DataHttpAdapter { state: &state }.test_connection().await;
        let (ok, message) = result.into_parts();
        Json(ConnectionTestResponse::new(ok, message))
    }

    async fn test_training_runtime(State(state): State<Self>) -> Json<ConnectionTestResponse> {
        Json(state.training().test_runtime().await)
    }

    async fn spiral_query(
        State(state): State<Self>,
        Json(request): Json<SpiralQueryRequest>,
    ) -> Result<Json<SpiralQueryResponse>, Failure> {
        let result = QueryCommand::new(request.sql(), request.max_rows())
            .execute(&DataHttpAdapter { state: &state })
            .await?;
        Ok(Json(result.into()))
    }

}

impl AppState {
}

#[cfg(test)]
mod destination_change_tests {
    use super::*;

    fn idle_hardware_with_connected_robot(id: &str) -> DestinationChangeContext {
        DestinationChangeContext::idle_with_connection(id)
    }

    #[test]
    fn unavailable_catalog_can_detach_idle_hidden_robots_during_cutover() {
        let hardware = idle_hardware_with_connected_robot("leader");

        assert_eq!(
            hardware.admission(),
            DestinationChangeAdmission::DisconnectOrphaned(vec!["leader".to_owned()]),
        );
    }

    #[test]
    fn visible_connected_robot_still_blocks_destination_cutover() {
        let mut hardware = idle_hardware_with_connected_robot("leader");
        let current_robot_ids = BTreeSet::from(["leader".to_owned()]);
        hardware.current_robot_ids = Some(current_robot_ids);

        assert_eq!(hardware.admission(), DestinationChangeAdmission::Busy,);
    }

    #[test]
    fn active_work_never_uses_the_hidden_robot_recovery_path() {
        let hardware = idle_hardware_with_connected_robot("leader").with_active_work(true);

        assert_eq!(hardware.admission(), DestinationChangeAdmission::Busy,);
    }
}
