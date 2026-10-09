//! LeRobot imports from Hugging Face, one held run each: Spiral lists the
//! repository through a Location and copies every episode into a Dataset in
//! one transaction, which takes as long as the dataset is big, while the
//! browser polls the run for its phase. The run is the backend's and
//! outlives the page; a restarted backend holds none, and its import
//! committed nothing.
use crate::OperationError;
use crate::held_runs::{ABANDONED_AFTER, HeldRuns, HeldWork};
use crate::sync::MutexRecovery;
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetInspection, HuggingFaceDatasetSource, LeRobotImportPhase, LeRobotImportStatus,
};
use spiral_robotics_storage::StorageManager;
use spiral_robotics_storage::ingestion::HuggingFaceDatasetDiscovery as _;
use std::sync::{Arc, Mutex};
use tokio::time::Instant;

/// The LeRobot imports one project has started.
pub(crate) struct LeRobotImports {
    storage: Arc<StorageManager>,
    runs: HeldRuns<ImportRun>,
}

impl LeRobotImports {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            storage,
            runs: HeldRuns::new("lerobot-import"),
        }
    }

    /// Start importing the repository `source` names, at the commit its
    /// revision resolves to now, and answer the run's status at once.
    pub(super) async fn start(
        &self,
        source: HuggingFaceDatasetSource,
    ) -> Result<LeRobotImportStatus, OperationError> {
        let inspection = HuggingFaceDatasetInspection::discover_lerobot(source)
            .await
            .map_err(OperationError::External)?;
        let run = Arc::new(ImportRun::new(
            HuggingFaceDatasetSource::new(inspection.dataset_id(), inspection.repository())
                .with_revision(inspection.resolved_revision()),
        ));
        let storage = self.storage.clone();
        let worked = run.clone();
        let id = self.runs.start(run.clone(), async move {
            let phase = |phase| worked.enter(phase);
            let imported = storage.import_lerobot_dataset(&inspection, &phase).await;
            worked.enter(match imported {
                Ok(()) => LeRobotImportPhase::Done,
                Err(error) => LeRobotImportPhase::Failed { error },
            });
        });
        Ok(run.status(&id))
    }

    /// Where run `id` has got.
    pub(super) fn status(&self, id: &str) -> Result<LeRobotImportStatus, OperationError> {
        let run = self.runs.get(id)?;
        run.polled();
        Ok(run.status(id))
    }

    /// Abandon every import: the project is closing, and Spiral rolls each
    /// one back.
    pub(crate) fn shutdown(&self) {
        self.runs.shutdown();
    }
}

/// One import: what it imports, and where it has got.
struct ImportRun {
    source: HuggingFaceDatasetSource,
    progress: Mutex<ImportProgress>,
}

struct ImportProgress {
    phase: LeRobotImportPhase,
    polled: Instant,
}

impl ImportRun {
    fn new(source: HuggingFaceDatasetSource) -> Self {
        Self {
            source,
            progress: Mutex::new(ImportProgress {
                phase: LeRobotImportPhase::ListingFiles,
                polled: Instant::now(),
            }),
        }
    }
    fn enter(&self, phase: LeRobotImportPhase) {
        self.progress.recover_lock().phase = phase;
    }
    fn polled(&self) {
        self.progress.recover_lock().polled = Instant::now();
    }
    fn status(&self, id: &str) -> LeRobotImportStatus {
        LeRobotImportStatus::new(
            id.to_owned(),
            self.source.clone(),
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
        let run = ImportRun::new(
            HuggingFaceDatasetSource::new("pick", "lerobot/svla_so101_pickplace")
                .with_revision("f641879e22172be7e8161d5e6c1503c2d2feb657"),
        );
        run.enter(LeRobotImportPhase::CopyingEpisodes);
        tokio::time::advance(ABANDONED_AFTER * 10).await;
        assert!(!run.is_forgotten(), "a running import outlives its page");
        run.enter(LeRobotImportPhase::Done);
        run.polled();
        assert!(!run.is_forgotten());
        tokio::time::advance(ABANDONED_AFTER + std::time::Duration::from_secs(1)).await;
        assert!(run.is_forgotten());
        let status = run.status("lerobot-import-1");
        assert_eq!(status.phase(), &LeRobotImportPhase::Done);
        assert_eq!(
            status.source().revision(),
            "f641879e22172be7e8161d5e6c1503c2d2feb657"
        );
    }
}
