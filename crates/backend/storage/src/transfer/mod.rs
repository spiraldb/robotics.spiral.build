//! Portable transfer and external dataset ingestion.

mod bundle;
mod ingestion;

pub use bundle::ProjectBundle;
pub use ingestion::HuggingFaceDatasetDiscovery;

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

    pub async fn export_project(&self) -> Result<Vec<u8>, String> {
        let mut connection = self.project_store.connect_exclusive().await?;
        let datasets =
            crate::datasets::McapSources::validate_ownership(self.project_store, &mut connection)
                .await?;
        ProjectBundle
            .export(
                &mut connection,
                ProjectBundleExport::new(self.project_store.table_prefix(), self.studio, datasets),
            )
            .await
    }

    pub async fn import_project(&self, bytes: &[u8]) -> Result<ProjectImportSummary, String> {
        let source =
            ProjectBundleImport::new(self.project_store.table_prefix(), self.studio, bytes)?;
        for name in source.dataset_names() {
            self.project_store.validate_mcap_destination(name).await?;
        }
        let mut connection = self.project_store.connect_exclusive().await?;
        let datasets =
            crate::datasets::McapSources::validate_ownership(self.project_store, &mut connection)
                .await?;
        ProjectBundle
            .import(&mut connection, source.with_current_datasets(datasets))
            .await
    }
}
