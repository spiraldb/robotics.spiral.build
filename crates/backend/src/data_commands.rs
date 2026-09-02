//! Data settings, catalog, query, and transfer commands exposed to delivery adapters.

use std::collections::BTreeSet;
use std::sync::Arc;

use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
    ProjectImportSummary,
};
use spiral_robotics_common::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_common::settings::{
    DataSettings, SpiralConnectionUpdate, SpiralSettings as ContractSpiralSettings,
    TrainingRuntimeSettings, VideoSettings,
};
use spiral_robotics_hardware::{HardwareState, ModeSelection as HardwareModeSelection};
use spiral_robotics_storage::ingestion::HuggingFaceDatasetDiscovery as _;

use crate::OperationError;
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease};
use crate::data::{
    ConnectionCheck, DataCatalogPort, DataSettingsSnapshot, DataSettingsUpdate, QueryCommand,
    QueryResult, TestConnectionCommand, UpdateDataSettingsCommand,
};
use crate::http::routes::{AppState, BootstrapState};
use crate::main_runtime::{MainRuntime, MainRuntimeFailure};

pub const MAX_PROJECT_IMPORT_BYTES: usize =
    spiral_robotics_storage::bundle::ProjectBundle::MAX_BYTES;

struct CatalogLeaseReservation {
    lease: Arc<CatalogMutationLease>,
    token: CatalogLeaseToken,
}

impl Drop for CatalogLeaseReservation {
    fn drop(&mut self) {
        let released = self.lease.release(self.token);
        debug_assert!(released, "catalog operation releases its exact lease once");
    }
}

struct ProjectRecoveryPublication {
    project_state_rehydrated: bool,
}

impl ProjectRecoveryPublication {
    const fn new(project_state_rehydrated: bool) -> Self {
        Self {
            project_state_rehydrated,
        }
    }

    fn publish_to(self, bootstrap: &BootstrapState) -> bool {
        if !self.project_state_rehydrated {
            return false;
        }
        bootstrap.ready();
        true
    }
}

/// Narrow command facade shared with Studio's server-function and file-transfer handlers.
#[derive(Clone)]
pub struct DataCommands {
    main_runtime: MainRuntime,
    state: AppState,
}

/// Settings values admitted together by the data command facade.
pub struct DataSettingsChange {
    connection: SpiralConnectionUpdate,
    training: TrainingRuntimeSettings,
    video: VideoSettings,
}

impl DataSettingsChange {
    pub fn new(
        connection: SpiralConnectionUpdate,
        training: TrainingRuntimeSettings,
        video: VideoSettings,
    ) -> Self {
        Self {
            connection,
            training,
            video,
        }
    }

    fn into_parts(
        self,
    ) -> (
        SpiralConnectionUpdate,
        TrainingRuntimeSettings,
        VideoSettings,
    ) {
        (self.connection, self.training, self.video)
    }
}

/// One data command failure whose backend message is shown verbatim by Studio.
#[derive(Debug)]
pub struct DataFailure(String);

impl DataCommands {
    pub(crate) fn new(main_runtime: MainRuntime, state: AppState) -> Self {
        Self {
            main_runtime,
            state,
        }
    }

