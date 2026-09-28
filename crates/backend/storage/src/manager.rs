//! One project's storage: its Spiral database and its Studio rows.
//!
//! Each feature's persistence is reached through its own accessor; dataset,
//! ingestion, and recording operations extend this type in their modules.

use std::sync::Arc;

use spiral_robotics_common::datasets::{
    DatasetCheckLookup, DatasetCheckSave, DatasetJointRoles, DatasetJointRolesSave, DatasetRobot,
    DatasetRobotSave, DatasetSelection,
};

use crate::curations::{CuratedViews, CurationPersistence};
use crate::db::ProjectDatabasePersistence;
use crate::db::ProjectQueries;
use crate::db::ProjectStore;
use crate::settings::StudioPreferencePersistence;
use crate::training::{TrainingCheckpointPersistence, TrainingRunPersistence};
use crate::transfer::TransferPersistence;

pub struct StorageManager {
    project_store: Arc<ProjectStore>,
    studio: crate::StudioProject,
}

impl StorageManager {
    pub fn new(project_store: Arc<ProjectStore>, studio: crate::StudioProject) -> Self {
        Self {
            project_store,
            studio,
        }
    }

    /// The project's Spiral connection and its table names.
    pub fn project_store(&self) -> &ProjectStore {
        &self.project_store
    }

    pub fn database(&self) -> ProjectDatabasePersistence<'_> {
        ProjectDatabasePersistence::new(&self.project_store)
    }

    pub fn queries(&self) -> ProjectQueries<'_> {
        ProjectQueries::new(&self.project_store)
    }

    pub fn transfer(&self) -> TransferPersistence<'_> {
        TransferPersistence::new(&self.project_store, &self.studio)
    }

    pub fn preferences(&self) -> StudioPreferencePersistence<'_> {
        StudioPreferencePersistence::new(&self.studio)
    }

    pub fn training_runs(&self) -> TrainingRunPersistence<'_> {
        TrainingRunPersistence::new(&self.studio)
    }

    pub fn training_checkpoints(&self) -> TrainingCheckpointPersistence<'_> {
        TrainingCheckpointPersistence::new(&self.project_store)
    }

    pub fn curations(&self) -> CurationPersistence<'_> {
        CurationPersistence::new(&self.studio)
    }

    pub fn curated_views(&self) -> CuratedViews<'_> {
        CuratedViews::new(&self.project_store)
    }

    /// The robot saved for the dataset version `selection` resolves to now.
    pub async fn dataset_robot(
        &self,
        selection: DatasetSelection,
    ) -> Result<Option<DatasetRobot>, String> {
        let inspection = self.inspect_dataset(selection).await?;
        let dataset = inspection.dataset();
        self.studio
            .dataset_robot((dataset.resource_id(), dataset.version_id()))
            .await
    }

    /// Save the robot the browser resolved for the dataset's current version.
    pub async fn save_dataset_robot(&self, save: DatasetRobotSave) -> Result<(), String> {
        let (selection, robot) = save.into_parts();
        let inspection = self.inspect_dataset(selection).await?;
        let dataset = inspection.dataset();
        self.studio
            .save_dataset_robot(
                (dataset.resource_id(), dataset.version_id()),
                robot.as_ref(),
            )
            .await
    }

    /// The joint roles resolved for the dataset version `selection`
    /// resolves to now; `None` until the browser has resolved them.
    pub async fn dataset_joint_roles(
        &self,
        selection: DatasetSelection,
    ) -> Result<Option<DatasetJointRoles>, String> {
        let inspection = self.inspect_dataset(selection).await?;
        let dataset = inspection.dataset();
        self.studio
            .dataset_joint_roles((dataset.resource_id(), dataset.version_id()))
            .await
    }

    /// Save the joint roles the browser resolved for the dataset's current
    /// version.
    pub async fn save_dataset_joint_roles(
        &self,
        save: DatasetJointRolesSave,
    ) -> Result<(), String> {
        let (selection, roles) = save.into_parts();
        let inspection = self.inspect_dataset(selection).await?;
        let dataset = inspection.dataset();
        self.studio
            .save_dataset_joint_roles((dataset.resource_id(), dataset.version_id()), &roles)
            .await
    }

    /// The check report for the dataset version `lookup` resolves to now,
    /// from checks at its definition.
    pub async fn dataset_check_report(
        &self,
        lookup: DatasetCheckLookup,
    ) -> Result<Option<serde_json::Value>, String> {
        let (selection, definition) = lookup.into_parts();
        let inspection = self.inspect_dataset(selection).await?;
        let dataset = inspection.dataset();
        self.studio
            .dataset_check_report((dataset.resource_id(), dataset.version_id()), definition)
            .await
    }

    /// Keep the report the browser computed for the dataset's current version.
    pub async fn save_dataset_check_report(&self, save: DatasetCheckSave) -> Result<(), String> {
        let (lookup, report) = save.into_parts();
        let (selection, definition) = lookup.into_parts();
        let inspection = self.inspect_dataset(selection).await?;
        let dataset = inspection.dataset();
        self.studio
            .save_dataset_check_report(
                (dataset.resource_id(), dataset.version_id()),
                definition,
                report,
            )
            .await
    }
}
