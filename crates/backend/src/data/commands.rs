//! Data settings, catalog, query, and transfer commands exposed to delivery adapters.

use spiral_robotics_common::native_datasets::{
    NativeDatasetInspection, NativeDatasetSelection, NativeDatasetSummary,
};
use std::sync::Arc;

use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
    ProjectImportSummary,
};
use spiral_robotics_common::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_common::settings::{ConnectionTest, DataSettings, SettingsUpdate};
use spiral_robotics_hardware::{HardwareState, ModeSelection as HardwareModeSelection};
use spiral_robotics_storage::ingestion::HuggingFaceDatasetDiscovery as _;

use crate::BackendCommandError;
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease};
use crate::data::query::QueryCommand;
use crate::data::settings::UpdateDataSettingsCommand;
use crate::project_session::context::ProjectContext;

pub(crate) const MAX_PROJECT_IMPORT_BYTES: usize =
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

/// Project-bound data operations, independent of HTTP delivery.
#[derive(Clone)]
pub(crate) struct DataCommands {
    state: ProjectContext,
}

/// Settings values admitted together by the data command facade.
impl DataCommands {
    pub(crate) async fn native_datasets(
        &self,
    ) -> Result<Vec<NativeDatasetSummary>, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move {
                let _activity = commands.state.admit()?;
                commands
                    .state
                    .storage()
                    .native_datasets()
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn inspect_native_dataset(
        &self,
        selection: NativeDatasetSelection,
    ) -> Result<NativeDatasetInspection, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move {
                let _activity = commands.state.admit()?;
                commands
                    .state
                    .storage()
                    .inspect_native_dataset(selection)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }
    pub(crate) async fn settings(&self) -> Result<DataSettings, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move { commands.settings_in_project().await })
            .await?
    }

    async fn settings_in_project(&self) -> Result<DataSettings, BackendCommandError> {
        Ok(self.state.project().settings())
    }

    pub(crate) async fn update_settings(
        &self,
        change: SettingsUpdate,
    ) -> Result<DataSettings, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move { commands.update_settings_in_project(change).await })
            .await?
    }

    async fn update_settings_in_project(
        &self,
        change: SettingsUpdate,
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

    pub(crate) async fn test_spiral(&self) -> Result<ConnectionTest, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move { commands.test_spiral_in_project().await })
            .await?
    }

    async fn test_spiral_in_project(&self) -> Result<ConnectionTest, BackendCommandError> {
        match self.state.projects().recover(self.state.project()).await {
            Ok(()) => {
                self.spawn_auto_connect_saved_robots();
                Ok(ConnectionTest::new(
                    true,
                    "Connected to SpiralDB and loaded the project".to_owned(),
                ))
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) async fn test_training_runtime(
        &self,
    ) -> Result<ConnectionTest, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move { commands.test_training_runtime_in_project().await })
            .await?
    }

    async fn test_training_runtime_in_project(
        &self,
    ) -> Result<ConnectionTest, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(ConnectionTest::from(
            self.state.training().test_runtime().await,
        ))
    }

    pub(crate) async fn relations(&self) -> Result<RelationCatalog, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move { commands.relations_in_project().await })
            .await?
    }

    async fn relations_in_project(&self) -> Result<RelationCatalog, BackendCommandError> {
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

    pub(crate) async fn query(
        &self,
        statement: String,
        max_rows: usize,
    ) -> Result<SpiralQuerySnapshot, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move { commands.query_in_project(statement, max_rows).await })
            .await?
    }

    async fn query_in_project(
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

    pub(crate) async fn inspect_hugging_face_dataset(
        &self,
        request: HuggingFaceDatasetSource,
    ) -> Result<HuggingFaceDatasetInspection, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move {
                commands
                    .inspect_hugging_face_dataset_in_project(request)
                    .await
            })
            .await?
    }

    async fn inspect_hugging_face_dataset_in_project(
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

    pub(crate) async fn import_hugging_face_dataset(
        &self,
        request: HuggingFaceDatasetSource,
    ) -> Result<HuggingFaceDatasetImportSummary, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move {
                commands
                    .import_hugging_face_dataset_in_project(request)
                    .await
            })
            .await?
    }

    async fn import_hugging_face_dataset_in_project(
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
        self.state
            .tasks()
            .execute(async move { commands.export_project_in_project().await })
            .await?
    }

    async fn export_project_in_project(&self) -> Result<Vec<u8>, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.require_catalog_ready()?;
        self.state
            .storage()
            .export_project_bundle()
            .await
            .map_err(BackendCommandError::from)
    }

    pub(crate) async fn import_project(
        &self,
        document: Vec<u8>,
    ) -> Result<ProjectImportSummary, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move { commands.import_project_in_project(document).await })
            .await?
    }

    async fn import_project_in_project(
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
        tokio::spawn(async move {
            crate::robots::commands::DeviceCommands::from(reconnect_state)
                .auto_connect_saved_robots()
                .await
        });
    }
}

impl From<crate::project_session::context::ProjectContext> for DataCommands {
    fn from(state: crate::project_session::context::ProjectContext) -> Self {
        Self { state }
    }
}