    pub async fn settings(&self) -> Result<DataSettings, DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.settings_on_main_runtime().await })
            .await?
    }

    async fn settings_on_main_runtime(&self) -> Result<DataSettings, DataFailure> {
        Ok(DataAdapter::new(&self.state).settings().into())
    }

    pub async fn update_settings(
        &self,
        change: DataSettingsChange,
    ) -> Result<DataSettings, DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.update_settings_on_main_runtime(change).await })
            .await?
    }

    async fn update_settings_on_main_runtime(
        &self,
        change: DataSettingsChange,
    ) -> Result<DataSettings, DataFailure> {
        let (connection, training, video) = change.into_parts();
        let (
            spiral_endpoint,
            spiral_spfs_endpoint,
            project_id,
            client_id,
            client_secret,
            clear_client_secret,
        ) = connection.into_parts();
        let requested_endpoint = spiral_robotics_storage::ServiceEndpoint::spiral(&spiral_endpoint)
            .map(spiral_robotics_storage::ServiceEndpoint::into_string)
            .map_err(DataFailure::invalid)?;
        let requested_spfs_endpoint =
            spiral_robotics_storage::ServiceEndpoint::spiral_spfs(&spiral_spfs_endpoint)
                .map(spiral_robotics_storage::ServiceEndpoint::into_string)
                .map_err(DataFailure::invalid)?;
        let requested_training_endpoint =
            spiral_robotics_storage::ServiceEndpoint::training_runtime(training.endpoint())
                .map(spiral_robotics_storage::ServiceEndpoint::into_string)
                .map_err(DataFailure::invalid)?;
        let catalog_was_available = self.state.bootstrap().current().catalog_available();
        let current_connection = self.state.connection().settings();
        let endpoint_changed = current_connection.spiral().endpoint() != requested_endpoint
            || current_connection.spiral().spfs_endpoint() != requested_spfs_endpoint;
        let project_changed = current_connection.spiral().project_id() != project_id.trim();
        let credentials_change_requested = clear_client_secret
            || !client_secret.trim().is_empty()
            || current_connection.spiral().client_id() != client_id;
        let training_endpoint_changed =
            self.state.preferences().get().training().endpoint() != requested_training_endpoint;
        let training_is_active = self
            .state
            .training()
            .snapshot()
            .runs()
            .iter()
            .any(|run| run.phase().active());
        if training_endpoint_changed && training_is_active {
            return Err(DataFailure::conflict(
                "Stop the active training job before changing its remote worker endpoint",
            ));
        }
        let _reservation = if project_changed || endpoint_changed || credentials_change_requested {
            let token = self
                .state
                .catalog_lease()
                .try_acquire(CatalogLeaseOwner::ProjectSwitch)
                .ok_or_else(|| {
                    DataFailure::conflict(
                        "Wait for the current recording or catalog change before changing the Spiral destination",
                    )
                })?;
            let reservation = CatalogLeaseReservation {
                lease: self.state.catalog_lease().clone(),
                token,
            };
            if self.state.workflow_agent().is_active() {
                return Err(DataFailure::conflict(
                    "Stop the active Workflow Assistant turn before changing the Spiral destination",
                ));
            }
            let hardware = self.state.hardware().state();
            let rollout_is_active = self.state.rollout().snapshot().phase().active();
            let current_robot_ids = self.state.workspace().try_list().await.ok().map(|records| {
                records
                    .into_iter()
                    .map(|record| record.id().to_owned())
                    .collect::<BTreeSet<_>>()
            });
            match DestinationChangeContext::new(&hardware, current_robot_ids.as_ref())
                .with_active_work(
                    training_is_active
                        || rollout_is_active
                        || self.state.workflow().state().snapshot().phase().active(),
                )
                .admission()
            {
                DestinationChangeAdmission::Ready => {}
                DestinationChangeAdmission::Busy => {
                    return Err(DataFailure::conflict(
                        "Stop recording, replay, rollout, training, and Workflow, then disconnect visible robots before changing the Spiral destination",
                    ));
                }
                DestinationChangeAdmission::DisconnectOrphaned(robot_ids) => {
                    for robot_id in robot_ids {
                        self.state.hardware().disconnect(&robot_id).await.map_err(|error| {
                            DataFailure::conflict(format!(
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
        let client_secret = if clear_client_secret {
            Some(String::new())
        } else if client_secret.trim().is_empty() {
            None
        } else {
            Some(client_secret)
        };
        let update =
            UpdateDataSettingsCommand::new(spiral_endpoint, project_id, training.endpoint())
                .with_spfs_endpoint(spiral_spfs_endpoint)
                .with_client(client_id)
                .with_client_secret(client_secret)
                .with_video_enabled(video.enabled())
                .apply(&DataAdapter::new(&self.state))
                .await?;
        let (settings, project_state_rehydrated) = update.into_parts();
        let recovery_completed = self.complete_project_recovery(project_state_rehydrated);
        if !recovery_completed && !endpoint_changed && catalog_was_available {
            self.spawn_auto_connect_saved_robots();
        }
        Ok(settings.into())
    }

    pub async fn test_spiral(&self) -> Result<(bool, String), DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.test_spiral_on_main_runtime().await })
            .await?
    }

    async fn test_spiral_on_main_runtime(&self) -> Result<(bool, String), DataFailure> {
        let result = TestConnectionCommand::execute(&DataAdapter::new(&self.state)).await;
        let (ok, message, project_state_rehydrated) = result.into_parts();
        self.complete_project_recovery(project_state_rehydrated);
        Ok((ok, message))
    }

    pub async fn test_training_runtime(&self) -> Result<(bool, String), DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.test_training_runtime_on_main_runtime().await })
            .await?
    }

    async fn test_training_runtime_on_main_runtime(&self) -> Result<(bool, String), DataFailure> {
        Ok(self.state.training().test_runtime().await)
    }

    pub async fn relations(&self) -> Result<RelationCatalog, DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.relations_on_main_runtime().await })
            .await?
    }

    async fn relations_on_main_runtime(&self) -> Result<RelationCatalog, DataFailure> {
        self.require_catalog_ready()?;
        let token = self
            .state
            .catalog_lease()
            .try_acquire(CatalogLeaseOwner::StudioCatalogRead)
            .ok_or_else(|| {
                DataFailure::conflict(
                    "Wait for the current recording or catalog change before loading Studio relations",
                )
            })?;
        let _reservation = CatalogLeaseReservation {
            lease: self.state.catalog_lease().clone(),
            token,
        };
        self.state.workflow().relations().await.map_err(Into::into)
    }

    pub async fn query(
        &self,
        statement: String,
        max_rows: usize,
    ) -> Result<SpiralQuerySnapshot, DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.query_on_main_runtime(statement, max_rows).await })
            .await?
    }

    async fn query_on_main_runtime(
        &self,
        statement: String,
        max_rows: usize,
    ) -> Result<SpiralQuerySnapshot, DataFailure> {
        self.require_catalog_ready()?;
        QueryCommand::new(statement, max_rows)
            .execute(&DataAdapter::new(&self.state))
            .await
            .map(Into::into)
            .map_err(Into::into)
    }

    pub async fn inspect_hugging_face_dataset(
        &self,
        request: HuggingFaceDatasetSource,
    ) -> Result<HuggingFaceDatasetInspection, DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move {
                commands
                    .inspect_hugging_face_dataset_on_main_runtime(request)
                    .await
            })
            .await?
    }

    async fn inspect_hugging_face_dataset_on_main_runtime(
        &self,
        request: HuggingFaceDatasetSource,
    ) -> Result<HuggingFaceDatasetInspection, DataFailure> {
        HuggingFaceDatasetInspection::discover(
            request,
            self.state.storage().workflow_model_store().namespace(),
        )
        .await
        .map_err(DataFailure)
    }

    pub async fn import_hugging_face_dataset(
        &self,
        request: HuggingFaceDatasetSource,
    ) -> Result<HuggingFaceDatasetImportSummary, DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move {
                commands
                    .import_hugging_face_dataset_on_main_runtime(request)
                    .await
            })
            .await?
    }

    async fn import_hugging_face_dataset_on_main_runtime(
        &self,
        request: HuggingFaceDatasetSource,
    ) -> Result<HuggingFaceDatasetImportSummary, DataFailure> {
        self.require_catalog_ready()?;
        let inspection = HuggingFaceDatasetInspection::discover(
            request,
            self.state.storage().workflow_model_store().namespace(),
        )
        .await
        .map_err(DataFailure)?;
        let token = self
            .state
            .catalog_lease()
            .try_acquire(CatalogLeaseOwner::DatasetImport)
            .ok_or_else(|| {
                DataFailure::conflict(
                    "Wait for the current recording or catalog change before importing a dataset",
                )
            })?;
        let _reservation = CatalogLeaseReservation {
            lease: self.state.catalog_lease().clone(),
            token,
        };
        self.require_import_idle("a dataset")?;
        self.state
            .storage()
            .import_huggingface_dataset(inspection)
            .await
            .map_err(DataFailure)
    }

    pub(crate) async fn export_project(&self) -> Result<Vec<u8>, DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.export_project_on_main_runtime().await })
            .await?
    }

    async fn export_project_on_main_runtime(&self) -> Result<Vec<u8>, DataFailure> {
        self.require_catalog_ready()?;
        self.state
            .storage()
            .export_project_bundle()
            .await
            .map_err(DataFailure)
    }

    pub async fn import_project(
        &self,
        document: Vec<u8>,
    ) -> Result<ProjectImportSummary, DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.import_project_on_main_runtime(document).await })
            .await?
    }

    async fn import_project_on_main_runtime(
        &self,
        document: Vec<u8>,
    ) -> Result<ProjectImportSummary, DataFailure> {
        self.require_catalog_ready()?;
        let token = self
            .state
            .catalog_lease()
            .try_acquire(CatalogLeaseOwner::ProjectImport)
            .ok_or_else(|| {
                DataFailure::conflict(
                    "Wait for the current recording or catalog change before importing a project",
                )
            })?;
        let _reservation = CatalogLeaseReservation {
            lease: self.state.catalog_lease().clone(),
            token,
        };
        if document.is_empty() {
            return Err(DataFailure::invalid(
                "choose a .spiral-project file to import",
            ));
        }
        self.require_project_import_idle()?;
        self.disconnect_all_robots().await?;
        let summary = self
            .state
            .storage()
            .import_project_bundle(&document)
            .await
            .map_err(DataFailure)?;
        DataAdapter::new(&self.state)
            .recover_project_state()
            .await?;
        self.state.auto_connect_saved_robots().await;
        Ok(summary)
    }

    fn require_catalog_ready(&self) -> Result<(), DataFailure> {
        match self.state.bootstrap().catalog_unavailable() {
            Some(detail) => Err(DataFailure(format!(
                "Studio catalog is not ready: {detail}"
            ))),
            None => Ok(()),
        }
    }

    fn require_import_idle(&self, subject: &str) -> Result<(), DataFailure> {
        let hardware = self.state.hardware().state();
        let connected = hardware
            .connections()
            .values()
            .any(|connection| connection.connected());
        if self.import_activity_is_busy(&hardware) || connected {
            return Err(DataFailure::conflict(format!(
                "Disconnect robots and stop recording, replay, rollout, training, Workflow, or the Workflow Assistant before importing {subject}"
            )));
        }
        Ok(())
    }

    fn require_project_import_idle(&self) -> Result<(), DataFailure> {
        let hardware = self.state.hardware().state();
        if self.import_activity_is_busy(&hardware) {
            return Err(DataFailure::conflict(
                "Stop recording, replay, rollout, training, Workflow, or the Workflow Assistant before importing a project",
            ));
        }
        Ok(())
    }

    fn import_activity_is_busy(&self, hardware: &HardwareState) -> bool {
        hardware.recording().is_some()
            || !matches!(hardware.mode(), HardwareModeSelection::Idle)
            || self
                .state
                .training()
                .snapshot()
                .runs()
                .iter()
                .any(|run| run.phase().active())
            || self.state.rollout().snapshot().phase().active()
            || self.state.workflow().is_active()
            || self.state.workflow_agent().is_active()
    }

    async fn disconnect_all_robots(&self) -> Result<(), DataFailure> {
        let hardware = self.state.hardware().state();
        let robot_ids = hardware
            .connections()
            .iter()
            .filter(|(_, connection)| connection.connected())
            .map(|(robot, _)| robot.clone())
            .collect::<Vec<_>>();
        for robot in robot_ids {
            self.state.hardware().disconnect(&robot).await.map_err(|error| {
                DataFailure::conflict(format!(
                    "Could not safely disconnect robot {robot:?} before importing the project: {error}"
                ))
            })?;
        }
        Ok(())
    }

    fn spawn_auto_connect_saved_robots(&self) {
        let reconnect_state = self.state.clone();
        tokio::spawn(async move { reconnect_state.auto_connect_saved_robots().await });
    }

    fn complete_project_recovery(&self, project_state_rehydrated: bool) -> bool {
        let published = ProjectRecoveryPublication::new(project_state_rehydrated)
            .publish_to(self.state.bootstrap());
        if published {
            self.spawn_auto_connect_saved_robots();
        }
        published
    }
}

