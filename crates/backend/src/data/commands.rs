//! Data settings, catalog, query, and transfer commands exposed to delivery adapters.

use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetSelection, DatasetSummary, ProjectResource,
};
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
    ProjectImportSummary,
};
use spiral_robotics_common::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_common::settings::{
    ConnectionTest, DataSettings, SettingsUpdate, SpiralSettings,
};
use spiral_robotics_storage::ingestion::HuggingFaceDatasetDiscovery as _;

use crate::BackendCommandError;
use crate::data::query::QueryCommand;
use crate::data::settings::UpdateDataSettingsCommand;
use crate::projects::OpenProject;
use crate::projects::context::ProjectContext;
use std::sync::Arc;

pub(crate) const MAX_PROJECT_IMPORT_BYTES: usize =
    spiral_robotics_storage::bundle::ProjectBundle::MAX_BYTES;

/// Project-bound data operations, independent of HTTP delivery.
#[derive(Clone)]
pub(crate) struct DataCommands {
    state: ProjectContext,
}

/// Settings values admitted together by the data command facade.
impl DataCommands {
    pub(super) async fn dataset_robot(
        &self,
        selection: DatasetSelection,
    ) -> Result<Option<spiral_robotics_common::datasets::DatasetRobot>, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move {
                let storage = commands.state.storage();
                let inspection = storage.inspect_dataset(selection).await?;
                let dataset = inspection.dataset();
                Ok(storage
                    .dataset_robot((dataset.resource_id(), dataset.version_id()))
                    .await?)
            })
            .await?
    }
    /// Persist the robot the browser resolved for a dataset's current version.
    pub(super) async fn save_dataset_robot(
        &self,
        save: spiral_robotics_common::datasets::DatasetRobotSave,
    ) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move {
                let (selection, robot) = save.into_parts();
                let storage = commands.state.storage();
                let inspection = storage.inspect_dataset(selection).await?;
                let dataset = inspection.dataset();
                Ok(storage
                    .save_dataset_robot(
                        (dataset.resource_id(), dataset.version_id()),
                        robot.as_ref(),
                    )
                    .await?)
            })
            .await?
    }
    pub(super) async fn project_resources(
        &self,
    ) -> Result<Vec<ProjectResource>, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move {
                commands
                    .state
                    .storage()
                    .project_resources()
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(super) async fn datasets(&self) -> Result<Vec<DatasetSummary>, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move {
                commands
                    .state
                    .storage()
                    .datasets()
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(super) async fn inspect_dataset(
        &self,
        selection: DatasetSelection,
    ) -> Result<DatasetInspection, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move {
                commands
                    .state
                    .storage()
                    .inspect_dataset(selection)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(super) async fn episode_topics(
        &self,
        request: spiral_robotics_common::datasets::EpisodeSelection,
    ) -> Result<Vec<spiral_robotics_common::datasets::DatasetTopic>, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move {
                commands
                    .state
                    .storage()
                    .episode_topics(request)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(super) async fn episode_coverage(
        &self,
        request: spiral_robotics_common::datasets::EpisodeCoverageRequest,
    ) -> Result<Vec<spiral_robotics_common::datasets::StreamCoverage>, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move {
                commands
                    .state
                    .storage()
                    .episode_coverage(request)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(super) async fn update_connection(
        &self,
        change: spiral_robotics_common::settings::SpiralConnectionUpdate,
    ) -> Result<SpiralSettings, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move {
                let origin = commands.state.project();
                let project = commands
                    .state
                    .projects()
                    .update_connection(origin, change)
                    .await?;
                if !Arc::ptr_eq(&project, origin) {
                    commands.spawn_auto_connect_saved_robots(project);
                }
                Ok(commands.state.projects().saved_settings())
            })
            .await?
    }

    pub(super) async fn settings(&self) -> Result<DataSettings, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move { commands.settings_in_project().await })
            .await?
    }

    async fn settings_in_project(&self) -> Result<DataSettings, BackendCommandError> {
        self.state.project().settings().await.map_err(Into::into)
    }

    pub(super) async fn update_settings(
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
        let (endpoint, spfs, project, client_id, secret, clear_secret, huggingface_token) =
            connection.into_parts();
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
            .with_huggingface_token(huggingface_token)
            .with_video_enabled(video.enabled());
        let result = self
            .state
            .projects()
            .update_settings(self.state.project(), command)
            .await?;
        let (settings, switched_to) = result.into_parts();
        if let Some(project) = switched_to {
            self.spawn_auto_connect_saved_robots(project);
        }
        Ok(settings)
    }

    pub(super) async fn test_spiral(&self) -> Result<ConnectionTest, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move { commands.test_spiral_in_project().await })
            .await?
    }

    async fn test_spiral_in_project(&self) -> Result<ConnectionTest, BackendCommandError> {
        Ok(ConnectionTest::from(
            self.state.storage().test_spiral().await,
        ))
    }

    pub(super) async fn test_training_runtime(
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
        Ok(ConnectionTest::from(
            self.state.training().test_runtime().await,
        ))
    }

    pub(super) async fn relations(&self) -> Result<RelationCatalog, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move { commands.relations_in_project().await })
            .await?
    }

    async fn relations_in_project(&self) -> Result<RelationCatalog, BackendCommandError> {
        self.state
            .storage()
            .relation_catalog()
            .await
            .map_err(Into::into)
    }

    pub(super) async fn query(
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

    /// One exact dataset member's read-only query, with `$1` bound to its UUID.
    pub(super) async fn query_for_episode(
        &self,
        request: spiral_robotics_common::relations::EpisodeQueryRequest,
    ) -> Result<SpiralQuerySnapshot, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move {
                let command =
                    QueryCommand::new(request.statement(), request.max_rows()).validated()?;
                let request = spiral_robotics_common::relations::EpisodeQueryRequest::new(
                    request.episode().clone(),
                    command.statement().to_owned(),
                    command.max_rows(),
                );
                commands
                    .state
                    .storage()
                    .query_for_episode(&request)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    /// The same episode query as one complete Arrow IPC stream.
    pub(super) async fn query_for_episode_arrow(
        &self,
        request: spiral_robotics_common::relations::EpisodeQueryRequest,
    ) -> Result<Vec<u8>, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move {
                commands
                    .state
                    .storage()
                    .query_for_episode_arrow(&request)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    async fn query_in_project(
        &self,
        statement: String,
        max_rows: usize,
    ) -> Result<SpiralQuerySnapshot, BackendCommandError> {
        let command = QueryCommand::new(statement, max_rows).validated()?;
        self.state
            .storage()
            .query(command.statement(), command.max_rows())
            .await
            .map_err(Into::into)
    }

    pub(super) async fn inspect_hugging_face_dataset(
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
        HuggingFaceDatasetInspection::discover(request)
            .await
            .map_err(BackendCommandError::from)
    }

    pub(super) async fn import_hugging_face_dataset(
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
        let inspection = HuggingFaceDatasetInspection::discover(request)
            .await
            .map_err(BackendCommandError::from)?;
        self.state
            .storage()
            .import_huggingface_dataset(inspection)
            .await
            .map_err(BackendCommandError::from)
    }

    pub(super) async fn export_project(&self) -> Result<Vec<u8>, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .execute(async move { commands.export_project_in_project().await })
            .await?
    }

    async fn export_project_in_project(&self) -> Result<Vec<u8>, BackendCommandError> {
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
        if document.is_empty() {
            return Err(BackendCommandError::invalid(
                "choose a .spiral-project file to import",
            ));
        }
        let (summary, imported) = self
            .state
            .projects()
            .import(self.state.project(), &document)
            .await?;
        self.spawn_auto_connect_saved_robots(imported);
        Ok(summary)
    }

    fn spawn_auto_connect_saved_robots(&self, project: Arc<OpenProject>) {
        let project = self.state.with_project(project);
        tokio::spawn(async move {
            crate::robots::commands::DeviceCommands::from(project)
                .auto_connect_saved_robots()
                .await
        });
    }
}

impl From<crate::projects::context::ProjectContext> for DataCommands {
    fn from(state: crate::projects::context::ProjectContext) -> Self {
        Self { state }
    }
}
