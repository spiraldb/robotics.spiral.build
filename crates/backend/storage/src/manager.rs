//! One project's storage: its Spiral database and its Studio rows.
//!
//! Each feature's persistence is reached through its own accessor; dataset,
//! ingestion, and recording operations extend this type in their modules.

use std::sync::Arc;

use spiral_robotics_common::datasets::{
    DatasetJointRoles, DatasetJointRolesSave, DatasetRobot, DatasetRobotSave, DatasetSummary,
};

use crate::curations::{CuratedViews, CurationPersistence};
use crate::db::ProjectDatabasePersistence;
use crate::db::ProjectQueries;
use crate::db::ProjectStore;
use crate::training::{PublishedModels, TrainingRunPersistence};
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

    pub fn training_runs(&self) -> TrainingRunPersistence<'_> {
        TrainingRunPersistence::new(&self.studio)
    }

    pub fn published_models(&self) -> PublishedModels<'_> {
        PublishedModels::new(&self.studio)
    }

    pub fn curations(&self) -> CurationPersistence<'_> {
        CurationPersistence::new(&self.studio)
    }

    pub fn curated_views(&self) -> CuratedViews<'_> {
        CuratedViews::new(&self.studio, &self.project_store)
    }

    /// The robot saved for `dataset`'s version.
    pub async fn dataset_robot(
        &self,
        dataset: &DatasetSummary,
    ) -> Result<Option<DatasetRobot>, String> {
        self.studio.dataset_robot(dataset.identity()).await
    }

    /// Save the robot the browser resolved for the dataset version it read.
    pub async fn save_dataset_robot(&self, save: DatasetRobotSave) -> Result<(), String> {
        let (dataset, robot) = save.into_parts();
        self.studio
            .save_dataset_robot(dataset.identity(), robot.as_ref())
            .await
    }

    /// The joint roles resolved for `dataset`'s version; `None` until the
    /// browser has resolved them.
    pub async fn dataset_joint_roles(
        &self,
        dataset: &DatasetSummary,
    ) -> Result<Option<DatasetJointRoles>, String> {
        self.studio.dataset_joint_roles(dataset.identity()).await
    }

    /// Save the joint roles the browser resolved for the dataset version it
    /// read.
    pub async fn save_dataset_joint_roles(
        &self,
        save: DatasetJointRolesSave,
    ) -> Result<(), String> {
        let (dataset, roles) = save.into_parts();
        self.studio
            .save_dataset_joint_roles(dataset.identity(), &roles)
            .await
    }
}