struct DataAdapter<'a> {
    state: &'a AppState,
}

impl<'a> DataAdapter<'a> {
    const fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl DataCatalogPort for DataAdapter<'_> {
    fn settings(&self) -> DataSettingsSnapshot {
        self.state.connection().settings()
    }

    async fn update_settings(
        &self,
        command: UpdateDataSettingsCommand,
    ) -> Result<DataSettingsUpdate, OperationError> {
        self.state
            .connection()
            .update(
                command,
                self.state.bootstrap().current().catalog_available(),
            )
            .await
    }

    async fn refresh_training_readiness(&self) {
        self.state.training().refresh_readiness().await;
    }

    async fn test_connection(&self) -> ConnectionCheck {
        let (ok, message) = self.state.storage().test_spiral().await;
        ConnectionCheck::new(ok, message)
    }

    async fn recover_project_state(&self) -> Result<(), OperationError> {
        self.state.connection().recover_project_state().await?;
        self.state.training().refresh_readiness().await;
        Ok(())
    }

    async fn query(&self, command: QueryCommand) -> Result<QueryResult, OperationError> {
        self.state
            .storage()
            .query(command.statement(), command.max_rows())
            .await
            .map(Into::into)
            .map_err(OperationError::External)
    }
}

#[cfg(test)]
pub(crate) struct DataSettingsOverrides<'a> {
    endpoint: Option<&'a str>,
    spfs_endpoint: Option<&'a str>,
    project: Option<&'a str>,
    client_id: Option<&'a str>,
    credentials_configured: bool,
}

