//! Data settings, catalog, query, and transfer commands exposed to delivery adapters.

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
    DataCatalogPort, DataSettingsSnapshot, QueryCommand, QueryResult, UpdateDataSettingsCommand,
};
use crate::http::routes::AppState;
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
    pub fn for_session(&self, session_id: &str) -> Result<Self, DataFailure> {
        Ok(Self::new(
            self.main_runtime.clone(),
            self.state.for_session(session_id)?,
        ))
    }

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
        Ok(self.state.project().settings().into())
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
        let (endpoint, spfs, project, client_id, secret, clear_secret) = connection.into_parts();
        let secret = if clear_secret {
            Some(String::new())
        } else if secret.trim().is_empty() {
            None
        } else {
            Some(secret)
        };
        let command = UpdateDataSettingsCommand::new(endpoint, project, training.endpoint())
            .with_spfs_endpoint(spfs)
            .with_client(client_id)
            .with_client_secret(secret)
            .with_video_enabled(video.enabled());
        let result = self
            .state
            .projects()
            .update_settings(self.state.project(), command)
            .await?;
        let (settings, replaced) = result.into_parts();
        if replaced {
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
        match self.state.projects().recover(self.state.project()).await {
            Ok(()) => {
                self.spawn_auto_connect_saved_robots();
                Ok((
                    true,
                    "Connected to SpiralDB and loaded the project".to_owned(),
                ))
            }
            Err(error) => Ok((false, error.to_string())),
        }
    }

    pub async fn test_training_runtime(&self) -> Result<(bool, String), DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.test_training_runtime_on_main_runtime().await })
            .await?
    }

    async fn test_training_runtime_on_main_runtime(&self) -> Result<(bool, String), DataFailure> {
        let _activity = self.state.admit()?;
        Ok(self.state.training().test_runtime().await)
    }

    pub async fn relations(&self) -> Result<RelationCatalog, DataFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.relations_on_main_runtime().await })
            .await?
    }

    async fn relations_on_main_runtime(&self) -> Result<RelationCatalog, DataFailure> {
        let _activity = self.state.admit()?;
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
        let _activity = self.state.admit()?;
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
        let _activity = self.state.admit()?;
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
        let _activity = self.state.admit()?;
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
        let _activity = self.state.admit()?;
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
        if document.is_empty() {
            return Err(DataFailure::invalid(
                "choose a .spiral-project file to import",
            ));
        }
        let summary = self
            .state
            .projects()
            .import(self.state.project(), &document)
            .await?;
        self.spawn_auto_connect_saved_robots();
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

    fn spawn_auto_connect_saved_robots(&self) {
        let reconnect_state = self.state.current();
        tokio::spawn(async move { reconnect_state.auto_connect_saved_robots().await });
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
