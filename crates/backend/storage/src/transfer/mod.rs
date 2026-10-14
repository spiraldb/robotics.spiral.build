//! Portable transfer and external dataset ingestion.

mod bundle;
mod ingestion;

pub use bundle::ProjectBundle;
pub use ingestion::{HuggingFaceDatasetDiscovery, HuggingFaceImport};

use spiral_robotics_common::datasets::ProjectImportSummary;

use crate::db::ProjectStore;
use bundle::{ProjectBundleExport, ProjectBundleImport};

/// Portable transfer operations for one project.
pub struct TransferPersistence<'a> {
    project_store: &'a ProjectStore,
    studio: &'a crate::StudioProject,
}

impl<'a> TransferPersistence<'a> {
    pub(crate) const fn new(
        project_store: &'a ProjectStore,
        studio: &'a crate::StudioProject,
    ) -> Self {
        Self {
            project_store,
            studio,
        }
    }

    /// The project's Studio records and every MCAP dataset's files.
    pub async fn export_project(&self) -> Result<Vec<u8>, String> {
        let datasets = self
            .project_store
            .dataset_catalog()
            .datasets()
            .await?
            .mcap();
        let mut connection = self.project_store.connect_exclusive().await?;
        ProjectBundle
            .export(
                &mut connection,
                ProjectBundleExport::new(self.studio, datasets),
            )
            .await
    }

    /// Replace the project's Studio records and every one of its MCAP
    /// datasets with the bundle's, in one Spiral transaction. A LeRobot
    /// dataset stays as it is, so a bundle may not name one.
    pub async fn import_project(&self, bytes: &[u8]) -> Result<ProjectImportSummary, String> {
        let source = ProjectBundleImport::new(self.studio, bytes)?;
        let catalog = self.project_store.dataset_catalog().datasets().await?;
        let current = catalog.mcap();
        if let Some(dataset) = source
            .datasets()
            .iter()
            .find(|dataset| catalog.holds(dataset) && !current.contains(dataset))
        {
            return Err(format!(
                "{} is no MCAP dataset in this project; delete it before importing a bundle that holds one of that name",
                dataset.label()
            ));
        }
        let mut replaced = Vec::new();
        for dataset in &current {
            replaced.extend(catalog.tables(dataset)?);
        }
        let mut connection = self.project_store.connect_exclusive().await?;
        ProjectBundle
            .import(&mut connection, source.replacing(replaced))
            .await
    }
}