#[cfg(test)]
impl<'a> DataSettingsOverrides<'a> {
    pub(crate) const fn new(endpoint: Option<&'a str>, spfs_endpoint: Option<&'a str>) -> Self {
        Self {
            endpoint,
            spfs_endpoint,
            project: None,
            client_id: None,
            credentials_configured: false,
        }
    }

    pub(crate) const fn with_identity(
        mut self,
        project: Option<&'a str>,
        client_id: Option<&'a str>,
    ) -> Self {
        self.project = project;
        self.client_id = client_id;
        self
    }

    pub(crate) const fn with_credentials(mut self, configured: bool) -> Self {
        self.credentials_configured = configured;
        self
    }

    pub(crate) fn apply(self, settings: DataSettings) -> DataSettingsSnapshot {
        let current = settings.spiral();
        let spiral = crate::data::SpiralDestinationSettings::new(
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
            spiral_robotics_common::settings::TrainingRuntimeSettings::new(
                settings.training_endpoint(),
            ),
            VideoSettings::new(settings.video_enabled()),
        )
    }
}

impl From<SpiralQuerySnapshot> for QueryResult {
    fn from(result: SpiralQuerySnapshot) -> Self {
        let truncated = result.truncated();
        let elapsed_ms = result.elapsed_ms();
        let columns = result
            .columns()
            .iter()
            .map(|column| {
                crate::data::QueryColumn::new(column.name(), column.data_type(), column.nullable())
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

impl From<serde_json::Value> for crate::data::QueryValue {
    fn from(value: serde_json::Value) -> Self {
        use crate::data::QueryValue;
        match value {
            serde_json::Value::Null => QueryValue::Null,
            serde_json::Value::Bool(value) => QueryValue::Bool(value),
            serde_json::Value::Number(value) => value.as_i64().map_or_else(
                || {
                    value.as_u64().map_or_else(
                        || QueryValue::Float(value.as_f64().unwrap_or(f64::NAN)),
                        QueryValue::Unsigned,
                    )
                },
                QueryValue::Signed,
            ),
            serde_json::Value::String(value) => QueryValue::String(value),
            serde_json::Value::Array(values) => {
                QueryValue::Array(values.into_iter().map(Into::into).collect())
            }
            serde_json::Value::Object(values) => QueryValue::Object(
                values
                    .into_iter()
                    .map(|(name, value)| (name, value.into()))
                    .collect(),
            ),
        }
    }
}

impl From<crate::data::QueryValue> for serde_json::Value {
    fn from(value: crate::data::QueryValue) -> Self {
        use crate::data::QueryValue;
        match value {
            QueryValue::Null => Self::Null,
            QueryValue::Bool(value) => Self::Bool(value),
            QueryValue::Signed(value) => Self::from(value),
            QueryValue::Unsigned(value) => Self::from(value),
            QueryValue::Float(value) => Self::from(value),
            QueryValue::String(value) => Self::String(value),
            QueryValue::Array(values) => Self::Array(values.into_iter().map(Into::into).collect()),
            QueryValue::Object(values) => Self::Object(
                values
                    .into_iter()
                    .map(|(name, value)| (name, value.into()))
                    .collect(),
            ),
        }
    }
}

impl From<QueryResult> for SpiralQuerySnapshot {
    fn from(result: QueryResult) -> Self {
        let (columns, rows, truncated, elapsed_ms) = result.into_parts();
        Self::new(
            columns
                .into_iter()
                .map(|column| {
                    let (name, data_type, nullable, semantic_type) = column.into_parts();
                    spiral_robotics_common::relations::QueryColumn::new(name, data_type, nullable)
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

#[derive(Debug, Eq, PartialEq)]
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
            hardware_busy: hardware.recording().is_some()
                || !matches!(hardware.mode(), HardwareModeSelection::Idle),
            connected_robot_ids: hardware
                .connections()
                .iter()
                .filter(|(_, connection)| connection.connected())
                .map(|(id, _)| id.clone())
                .collect(),
            current_robot_ids: current_robot_ids.cloned(),
        }
    }

    const fn with_active_work(mut self, active: bool) -> Self {
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

impl DataFailure {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for DataFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<OperationError> for DataFailure {
    fn from(error: OperationError) -> Self {
        Self(error.to_string())
    }
}

impl From<String> for DataFailure {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<MainRuntimeFailure> for DataFailure {
    fn from(error: MainRuntimeFailure) -> Self {
        Self(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::settings::{
        DataSettings, SpiralSettings, TrainingRuntimeSettings, VideoSettings,
    };
    use spiral_robotics_common::status::{BackendReadiness, BackendReadinessState};

    #[test]
    fn unavailable_catalog_can_detach_idle_hidden_robots_during_cutover() {
        let hardware = DestinationChangeContext::idle_with_connection("leader");
        assert_eq!(
            hardware.admission(),
            DestinationChangeAdmission::DisconnectOrphaned(vec!["leader".to_owned()]),
        );
    }

    #[test]
    fn visible_connected_robot_still_blocks_destination_cutover() {
        let mut hardware = DestinationChangeContext::idle_with_connection("leader");
        hardware.current_robot_ids = Some(BTreeSet::from(["leader".to_owned()]));
        assert_eq!(hardware.admission(), DestinationChangeAdmission::Busy);
    }

    #[test]
    fn active_work_never_uses_the_hidden_robot_recovery_path() {
        let hardware =
            DestinationChangeContext::idle_with_connection("leader").with_active_work(true);
        assert_eq!(hardware.admission(), DestinationChangeAdmission::Busy);
    }

    #[test]
    fn failed_opportunistic_recovery_keeps_the_catalog_degraded() {
        let bootstrap = BootstrapState::new();
        bootstrap.degraded(false, "run Test connection");
        assert!(!ProjectRecoveryPublication::new(false).publish_to(&bootstrap));
        assert_eq!(
            bootstrap.current(),
            BackendReadiness::new(
                BackendReadinessState::Degraded,
                false,
                "run Test connection"
            )
        );
    }

    #[test]
    fn successful_project_recovery_makes_the_catalog_ready() {
        let bootstrap = BootstrapState::new();
        bootstrap.degraded(false, "run Test connection");
        assert!(ProjectRecoveryPublication::new(true).publish_to(&bootstrap));
        assert_eq!(bootstrap.current().state(), BackendReadinessState::Ready);
        assert!(bootstrap.current().catalog_available());
    }

    #[test]
    fn data_settings_projection_preserves_values_and_pending_identity() {
        let settings = DataSettings::new(
            SpiralSettings::new(
                "https://api.spiraldb.dev",
                "https://spfs.spiraldb.dev",
                "project",
            )
            .with_namespace("dataset"),
            TrainingRuntimeSettings::default(),
            VideoSettings::new(true),
        );
        assert_eq!(
            DataSettings::from(DataSettingsOverrides::new(None, None).apply(settings.clone())),
            settings
        );

        let pending = DataSettingsOverrides::new(
            Some("http://127.0.0.1:4279"),
            Some("http://127.0.0.1:4295"),
        )
        .with_identity(Some("local-project"), Some("client-id"))
        .with_credentials(true)
        .apply(settings);
        assert_eq!(pending.spiral().endpoint(), "http://127.0.0.1:4279");
        assert_eq!(pending.spiral().spfs_endpoint(), "http://127.0.0.1:4295");
        assert_eq!(pending.spiral().project_id(), "local-project");
        assert_eq!(pending.spiral().client_id(), "client-id");
        assert!(pending.spiral().client_secret_configured());
    }
}
