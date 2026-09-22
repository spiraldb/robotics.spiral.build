//! Thin composition root for storage-owned feature persistence.
//!
//! Each public operation delegates to the feature that owns its rows or to a
//! database/transfer mechanism. This module owns only the stable facade used
//! by the backend.

use std::sync::Arc;

use spiral_robotics_common::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_common::rollout::RolloutSnapshot;
use spiral_robotics_common::training::{TrainingRun, TrainingSnapshot};

use crate::curations::{CurationPersistence, StoredCurationDocument};
use crate::db::ProjectDatabasePersistence;
use crate::db::ProjectQueries;
use crate::db::ProjectStore;
use crate::explorations::ExplorationPersistence;
use crate::rollout_runs::RolloutSessionPersistence;
use crate::settings::{
    SpiralConnectionSettings, StoredStudioPreferences, StudioPreferencePersistence,
};
use crate::training::TrainingHistoryPersistence;
use crate::transfer::TransferPersistence;
use crate::workflows::{StoredWorkflowDocument, WorkflowJson, WorkflowPersistence};

pub struct StorageManager {
    project_store: Arc<ProjectStore>,
    studio: crate::StudioProject,
}

impl StorageManager {
    pub async fn dataset_robot(
        &self,
        identity: (&str, &str),
    ) -> Result<Option<spiral_robotics_common::datasets::DatasetRobot>, String> {
        self.studio.dataset_robot(identity).await
    }
    pub async fn save_dataset_robot(
        &self,
        identity: (&str, &str),
        robot: Option<&spiral_robotics_common::datasets::DatasetRobot>,
    ) -> Result<(), String> {
        self.studio.save_dataset_robot(identity, robot).await
    }
    pub fn new(project_store: Arc<ProjectStore>, studio: crate::StudioProject) -> Self {
        Self {
            project_store,
            studio,
        }
    }

    pub(crate) fn project_store(&self) -> &ProjectStore {
        &self.project_store
    }

    pub fn project_id(&self) -> String {
        self.project_store.project_id()
    }

    pub fn connection_settings(&self) -> SpiralConnectionSettings {
        self.project_store.connection_settings()
    }

    pub fn workflow_store(&self) -> Arc<ProjectStore> {
        self.project_store.clone()
    }

    pub fn spiral_endpoint(&self) -> String {
        self.project_store.endpoint()
    }

    pub fn spiral_spfs_endpoint(&self) -> String {
        self.project_store.spfs_endpoint()
    }

    pub fn spiral_credentials(&self) -> Option<crate::settings::SpiralCredentials> {
        self.project_store.credentials()
    }

    pub fn is_configured(&self) -> bool {
        self.project_store.is_configured()
    }

    pub fn table_prefix(&self) -> &str {
        self.project_store.table_prefix()
    }

    /// Read the durable training history.
    pub async fn load_training_history(&self) -> Result<Vec<TrainingRun>, String> {
        TrainingHistoryPersistence::new(&self.studio).load().await
    }

    /// Persist retained runs and their histories in one SQLite transaction.
    pub async fn save_training_history(&self, snapshot: &TrainingSnapshot) -> Result<(), String> {
        TrainingHistoryPersistence::new(&self.studio)
            .save(snapshot)
            .await
    }

    /// Read the one durable rollout session.
    pub async fn load_rollout_session(&self) -> Result<Option<RolloutSnapshot>, String> {
        RolloutSessionPersistence::new(&self.studio).load().await
    }

    /// Replace the durable rollout session. A snapshot that never named a
    /// session clears the relation, so a project that has never run a policy
    /// and a project whose history was cleared read back identically.
    pub async fn save_rollout_session(&self, snapshot: &RolloutSnapshot) -> Result<(), String> {
        RolloutSessionPersistence::new(&self.studio)
            .save(snapshot)
            .await
    }

    pub async fn workflows(
        &self,
    ) -> Result<Vec<spiral_robotics_common::workflow::WorkflowSummary>, String> {
        WorkflowPersistence::new(&self.studio).list().await
    }

    pub async fn load_workflow(
        &self,
        workflow_id: &str,
    ) -> Result<Option<StoredWorkflowDocument>, String> {
        WorkflowPersistence::new(&self.studio)
            .load(workflow_id)
            .await
    }

    /// Persist one user-requested graph and conversation checkpoint atomically.
    pub async fn save_workflow(
        &self,
        workflow: &spiral_robotics_common::workflow::WorkflowSpec,
        contents: (&WorkflowJson, &WorkflowJson),
    ) -> Result<bool, String> {
        WorkflowPersistence::new(&self.studio)
            .save(workflow, contents)
            .await
    }

    pub async fn curations(
        &self,
    ) -> Result<Vec<spiral_robotics_common::curation::CurationSummary>, String> {
        CurationPersistence::new(&self.studio).list().await
    }

    pub async fn load_curation(
        &self,
        curation_id: &str,
    ) -> Result<Option<StoredCurationDocument>, String> {
        CurationPersistence::new(&self.studio)
            .load(curation_id)
            .await
    }

    /// Persist one saved recipe revision; false when another save won.
    pub async fn save_curation(&self, document: &StoredCurationDocument) -> Result<bool, String> {
        CurationPersistence::new(&self.studio).save(document).await
    }

    pub async fn explorations(
        &self,
    ) -> Result<Vec<spiral_robotics_common::exploration::ExplorationSummary>, String> {
        ExplorationPersistence::new(&self.studio).list().await
    }

    pub async fn load_exploration(
        &self,
        document_id: &str,
    ) -> Result<Option<spiral_robotics_common::exploration::ExplorationDocument>, String> {
        ExplorationPersistence::new(&self.studio)
            .load(document_id)
            .await
    }

    /// Persist one exploration revision; false when another save won.
    pub async fn save_exploration(
        &self,
        document: &spiral_robotics_common::exploration::ExplorationDocument,
    ) -> Result<bool, String> {
        ExplorationPersistence::new(&self.studio)
            .save(document)
            .await
    }

    pub async fn load_studio_preferences(&self) -> Result<StoredStudioPreferences, String> {
        StudioPreferencePersistence::new(&self.studio).load().await
    }

    pub async fn save_studio_preferences(
        &self,
        preferences: &StoredStudioPreferences,
    ) -> Result<(), String> {
        StudioPreferencePersistence::new(&self.studio)
            .save(preferences)
            .await
    }

    pub async fn export_project_bundle(&self) -> Result<Vec<u8>, String> {
        TransferPersistence::new(&self.project_store, &self.studio)
            .export_project()
            .await
    }

    pub async fn import_project_bundle(
        &self,
        bytes: &[u8],
    ) -> Result<spiral_robotics_common::datasets::ProjectImportSummary, String> {
        TransferPersistence::new(&self.project_store, &self.studio)
            .import_project(bytes)
            .await
    }

    pub async fn test_spiral(&self) -> (bool, String) {
        ProjectDatabasePersistence::new(&self.project_store)
            .test()
            .await
    }

    pub async fn query(&self, sql: &str, max_rows: usize) -> Result<SpiralQuerySnapshot, String> {
        ProjectQueries::new(&self.project_store)
            .query(sql, max_rows)
            .await
    }

    pub async fn query_for_episode(
        &self,
        request: &spiral_robotics_common::relations::EpisodeQueryRequest,
    ) -> Result<SpiralQuerySnapshot, String> {
        ProjectQueries::new(&self.project_store)
            .query_for_episode(request)
            .await
    }

    pub async fn relation_catalog(&self) -> Result<RelationCatalog, String> {
        ProjectQueries::new(&self.project_store)
            .relation_catalog()
            .await
    }
}
