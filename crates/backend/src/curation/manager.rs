//! Saved recipes and independent judged runs. No selected curation or editor draft.
use super::sql::CurationSql;
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
struct JudgedRun {
    outcome: CurationOutcome,
    derived_relations: Vec<String>,
}
struct CurationRuntime {
    storage: Arc<StorageManager>,
    /// Judging runs in flight; shutdown waits for the relations they write.
    queries: tokio_util::task::TaskTracker,
    episode_progress: super::episode_progress::EpisodeProgress,
}

impl CurationManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            inner: Arc::new(CurationRuntime {
                episode_progress: super::episode_progress::EpisodeProgress::new(storage.clone()),
                storage,
                queries: tokio_util::task::TaskTracker::new(),
            }),
        }
    }
    pub(super) async fn score_episode(
        &self,
        request: spiral_robotics_common::curation::progress::ProgressScoreRequest,
    ) -> Result<spiral_robotics_common::curation::progress::ProgressScoreResult, OperationError>
    {
        self.inner
            .episode_progress
            .score(request)
            .await
            .map_err(OperationError::External)
    }
    pub(super) async fn preview(
        &self,
        curation: &CurationSpec,
    ) -> Result<CurationOutcome, OperationError> {
        let compiled = CurationSql::new(curation)
            .compile()
            .map_err(OperationError::InvalidInput)?;
        let mut connection = self
            .inner
            .storage
            .workflow_store()
            .connect()
            .await
            .map_err(OperationError::External)?;
        let roster = connection
            .execute_query(compiled.roster_sql())
            .await
            .map_err(OperationError::External)?
            .into_preview_drained(ROSTER_ROWS)
            .await
            .map_err(OperationError::External)?;
        let outcome = compiled
            .outcome(&roster)
            .map_err(OperationError::External)?;
        Ok(outcome.with_stage_impacts(compiled.stage_impacts(curation, &roster)))
    }
    pub(super) async fn list(&self) -> Result<Vec<CurationSummary>, OperationError> {
        self.inner
            .storage
            .curations()
            .list()
            .await
            .map_err(OperationError::External)
    }
    pub(super) async fn load(&self, id: &str) -> Result<CurationSpec, OperationError> {
        let stored = self
            .inner
            .storage
            .curations()
            .load(id)
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
        let mut curation = stored.into_curation();
        curation.set_source(curation.source().cloned());
        Ok(curation)
    }
    /// Save one recipe revision. The submitted revision must equal the stored
    /// one; the saved copy carries the next.
    pub(super) async fn save(
        &self,
        mut curation: CurationSpec,
    ) -> Result<CurationSpec, OperationError> {
        curation.set_source(curation.source().cloned());
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
        let stored_revision = self
            .inner
            .storage
            .curations()
            .load(curation.id())
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
            .curations()
            .save(&document)
            .await
            .map_err(OperationError::External)?
        {
            return Err(OperationError::Conflict(
                "This curation was saved elsewhere. Reopen it before saving.".into(),
            ));
        }
        Ok(curation)
    }
    /// Judge the submitted recipe, unsaved edits included, to completion. The
    /// recipe is compiled up front so an invalid one never becomes a run; a
    /// failed judgement is a failed run.
    pub(super) async fn run(
        &self,
        mut curation: CurationSpec,
    ) -> Result<CurationRun, OperationError> {
        curation.set_source(curation.source().cloned());
        let compiled = CurationSql::new(&curation)
            .compile()
            .map_err(OperationError::InvalidInput)?;
        let _query = self.inner.queries.token();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut snapshot = CurationRunSnapshot::started(format!("curation-run-{id}"))
            .with_started_ms(SystemTime::now().unix_millis());
        snapshot.set_sql(Some(compiled.roster_sql()));
        let output_relation = format!("curation_{id}");
        match self.judge(&curation, &output_relation).await {
            Ok(judged) => {
                snapshot.set_phase(RunPhase::Completed);
                snapshot.set_outcome(Some(judged.outcome));
                snapshot.set_derived_relations(judged.derived_relations);
                snapshot.set_output_relation(output_relation);
            }
            Err(error) => {
                snapshot.set_phase(RunPhase::Failed);
                snapshot.set_error(Some(error));
            }
        }
        Ok(CurationRun::new(curation, snapshot))
    }
    pub(crate) async fn shutdown(&self) {
        self.inner.queries.close();
        self.inner.queries.wait().await;
    }
    /// Materialize the judged population, then read its bounded roster, whose
    /// rows carry the exact totals and stage counts.
    async fn judge(&self, curation: &CurationSpec, output: &str) -> Result<JudgedRun, String> {
        let compiled = CurationSql::new(curation).compile()?;
        let started = Instant::now();
        let store = self.inner.storage.workflow_store();
        let mut connection = store.connect().await?;
        let progress = super::progress::ProgressOutput::for_spec(curation)?;
        let camera_outputs = CurationSql::new(curation).derived_outputs(output)?;
        let mut derived_relations = camera_outputs
            .iter()
            .map(|derived| derived.relation().to_owned())
            .collect::<Vec<_>>();
        connection.begin_transaction().await?;
        connection
            .execute_update_sql(&compiled.materialize_sql(output, curation)?)
            .await?;
        for selection in &progress {
            connection
                .execute_update_sql(&selection.materialize_sql(output, curation))
                .await?;
            derived_relations.push(selection.relation(output));
        }
        for derived in camera_outputs {
            connection.execute_update_sql(derived.statement()).await?;
        }
        let roster = connection
            .execute_query(&compiled.output_roster_sql(output))
            .await?
            .into_preview_drained(ROSTER_ROWS)
            .await?;
        let mut impacts = compiled.stage_impacts(curation, &roster);
        for selection in &progress {
            let effect = connection
                .execute_query(&selection.effect_sql(output))
                .await?
                .into_preview_drained(1)
                .await?;
            let coverage = connection
                .execute_query(&selection.coverage_sql(output))
                .await?
                .into_preview_drained(10_000)
                .await?;
            if coverage.truncated() {
                return Err(
                    "The scored episode coverage exceeded the validated source limit.".into(),
                );
            }
            if let (Some(row), Some(impact)) = (
                effect.rows().first(),
                impacts
                    .iter_mut()
                    .find(|impact| impact.id() == selection.transform_id()),
            ) {
                let count = |name| {
                    row.get(name)
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                };
                impact.set_time(count("retained_ns"), count("excluded_ns"));
                impact.set_scored_episode_ids(
                    coverage
                        .rows()
                        .iter()
                        .filter_map(|row| {
                            row.get("id")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect(),
                );
            }
        }
        let outcome = compiled.outcome(&roster)?.with_stage_impacts(impacts);
        connection.commit().await?;
        Ok(JudgedRun {
            outcome: outcome
                .with_elapsed_ms(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
            derived_relations,
        })
    }
}

#[cfg(test)]
mod tests;
