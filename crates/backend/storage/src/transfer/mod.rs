//! Portable transfer and external dataset ingestion.

mod bundle;
mod ingestion;

pub(crate) use bundle::ManagedRelationCatalog;
pub use bundle::ProjectBundle;
pub use ingestion::HuggingFaceDatasetDiscovery;

use spiral_robotics_common::datasets::ProjectImportSummary;

use crate::db::ProjectStore;
use bundle::ProjectBundleImport;

/// Portable transfer operations for one project.
pub(crate) struct TransferPersistence<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> TransferPersistence<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    pub(crate) async fn export_project(&self) -> Result<Vec<u8>, String> {
        let mut connection = self.project_store.connect_exclusive().await?;
        ProjectBundle
            .export(&mut connection, self.project_store.table_prefix())
            .await
    }

    pub(crate) async fn import_project(
        &self,
        bytes: &[u8],
    ) -> Result<ProjectImportSummary, String> {
        let mut connection = self.project_store.connect_exclusive().await?;
        ProjectBundle
            .import(
                &mut connection,
                ProjectBundleImport::new(self.project_store.table_prefix(), bytes),
            )
            .await
    }
}
