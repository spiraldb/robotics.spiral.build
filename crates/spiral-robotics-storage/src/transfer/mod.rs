//! Portable transfer and external dataset ingestion.

mod bundle;
mod ingestion;

pub(crate) use bundle::ManagedRelationCatalog;
pub use bundle::{
    EpisodeBundle, ProjectBundle, SIGLIP_BASE_MODEL_ID, SIGLIP_BASE_REPOSITORY,
    SIGLIP_BASE_REVISION,
};
pub use ingestion::HuggingFaceDatasetDiscovery;

use spiral_robotics_api::datasets::{EpisodeImportSummary, ProjectImportSummary};

use crate::db::ProjectStore;
use bundle::{BundleSource, EpisodeBundleImport, EpisodeBundleSelection};

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
            .export(&mut connection, self.project_store.namespace())
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
                BundleSource::new(self.project_store.namespace(), bytes),
            )
            .await
    }

    pub(crate) async fn import_episodes(
        &self,
        dataset_id: &str,
        bytes: &[u8],
    ) -> Result<EpisodeImportSummary, String> {
        let mut connection = self.project_store.connect_exclusive().await?;
        EpisodeBundle
            .import(
                &mut connection,
                EpisodeBundleImport::new(
                    EpisodeBundleSelection::new(self.project_store.namespace(), dataset_id),
                    bytes,
                ),
            )
            .await
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn export_episodes(&self, dataset_id: &str) -> Result<Vec<u8>, String> {
        let mut connection = self.project_store.connect_exclusive().await?;
        EpisodeBundle
            .export(
                &mut connection,
                EpisodeBundleSelection::new(self.project_store.namespace(), dataset_id),
            )
            .await
    }
}
