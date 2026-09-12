//! Saved documents and independent native query executions. No selected workflow or editor draft.
use super::query::WorkflowQuery;
use crate::{OperationError, clock::SystemTimeMillis, sync::MutexRecovery};
use serde::{Deserialize, Serialize};
use spiral_robotics_common::{
    relations::SpiralQuerySnapshot,
    workflow::{
        WorkflowRunPhase, WorkflowRunSnapshot, WorkflowSpec, WorkflowSummary,
        conversation::WorkflowConversation,
        document::{WorkflowDocument, WorkflowPlan, WorkflowRun},
    },
};
use spiral_robotics_storage::{StorageManager, workflows::WorkflowJson};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::SystemTime,
};
const WORKFLOW_ENCODING_VERSION: u32 = 4;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedWorkflow {
    workflow: WorkflowSpec,
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
    runs: Mutex<WorkflowRuns>,
    saves: tokio::sync::Mutex<()>,
}
#[derive(Default)]
struct WorkflowRuns {
    snapshots: BTreeMap<String, WorkflowRun>,
    active: Option<ActiveRun>,
}
struct ActiveRun {
    id: String,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl WorkflowManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            inner: Arc::new(WorkflowRuntime {
                storage,
                runs: Mutex::new(WorkflowRuns::default()),
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
    pub(crate) async fn load(&self, id: &str) -> Result<WorkflowDocument, OperationError> {
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
        Ok(WorkflowDocument::new(saved.workflow).with_conversation(conversation))
    }
    pub(crate) async fn save(
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
    pub(crate) async fn plan(&self, workflow: &WorkflowSpec) -> WorkflowPlan {
        match WorkflowQuery::new(self.inner.storage.workflow_store())
            .plan(workflow)
            .await
        {
            Ok(plan) => plan,
            Err(error) => WorkflowPlan::failed(error),
        }
    }
    pub(crate) fn is_active(&self) -> bool {
        self.inner.runs.recover_lock().active.is_some()
    }
    pub(crate) fn runs(&self) -> Vec<WorkflowRun> {
        self.inner
            .runs
            .recover_lock()
            .snapshots
            .values()
            .cloned()
            .collect()
    }
    pub(crate) fn run(&self, id: &str) -> Result<WorkflowRun, OperationError> {
        self.inner
            .runs
            .recover_lock()
            .snapshots
            .get(id)
            .cloned()
            .ok_or_else(|| OperationError::InvalidInput("Unknown workflow run.".into()))
    }
    pub(crate) fn start(&self, workflow: WorkflowSpec) -> Result<WorkflowRun, OperationError> {
        if !workflow.validate().valid() {
            return Err(OperationError::InvalidInput(
                "Fix SQL validation errors before running.".into(),
            ));
        }
        let mut runs = self.inner.runs.recover_lock();
        if runs.active.is_some() {
            return Err(OperationError::Conflict(
                "A workflow run is already active.".into(),
            ));
        }
        let id = format!("run-{}", uuid::Uuid::new_v4());
        let snapshot = WorkflowRunSnapshot::started(&workflow, id.clone())
            .with_started_ms(SystemTime::now().unix_millis());
        let record = WorkflowRun::new(workflow.clone(), snapshot);
        runs.snapshots.insert(id.clone(), record.clone());
        let manager = self.clone();
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            manager.execute(task_id, workflow).await;
        });
        runs.active = Some(ActiveRun {
            id,
            task: Some(task),
        });
        Ok(record)
    }
    pub(crate) async fn shutdown(&self) {
        let task = {
            let mut runs = self.inner.runs.recover_lock();
            runs.active.as_mut().and_then(|active| active.task.take())
        };
        if let Some(task) = task {
            let _ = task.await;
        }
    }
    async fn execute(&self, id: String, workflow: WorkflowSpec) {
        self.update(&id, |snapshot| {
            snapshot.set_phase(WorkflowRunPhase::Running)
        });
        let result = WorkflowQuery::new(self.inner.storage.workflow_store())
            .run(&workflow)
            .await;
        self.finish(&id, result);
    }
    fn update(&self, id: &str, change: impl FnOnce(&mut WorkflowRunSnapshot)) {
        if let Some(record) = self.inner.runs.recover_lock().snapshots.get_mut(id) {
            let snapshot = record.snapshot_mut();
            change(snapshot);
            snapshot.set_updated_ms(SystemTime::now().unix_millis());
        }
    }
    fn finish(&self, id: &str, result: Result<SpiralQuerySnapshot, String>) {
        let mut runs = self.inner.runs.recover_lock();
        let Some(_) = runs.active.as_ref().filter(|active| active.id == id) else {
            return;
        };
        let snapshot = runs
            .snapshots
            .get_mut(id)
            .expect("active run has a snapshot")
            .snapshot_mut();
        match result {
            Ok(result) => {
                snapshot.set_phase(WorkflowRunPhase::Completed);
                snapshot.set_result(Some(result));
            }
            Err(error) => {
                snapshot.set_phase(WorkflowRunPhase::Failed);
                snapshot.set_error(Some(error));
            }
        }
        let now = SystemTime::now().unix_millis();
        snapshot.set_finished_ms(now);
        snapshot.set_updated_ms(now);
        runs.active = None;
    }
}

#[cfg(test)]
mod tests;
