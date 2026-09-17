//! Saved recipes and independent judged runs. No selected curation or editor draft.
use crate::run_registry::{RunAdmission, RunRegistry};
use crate::{OperationError, clock::SystemTimeMillis};
use spiral_robotics_common::curation::{
    CurationSpec, CurationSummary,
    run::{CurationOutcome, CurationRun, CurationRunSnapshot},
};
use spiral_robotics_common::runs::RunPhase;
use spiral_robotics_storage::{StorageManager, curations::StoredCurationDocument};
use std::{
    sync::Arc,
    time::{Instant, SystemTime},
};

/// The roster lists at most this many episodes; totals stay exact beyond it.
const ROSTER_ROWS: usize = 1_000;

#[derive(Clone)]
pub(crate) struct CurationManager {
    inner: Arc<CurationRuntime>,
}
struct CurationRuntime {
    storage: Arc<StorageManager>,
    runs: RunRegistry<CurationRun>,
    saves: tokio::sync::Mutex<()>,
}

impl CurationManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            inner: Arc::new(CurationRuntime {
                storage,
                runs: RunRegistry::new(),
                saves: tokio::sync::Mutex::new(()),
            }),
        }
    }
    pub(crate) async fn list(&self) -> Result<Vec<CurationSummary>, OperationError> {
        self.inner
            .storage
            .curations()
            .await
            .map_err(OperationError::External)
    }
    pub(crate) async fn load(&self, id: &str) -> Result<CurationSpec, OperationError> {
        let _save = self.inner.saves.lock().await;
        let stored = self
            .inner
            .storage
            .load_curation(id)
            .await
            .map_err(OperationError::External)?
            .ok_or_else(|| {
                OperationError::InvalidInput("That curation no longer exists.".into())
            })?;
        if stored.version() != StoredCurationDocument::CURRENT_VERSION {
            return Err(OperationError::IncompatibleWorkflow(format!(
                "This curation uses an unsupported document version ({}). The original has been preserved.",
                stored.version()
            )));
        }
        if stored.curation().id() != id {
            return Err(OperationError::IncompatibleWorkflow(
                "The saved recipe belongs to a different curation.".into(),
            ));
        }
        Ok(stored.into_curation())
    }
    /// Save one recipe revision. The submitted revision must equal the stored
    /// one; the saved copy carries the next.
    pub(crate) async fn save(
        &self,
        mut curation: CurationSpec,
    ) -> Result<CurationSpec, OperationError> {
        if curation.id().is_empty() || curation.id().len() > 128 {
            return Err(OperationError::InvalidInput(
                "Curation identity must contain 1–128 characters.".into(),
            ));
        }
        if curation.name().trim().is_empty() {
            return Err(OperationError::InvalidInput(
                "Name the curation before saving.".into(),
            ));
        }
        let _save = self.inner.saves.lock().await;
        let stored_revision = self
            .inner
            .storage
            .load_curation(curation.id())
            .await
            .map_err(OperationError::External)?
            .map_or(0, |stored| stored.curation().revision());
        if stored_revision != curation.revision() {
            return Err(OperationError::Conflict(
                "This curation was saved elsewhere. Reopen it before saving.".into(),
            ));
        }
        curation.set_revision(
            stored_revision
                .checked_add(1)
                .ok_or_else(|| OperationError::Internal("Curation revision overflowed.".into()))?,
        );
        let document = StoredCurationDocument::new(curation.clone());
        if !self
            .inner
            .storage
            .save_curation(&document)
            .await
            .map_err(OperationError::External)?
        {
            return Err(OperationError::Conflict(
                "This curation was saved elsewhere. Reopen it before saving.".into(),
            ));
        }
        Ok(curation)
    }
    pub(crate) fn runs(&self) -> Vec<CurationRun> {
        self.inner.runs.all()
    }
    pub(crate) fn run(&self, id: &str) -> Result<CurationRun, OperationError> {
        self.inner
            .runs
            .get(id)
            .ok_or_else(|| OperationError::InvalidInput("Unknown curation run.".into()))
    }
    /// Admit one run of the submitted recipe, unsaved edits included. The
    /// recipe is compiled up front so an invalid one never becomes a run.
    pub(crate) fn start(&self, curation: CurationSpec) -> Result<CurationRun, OperationError> {
        let compiled = curation.compile().map_err(OperationError::InvalidInput)?;
        let id = format!("curation-run-{}", uuid::Uuid::new_v4());
        let mut snapshot = CurationRunSnapshot::started(&curation, id.clone())
            .with_started_ms(SystemTime::now().unix_millis());
        snapshot.set_sql(Some(compiled.roster_sql().to_owned()));
        let record = CurationRun::new(curation.clone(), snapshot);
        let manager = self.clone();
        let task_id = id.clone();
        let judged = curation.clone();
        self.inner
            .runs
            .admit(
                RunAdmission::new(id, curation.id().to_owned(), record.clone()),
                move || tokio::spawn(async move { manager.execute(task_id, judged).await }),
            )
            .map_err(|existing| {
                OperationError::Conflict(format!(
                    "Curation '{}' already has an active run: {existing}.",
                    curation.name()
                ))
            })?;
        Ok(record)
    }
    pub(crate) async fn shutdown(&self) {
        self.inner.runs.shutdown().await;
    }
    async fn execute(&self, id: String, curation: CurationSpec) {
        self.inner.runs.update(&id, |record| {
            let snapshot = record.snapshot_mut();
            snapshot.set_phase(RunPhase::Running);
            snapshot.set_updated_ms(SystemTime::now().unix_millis());
        });
        let result = self.judge(&curation).await;
        self.inner.runs.finish(&id, |record| {
            let snapshot = record.snapshot_mut();
            match result {
                Ok(outcome) => {
                    snapshot.set_phase(RunPhase::Completed);
                    snapshot.set_outcome(Some(outcome));
                }
                Err(error) => {
                    snapshot.set_phase(RunPhase::Failed);
                    snapshot.set_error(Some(error));
                }
            }
            let now = SystemTime::now().unix_millis();
            snapshot.set_finished_ms(now);
            snapshot.set_updated_ms(now);
        });
    }
    /// Run the roster query; when Spiral bounded it, run the exact totals too.
    async fn judge(&self, curation: &CurationSpec) -> Result<CurationOutcome, String> {
        let compiled = curation.compile()?;
        let started = Instant::now();
        let store = self.inner.storage.workflow_store();
        let mut connection = store.connect().await?;
        let roster = connection
            .execute_query(compiled.roster_sql())
            .await?
            .into_preview_drained(ROSTER_ROWS)
            .await?;
        let totals = if roster.truncated() {
            Some(
                connection
                    .execute_query(compiled.totals_sql())
                    .await?
                    .into_preview_drained(1)
                    .await?,
            )
        } else {
            None
        };
        let outcome = compiled.outcome(&roster, totals.as_ref())?;
        Ok(outcome.with_elapsed_ms(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)))
    }
}

#[cfg(test)]
mod tests;
