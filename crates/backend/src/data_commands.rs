//! Data settings, catalog, query, and transfer commands exposed to delivery adapters.

use std::sync::Arc;

use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
    ProjectImportSummary,
};
use spiral_robotics_common::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_common::settings::{
    DataSettings, SpiralConnectionUpdate, TrainingRuntimeSettings, VideoSettings,
};
use spiral_robotics_hardware::{HardwareState, ModeSelection as HardwareModeSelection};
use spiral_robotics_storage::ingestion::HuggingFaceDatasetDiscovery as _;

use crate::BackendCommandError;
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease};
use crate::data::query::QueryCommand;
use crate::data::settings::UpdateDataSettingsCommand;
use crate::http::routes::AppState;
use crate::main_runtime::MainRuntime;

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

impl DataCommands {
    pub fn for_session(&self, session_id: &str) -> Result<Self, BackendCommandError> {
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

    pub async fn settings(&self) -> Result<DataSettings, BackendCommandError> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.settings_on_main_runtime().await })
            .await?
    }

    async fn settings_on_main_runtime(&self) -> Result<DataSettings, BackendCommandError> {
        Ok(self.state.project().settings())
    }

    pub async fn update_settings(
        &self,
        change: DataSettingsChange,
    ) -> Result<DataSettings, BackendCommandError> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.update_settings_on_main_runtime(change).await })
            .await?
    }

    async fn update_settings_on_main_runtime(
        &self,
        change: DataSettingsChange,
    ) -> Result<DataSettings, BackendCommandError> {
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
        Ok(settings)
    }

    pub async fn test_spiral(&self) -> Result<(bool, String), BackendCommandError> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.test_spiral_on_main_runtime().await })
            .await?
    }

    async fn test_spiral_on_main_runtime(&self) -> Result<(bool, String), BackendCommandError> {
        match self.state.projects().recover(self.state.project()).await {
            Ok(()) => {
                self.spawn_auto_connect_saved_robots();
                Ok((
                    true,
                    "Connected to SpiralDB and loaded the project".to_owned(),
                ))
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn test_training_runtime(&self) -> Result<(bool, String), BackendCommandError> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.test_training_runtime_on_main_runtime().await })
            .await?
    }

    async fn test_training_runtime_on_main_runtime(
        &self,
    ) -> Result<(bool, String), BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.training().test_runtime().await)
    }

    pub async fn relations(&self) -> Result<RelationCatalog, BackendCommandError> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.relations_on_main_runtime().await })
            .await?
    }

    async fn relations_on_main_runtime(&self) -> Result<RelationCatalog, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.require_catalog_ready()?;
        let token = self
            .state
            .catalog_lease()
            .try_acquire(CatalogLeaseOwner::StudioCatalogRead)
            .ok_or_else(|| {
                BackendCommandError::conflict(
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
    ) -> Result<SpiralQuerySnapshot, BackendCommandError> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.query_on_main_runtime(statement, max_rows).await })
            .await?
    }

    async fn query_on_main_runtime(
        &self,
        statement: String,
        max_rows: usize,
    ) -> Result<SpiralQuerySnapshot, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.require_catalog_ready()?;
        let command = QueryCommand::new(statement, max_rows).validated()?;
        self.state
            .storage()
            .query(command.statement(), command.max_rows())
            .await
            .map_err(Into::into)
    }

    pub async fn inspect_hugging_face_dataset(
        &self,
        request: HuggingFaceDatasetSource,
    ) -> Result<HuggingFaceDatasetInspection, BackendCommandError> {
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
    ) -> Result<HuggingFaceDatasetInspection, BackendCommandError> {
        let _activity = self.state.admit()?;
        HuggingFaceDatasetInspection::discover(
            request,
            self.state.storage().workflow_model_store().namespace(),
        )
        .await
        .map_err(BackendCommandError::from)
    }

    pub async fn import_hugging_face_dataset(
        &self,
        request: HuggingFaceDatasetSource,
    ) -> Result<HuggingFaceDatasetImportSummary, BackendCommandError> {
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
    ) -> Result<HuggingFaceDatasetImportSummary, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.require_catalog_ready()?;
        let inspection = HuggingFaceDatasetInspection::discover(
            request,
            self.state.storage().workflow_model_store().namespace(),
        )
        .await
        .map_err(BackendCommandError::from)?;
        let token = self
            .state
            .catalog_lease()
            .try_acquire(CatalogLeaseOwner::DatasetImport)
            .ok_or_else(|| {
                BackendCommandError::conflict(
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
            .map_err(BackendCommandError::from)
    }

    pub(crate) async fn export_project(&self) -> Result<Vec<u8>, BackendCommandError> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.export_project_on_main_runtime().await })
            .await?
    }

    async fn export_project_on_main_runtime(&self) -> Result<Vec<u8>, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.require_catalog_ready()?;
        self.state
            .storage()
            .export_project_bundle()
            .await
            .map_err(BackendCommandError::from)
    }

    pub async fn import_project(
        &self,
        document: Vec<u8>,
    ) -> Result<ProjectImportSummary, BackendCommandError> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move { commands.import_project_on_main_runtime(document).await })
            .await?
    }

    async fn import_project_on_main_runtime(
        &self,
        document: Vec<u8>,
    ) -> Result<ProjectImportSummary, BackendCommandError> {
        self.require_catalog_ready()?;
        if document.is_empty() {
            return Err(BackendCommandError::invalid(
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

    fn require_catalog_ready(&self) -> Result<(), BackendCommandError> {
        match self.state.bootstrap().catalog_unavailable() {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    fn require_import_idle(&self, subject: &str) -> Result<(), BackendCommandError> {
        let hardware = self.state.hardware().state();
        let connected = hardware
            .connections()
            .values()
            .any(|connection| connection.connected());
        if self.import_activity_is_busy(&hardware) || connected {
            return Err(BackendCommandError::conflict(format!(
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
