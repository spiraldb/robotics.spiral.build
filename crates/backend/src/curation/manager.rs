//! Saved recipes and independent judged runs. No selected curation or editor draft.
use super::sql::CurationSql;
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
struct JudgedRun {
    outcome: CurationOutcome,
    derived_relations: Vec<String>,
}
struct CurationRuntime {
    storage: Arc<StorageManager>,
    runs: RunRegistry<CurationRun>,
    saves: tokio::sync::Mutex<()>,
    episode_progress: super::episode_progress::EpisodeProgress,
}

impl CurationManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            inner: Arc::new(CurationRuntime {
                episode_progress: super::episode_progress::EpisodeProgress::new(storage.clone()),
                storage,
                runs: RunRegistry::new(),
                saves: tokio::sync::Mutex::new(()),
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
        let totals = if roster.truncated() {
            Some(
                connection
                    .execute_query(compiled.totals_sql())
                    .await
                    .map_err(OperationError::External)?
                    .into_preview_drained(1)
                    .await
                    .map_err(OperationError::External)?,
            )
        } else {
            None
        };
        let outcome = compiled
            .outcome(&roster, totals.as_ref())
            .map_err(OperationError::External)?;
        if outcome.totals().truncated() {
            return Ok(outcome);
        }
        let mut entering = outcome.verdicts().iter().collect::<Vec<_>>();
        let mut impacts = Vec::new();
        for step in curation.steps() {
            let incoming = entering.len() as u64;
            match step {
                spiral_robotics_common::curation::CurationStep::Filter(rule) => {
                    entering
                        .retain(|verdict| !verdict.excluded_by().iter().any(|id| id == rule.id()));
                    impacts.push(
                        spiral_robotics_common::curation::run::CurationStageImpact::new(
                            rule.id().to_owned(),
                            incoming,
                            entering.len() as u64,
                        ),
                    );
                }
                spiral_robotics_common::curation::CurationStep::Transform(transform) => {
                    let mut impact =
                        spiral_robotics_common::curation::run::CurationStageImpact::new(
                            transform.id().to_owned(),
                            incoming,
                            incoming,
                        );
                    impact.set_affected_episodes(
                        entering
                            .iter()
                            .filter(|verdict| {
                                verdict
                                    .eligible_transforms()
                                    .iter()
                                    .any(|id| id == transform.id())
                            })
                            .count() as u64,
                    );
                    impacts.push(impact);
                }
            }
        }
        Ok(outcome.with_stage_impacts(impacts))
    }
    pub(super) async fn list(&self) -> Result<Vec<CurationSummary>, OperationError> {
        self.inner
            .storage
            .curations()
            .await
            .map_err(OperationError::External)
    }
    pub(super) async fn load(&self, id: &str) -> Result<CurationSpec, OperationError> {
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
    pub(super) fn runs(&self) -> Vec<CurationRun> {
        self.inner.runs.all()
    }
    pub(super) fn run(&self, id: &str) -> Result<CurationRun, OperationError> {
        self.inner
            .runs
            .get(id)
            .ok_or_else(|| OperationError::InvalidInput("Unknown curation run.".into()))
    }
    /// Admit one run of the submitted recipe, unsaved edits included. The
    /// recipe is compiled up front so an invalid one never becomes a run.
    pub(super) fn start(&self, mut curation: CurationSpec) -> Result<CurationRun, OperationError> {
        curation.set_source(curation.source().cloned());
        let compiled = CurationSql::new(&curation)
            .compile()
            .map_err(OperationError::InvalidInput)?;
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
        let output_relation = format!(
            "curation_{}",
            id.trim_start_matches("curation-run-").replace('-', "")
        );
        let result = self.judge(&curation, &output_relation).await;
        self.inner.runs.finish(&id, |record| {
            let snapshot = record.snapshot_mut();
            match result {
                Ok(result) => {
                    snapshot.set_phase(RunPhase::Completed);
                    snapshot.set_outcome(Some(result.outcome));
                    snapshot.set_derived_relations(result.derived_relations);
                    snapshot.set_output_relation(output_relation);
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
        let totals = if roster.truncated() {
            Some(
                connection
                    .execute_query(&compiled.output_totals_sql(output))
                    .await?
                    .into_preview_drained(1)
                    .await?,
            )
        } else {
            None
        };
        let stage_totals = connection
            .execute_query(&compiled.stage_totals_sql(output, curation))
            .await?
            .into_preview_drained(1)
            .await?;
        let mut impacts = compiled.stage_impacts(curation, &stage_totals)?;
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
        let outcome = compiled
            .outcome(&roster, totals.as_ref())?
            .with_stage_impacts(impacts);
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
