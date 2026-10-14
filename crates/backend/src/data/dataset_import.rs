//! Dataset imports from Hugging Face, one held run each: the backend
//! downloads and decodes the repository's episodes and writes them 32 at a
//! time, which takes as long as the dataset is big, while the browser polls
//! the run for how far it has got. The run is the backend's and outlives the
//! page; a restarted backend holds none, and the batches its import committed
//! stay.
use crate::OperationError;
use crate::held_runs::{ABANDONED_AFTER, HeldRuns, HeldWork};
use crate::sync::MutexRecovery;
use spiral_robotics_common::datasets::{
    DatasetFormat, DatasetImportPhase, DatasetImportRequest, DatasetImportStatus,
    HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
};
use spiral_robotics_storage::ingestion::HuggingFaceDatasetDiscovery as _;
use spiral_robotics_storage::{HuggingFaceImport, ProviderKeys, StorageManager};
use std::sync::{Arc, Mutex};
use tokio::time::Instant;

/// The dataset imports one project has started.
pub(crate) struct DatasetImports {
    storage: Arc<StorageManager>,
    keys: Arc<ProviderKeys>,
    runs: HeldRuns<ImportRun>,
}

impl DatasetImports {
    /// `keys` hold the deployment's Hugging Face token, which every download
    /// sends.
    pub(crate) fn new(storage: Arc<StorageManager>, keys: Arc<ProviderKeys>) -> Self {
        Self {
            storage,
            keys,
            runs: HeldRuns::new("dataset-import"),
        }
    }

    /// Start importing the repository `request` names as a dataset of its
    /// format, at the commit its revision resolves to now, and answer the
    /// run's status at once.
    pub(super) async fn start(
        &self,
        request: DatasetImportRequest,
    ) -> Result<DatasetImportStatus, OperationError> {
        let format = request.format();
        let inspection = Self::discover(format, request.into_source())
            .await
            .map_err(OperationError::External)?;
        let import = HuggingFaceImport::new(format, &inspection)
            .map_err(OperationError::External)?
            .with_token(self.keys.huggingface_token());
        let run = Arc::new(ImportRun::new(DatasetImportRequest::new(
            format,
            HuggingFaceDatasetSource::new(inspection.dataset_id(), inspection.repository())
                .with_revision(inspection.resolved_revision()),
        )));
        let storage = self.storage.clone();
        let worked = run.clone();
        let id = self.runs.start(run.clone(), async move {
            let progress = |phase| worked.enter(phase);
            let imported = import.run(&storage, &progress).await;
            worked.enter(match imported {
                Ok(()) => DatasetImportPhase::Done,
                Err(error) => DatasetImportPhase::Failed { error },
            });
        });
        Ok(run.status(&id))
    }

    /// Where run `id` has got.
    pub(super) fn status(&self, id: &str) -> Result<DatasetImportStatus, OperationError> {
        let run = self.runs.get(id)?;
        run.polled();
        Ok(run.status(id))
    }

    /// Abandon every import: the project is closing. The batches each one
    /// committed stay.
    pub(crate) fn shutdown(&self) {
        self.runs.shutdown();
    }

    /// The repository `source` names, read as `format` and pinned to the
    /// commit its revision resolves to now.
    async fn discover(
        format: DatasetFormat,
        source: HuggingFaceDatasetSource,
    ) -> Result<HuggingFaceDatasetInspection, String> {
        match format {
            DatasetFormat::Mcap => HuggingFaceDatasetInspection::discover(source).await,
            DatasetFormat::LeRobot => HuggingFaceDatasetInspection::discover_lerobot(source).await,
        }
    }
}

/// One import: what it imports, and where it has got.
struct ImportRun {
    request: DatasetImportRequest,
    progress: Mutex<ImportProgress>,
}

struct ImportProgress {
    phase: DatasetImportPhase,
    polled: Instant,
}

impl ImportRun {
    /// A run importing `request`, its revision a resolved commit.
    fn new(request: DatasetImportRequest) -> Self {
        Self {
            request,
            progress: Mutex::new(ImportProgress {
                phase: DatasetImportPhase::ListingFiles,
                polled: Instant::now(),
            }),
        }
    }
    fn enter(&self, phase: DatasetImportPhase) {
        self.progress.recover_lock().phase = phase;
    }
    fn polled(&self) {
        self.progress.recover_lock().polled = Instant::now();
    }
    fn status(&self, id: &str) -> DatasetImportStatus {
        DatasetImportStatus::new(
            id.to_owned(),
            self.request.clone(),
            self.progress.recover_lock().phase.clone(),
        )
    }
}

/// An import runs to its end whoever polls it; once it has ended, it is
/// kept until nobody has read it for a while.
impl HeldWork for ImportRun {
    fn is_forgotten(&self) -> bool {
        let progress = self.progress.recover_lock();
        progress.phase.is_settled() && progress.polled.elapsed() > ABANDONED_AFTER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn an_import_is_kept_while_it_runs_and_until_its_end_has_gone_unread() {
        let run = ImportRun::new(DatasetImportRequest::new(
            DatasetFormat::LeRobot,
            HuggingFaceDatasetSource::new("pick", "lerobot/svla_so101_pickplace")
                .with_revision("f641879e22172be7e8161d5e6c1503c2d2feb657"),
        ));
        run.enter(DatasetImportPhase::ImportingEpisodes {
            written: 32,
            total: 50,
        });
        tokio::time::advance(ABANDONED_AFTER * 10).await;
        assert!(!run.is_forgotten(), "a running import outlives its page");
        run.enter(DatasetImportPhase::Done);
        run.polled();
        assert!(!run.is_forgotten());
        tokio::time::advance(ABANDONED_AFTER + std::time::Duration::from_secs(1)).await;
        assert!(run.is_forgotten());
        let status = run.status("dataset-import-1");
        assert_eq!(status.phase(), &DatasetImportPhase::Done);
        assert_eq!(status.request().format(), DatasetFormat::LeRobot);
        assert_eq!(
            status.request().source().revision(),
            "f641879e22172be7e8161d5e6c1503c2d2feb657"
        );
    }
}
