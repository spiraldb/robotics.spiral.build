//! Saved documents and independent native query executions. No selected workflow or editor draft.
use super::query::WorkflowQuery;
use crate::data::catalog_lease::CatalogLeaseReservation;
use crate::{OperationError, clock::SystemTimeMillis};
use serde::{Deserialize, Serialize};
use spiral_robotics_common::workflow::{
    WorkflowRunSnapshot, WorkflowSpec, WorkflowSummary,
    conversation::WorkflowConversation,
    document::{WorkflowDocument, WorkflowPlan, WorkflowRun, WorkflowSqlPlan},
};
use spiral_robotics_storage::{StorageManager, workflows::WorkflowJson};
use std::{sync::Arc, time::SystemTime};
const WORKFLOW_ENCODING_VERSION: u32 = 4;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedWorkflow {
    workflow: WorkflowSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    planned_sql: Option<WorkflowSqlPlan>,
}
impl SavedWorkflow {
    fn persisted(&self) -> Result<WorkflowJson, String> {
        serde_json::to_string(self)
            .map(|payload| WorkflowJson::new(WORKFLOW_ENCODING_VERSION, payload))
            .map_err(|error| format!("encoding the saved Workflow: {error}"))
    }

    fn for_id(id: &str, stored: &WorkflowJson) -> Result<Self, OperationError> {
        let saved = Self::restored(stored)?;
        if saved.workflow.id() != id {
            return Err(OperationError::IncompatibleWorkflow(
                "Saved SQL belongs to a different workflow.".to_owned(),
            ));
        }
        Ok(saved)
    }

    fn restored(stored: &WorkflowJson) -> Result<Self, OperationError> {
        if stored.version() != WORKFLOW_ENCODING_VERSION {
            return Err(OperationError::IncompatibleWorkflow(format!(
                "This workflow uses the retired graph format (version {}). Create a new SQL workflow; the original document has been preserved.",
                stored.version()
            )));
        }
        serde_json::from_str(stored.payload()).map_err(|error| {
            OperationError::IncompatibleWorkflow(format!(
                "The saved Workflow has an unsupported shape: {error}. The original has been preserved."
            ))
        })
    }
}

#[derive(Clone)]
pub(crate) struct WorkflowManager {
    inner: Arc<WorkflowRuntime>,
}
struct WorkflowRuntime {
    storage: Arc<StorageManager>,
    /// Queries in flight; recording and project changes wait for them.
    queries: tokio_util::task::TaskTracker,
    saves: tokio::sync::Mutex<()>,
}
impl WorkflowManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            inner: Arc::new(WorkflowRuntime {
                storage,
                queries: tokio_util::task::TaskTracker::new(),
                saves: tokio::sync::Mutex::new(()),
            }),
        }
    }
    pub(crate) async fn list(&self) -> Result<Vec<WorkflowSummary>, OperationError> {
        self.inner
            .storage
            .workflows()
            .await
            .map_err(OperationError::External)
    }
    pub(super) async fn load(&self, id: &str) -> Result<WorkflowDocument, OperationError> {
        let _save = self.inner.saves.lock().await;
        self.load_document(id).await
    }
    async fn load_document(&self, id: &str) -> Result<WorkflowDocument, OperationError> {
        let stored = self
            .inner
            .storage
            .load_workflow(id)
            .await
            .map_err(OperationError::External)?
            .ok_or_else(|| {
                OperationError::InvalidInput("That workflow no longer exists.".into())
            })?;
        let saved = SavedWorkflow::for_id(id, stored.graph())?;
        let conversation = match stored.conversation() {
            Some(stored) => WorkflowConversation::load(stored, id)
                .map_err(OperationError::IncompatibleWorkflow)?,
            None => WorkflowConversation::new(id),
        };
        // Plans are derived caches. Selection/output changes invalidate older
        // measurement-only plans without discarding the authored document.
        let planned_sql = saved
            .planned_sql
            .filter(|plan| plan.for_workflow(&saved.workflow).is_some());
        let document = WorkflowDocument::new(saved.workflow)
            .with_conversation(conversation)
            .with_plan(planned_sql);
        document
            .validate()
            .map_err(OperationError::IncompatibleWorkflow)?;
        Ok(document)
    }
    pub(super) async fn save(
        &self,
        mut document: WorkflowDocument,
    ) -> Result<WorkflowDocument, OperationError> {
        document.validate().map_err(OperationError::InvalidInput)?;
        let _save = self.inner.saves.lock().await;
        let id = document.workflow().id();
        let previous = self
            .inner
            .storage
            .load_workflow(id)
            .await
            .map_err(OperationError::External)?;
        let revision = previous
            .as_ref()
            .map(|stored| SavedWorkflow::for_id(id, stored.graph()))
            .transpose()?
            .map_or(0, |saved| saved.workflow.revision());
        if revision != document.workflow().revision() {
            return Err(OperationError::Conflict(
                "This workflow was saved elsewhere. Reopen it before saving.".into(),
            ));
        }
        document.workflow_mut().set_revision(
            revision
                .checked_add(1)
                .ok_or_else(|| OperationError::Internal("Workflow revision overflowed.".into()))?,
        );
        let graph = SavedWorkflow {
            workflow: document.workflow().clone(),
            planned_sql: document.planned_sql().cloned(),
        }
        .persisted()
        .map_err(OperationError::External)?;
        let conversation = document
            .conversation()
            .persisted()
            .map_err(OperationError::InvalidInput)?;
        if !self
            .inner
            .storage
            .save_workflow(document.workflow(), (&graph, &conversation))
            .await
            .map_err(OperationError::External)?
        {
            return Err(OperationError::Conflict(
                "This workflow was saved elsewhere. Reopen it before saving.".into(),
            ));
        }
        Ok(document)
    }
    pub(super) async fn plan(&self, workflow: &WorkflowSpec) -> WorkflowPlan {
        match WorkflowQuery::new(self.inner.storage.workflow_store())
            .plan(workflow)
            .await
        {
            Ok(plan) => plan,
            Err(error) => WorkflowPlan::failed(error),
        }
    }
    pub(crate) fn is_active(&self) -> bool {
        !self.inner.queries.is_empty()
    }
    /// Execute the submitted SQL to completion. A failed query is a failed
    /// run; only an invalid submission is an error. The run counts as active
    /// before the catalog reservation that admitted it is released, so a
    /// recording can never start in between.
    pub(super) async fn run(
        &self,
        workflow: WorkflowSpec,
        admission: CatalogLeaseReservation,
    ) -> Result<WorkflowRun, OperationError> {
        if !workflow.validate().valid() {
            return Err(OperationError::InvalidInput(
                "Fix SQL validation errors before running.".into(),
            ));
        }
        let _query = self.inner.queries.token();
        drop(admission);
        let mut snapshot = WorkflowRunSnapshot::started(format!("run-{}", uuid::Uuid::new_v4()))
            .with_started_ms(SystemTime::now().unix_millis());
        snapshot.finish(
            WorkflowQuery::new(self.inner.storage.workflow_store())
                .run(&workflow)
                .await,
        );
        Ok(WorkflowRun::new(workflow, snapshot))
    }
    pub(crate) async fn shutdown(&self) {
        self.inner.queries.close();
        self.inner.queries.wait().await;
    }
}

#[cfg(test)]
mod tests;
