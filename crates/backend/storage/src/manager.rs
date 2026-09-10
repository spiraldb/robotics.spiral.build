//! Thin composition root for storage-owned feature persistence.
//!
//! Each public operation delegates to the feature that owns its rows or to a
//! database/transfer mechanism. This module owns only the stable facade used
//! by the backend.

use std::sync::Arc;

use spiral_robotics_common::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_common::rollout::RolloutSnapshot;
use spiral_robotics_common::training::{TrainingRun, TrainingSnapshot};

use crate::db::ProjectDatabasePersistence;
use crate::db::ProjectQueries;
use crate::db::ProjectStore;
use crate::rollout_runs::RolloutSessionPersistence;
use crate::settings::{
    SpiralConnectionSettings, StoredStudioPreferences, StudioPreferencePersistence,
};
use crate::training::TrainingHistoryPersistence;
use crate::transfer::TransferPersistence;
use crate::workflows::{WorkflowJson, WorkflowPersistence};

pub struct StorageManager {
    project_store: Arc<ProjectStore>,
}

impl StorageManager {
    pub fn new(project_store: Arc<ProjectStore>) -> Self {
        Self { project_store }
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
        TrainingHistoryPersistence::new(&self.project_store)
            .load()
            .await
    }

    /// Replace the complete training history.
    ///
    /// All three relations are replaced inside one transaction so a run row
    /// and its metric or checkpoint rows can never disagree about which runs
    /// are retained.
    pub async fn save_training_history(&self, snapshot: &TrainingSnapshot) -> Result<(), String> {
        TrainingHistoryPersistence::new(&self.project_store)
            .save(snapshot)
            .await
    }

    /// Read the one durable rollout session.
    pub async fn load_rollout_session(&self) -> Result<Option<RolloutSnapshot>, String> {
        RolloutSessionPersistence::new(&self.project_store)
            .load()
            .await
    }

    /// Replace the durable rollout session. A snapshot that never named a
    /// session clears the relation, so a project that has never run a policy
    /// and a project whose history was cleared read back identically.
    pub async fn save_rollout_session(&self, snapshot: &RolloutSnapshot) -> Result<(), String> {
        RolloutSessionPersistence::new(&self.project_store)
            .save(snapshot)
            .await
    }

    pub async fn workflows(
        &self,
    ) -> Result<Vec<spiral_robotics_common::workflow::WorkflowSummary>, String> {
        WorkflowPersistence::new(&self.project_store).list().await
    }

    pub async fn load_workflow_graph(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowJson>, String> {
        WorkflowPersistence::new(&self.project_store)
            .load_graph(workflow_id)
            .await
    }

    pub async fn save_workflow_graph(
        &self,
        workflow: &spiral_robotics_common::workflow::WorkflowSpec,
        graph: &WorkflowJson,
    ) -> Result<(), String> {
        WorkflowPersistence::new(&self.project_store)
            .save_graph(workflow, graph)
            .await
    }

    pub async fn load_workflow_conversation(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowJson>, String> {
        WorkflowPersistence::new(&self.project_store)
            .load_assistant(workflow_id)
            .await
    }

    pub async fn save_workflow_conversation(
        &self,
        workflow_id: &str,
        conversation: &WorkflowJson,
    ) -> Result<(), String> {
        WorkflowPersistence::new(&self.project_store)
            .save_assistant(workflow_id, conversation)
            .await
    }

    pub async fn load_studio_preferences(&self) -> Result<StoredStudioPreferences, String> {
        StudioPreferencePersistence::new(&self.project_store)
            .load()
            .await
    }

    pub async fn save_studio_preferences(
        &self,
        preferences: &StoredStudioPreferences,
    ) -> Result<(), String> {
        StudioPreferencePersistence::new(&self.project_store)
            .save(preferences)
            .await
    }

    pub async fn export_project_bundle(&self) -> Result<Vec<u8>, String> {
        TransferPersistence::new(&self.project_store)
            .export_project()
            .await
    }

    pub async fn import_project_bundle(
        &self,
        bytes: &[u8],
    ) -> Result<spiral_robotics_common::datasets::ProjectImportSummary, String> {
        TransferPersistence::new(&self.project_store)
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

    pub async fn relation_catalog(&self) -> Result<RelationCatalog, String> {
        ProjectQueries::new(&self.project_store)
            .relation_catalog()
            .await
    }
}
