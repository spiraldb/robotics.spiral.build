//! Thin composition root for storage-owned feature persistence.
//!
//! Each public operation delegates to the feature that owns its rows or to a
//! database/transfer mechanism. This module owns only the stable facade used
//! by the backend.

use std::sync::Arc;

use spiral_robotics_common::recording::DatasetSummary;
use spiral_robotics_common::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_common::rollout::RolloutSnapshot;
use spiral_robotics_common::training::{TrainingRun, TrainingSnapshot};

use crate::db::ProjectDatabasePersistence;
use crate::db::ProjectQueries;
use crate::db::ProjectStore;
use crate::episodes::video::VideoAssetPersistence;
use crate::episodes::video::{VideoAsset, VideoAssetKey, VideoByteRange};
use crate::episodes::{EpisodePersistence, FullEpisode};
use crate::rollout_runs::RolloutSessionPersistence;
use crate::settings::{
    SpiralConnectionSettings, StoredStudioPreferences, StudioPreferencePersistence,
};
use crate::training::TrainingHistoryPersistence;
use crate::transfer::TransferPersistence;
use crate::workflow_documents::{StoredWorkflowDocument, WorkflowDocumentPersistence};

pub struct StorageManager {
    project_store: Arc<ProjectStore>,
}

impl StorageManager {
    pub async fn legacy_recording_available(&self) -> Result<bool, String> {
        let mut connection = self.project_store.connect().await?;
        for table in crate::episodes::EpisodeRelations::RELATIONS {
            if !connection
                .table_exists(&table.qualified_name(self.namespace()))
                .await?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
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

    pub fn workflow_model_store(&self) -> Arc<ProjectStore> {
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

    pub async fn prewarm(&self) -> Result<(), String> {
        ProjectDatabasePersistence::new(&self.project_store)
            .prewarm()
            .await
    }

    pub fn namespace(&self) -> &str {
        self.project_store.namespace()
    }

    /// Persist one complete episode snapshot and its finalized media directly
    /// to SpiralDB. A later pause or stop replaces the same episode
    /// idempotently with the newer complete snapshot.
    pub async fn commit_episode(
        &self,
        episode: &FullEpisode,
        videos: &[VideoAsset],
    ) -> Result<(), String> {
        EpisodePersistence::new(&self.project_store)
            .commit(episode, videos)
            .await
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

    pub async fn load_workflow_document(&self) -> Result<Option<StoredWorkflowDocument>, String> {
        WorkflowDocumentPersistence::new(&self.project_store)
            .load_graph()
            .await
    }

    pub async fn save_workflow_document(
        &self,
        document: &StoredWorkflowDocument,
    ) -> Result<(), String> {
        WorkflowDocumentPersistence::new(&self.project_store)
            .save_graph(document)
            .await
    }

    pub async fn load_workflow_agent_document(
        &self,
    ) -> Result<Option<StoredWorkflowDocument>, String> {
        WorkflowDocumentPersistence::new(&self.project_store)
            .load_assistant()
            .await
    }

    pub async fn save_workflow_agent_document(
        &self,
        document: &StoredWorkflowDocument,
    ) -> Result<(), String> {
        WorkflowDocumentPersistence::new(&self.project_store)
            .save_assistant(document)
            .await
    }

    pub async fn reset_workflow_agent_document(
        &self,
        expected_payload: &str,
        document: &StoredWorkflowDocument,
    ) -> Result<(), String> {
        WorkflowDocumentPersistence::new(&self.project_store)
            .reset_assistant(expected_payload, document)
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

    pub async fn datasets(&self) -> Result<Vec<DatasetSummary>, String> {
        ProjectQueries::new(&self.project_store).datasets().await
    }

    pub async fn relation_catalog(&self) -> Result<RelationCatalog, String> {
        ProjectQueries::new(&self.project_store)
            .relation_catalog()
            .await
    }

    /// Read the authoritative MP4 size without materializing its bytes.
    pub async fn video_asset_size(&self, asset: &VideoAssetKey) -> Result<u64, String> {
        VideoAssetPersistence::new(&self.project_store)
            .size(asset)
            .await
    }

    /// Fetch exactly one browser-requested range from Spiral's managed Blob.
    ///
    /// The managed MP4 output retains the source Blob, including its provider
    /// URI and object offset. `spql.io.read_bytes` can therefore push the range
    /// into object storage instead of materializing the complete MP4 in Studio.
    pub async fn video_asset_range(
        &self,
        asset: &VideoAssetKey,
        range: VideoByteRange,
    ) -> Result<Vec<u8>, String> {
        VideoAssetPersistence::new(&self.project_store)
            .range(asset, range)
            .await
    }
}
