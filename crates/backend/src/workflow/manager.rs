//! Saved documents and independent graph executions. No selected workflow or editor draft.
use super::execution::{GraphExecution, NodeEvent};
use crate::{OperationError, clock::SystemTimeMillis, sync::MutexRecovery};
use serde::{Deserialize, Serialize};
use spiral_robotics_common::{
    relations::SpiralQuerySnapshot,
    workflow::{
        WorkflowNodePhase, WorkflowNodeSnapshot, WorkflowRunPhase, WorkflowRunSnapshot,
        WorkflowSpec, WorkflowSummary,
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
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
const WORKFLOW_ENCODING_VERSION: u32 = 3;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedWorkflow {
    workflow: WorkflowSpec,
    snapshot: WorkflowRunSnapshot,
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
                "Saved graph belongs to a different workflow.".to_owned(),
            ));
        }
        Ok(saved)
    }

    fn restored(stored: &WorkflowJson) -> Result<Self, OperationError> {
        // Version 2 uses the same envelope. Unchanged SQL/Table/Result graphs
        // still decode exactly; retired node shapes fail below, without migration.
        if !matches!(stored.version(), 2 | WORKFLOW_ENCODING_VERSION) {
            return Err(OperationError::IncompatibleWorkflow(format!(
                "The saved Workflow uses encoding version {}, which this Studio build cannot read. Update Studio to open this project.",
                stored.version()
            )));
        }
        let original: serde_json::Value =
            serde_json::from_str(stored.payload()).map_err(|error| {
                OperationError::IncompatibleWorkflow(format!(
                    "The saved Workflow is damaged: {error}. The original has been preserved."
                ))
            })?;
        let saved: Self = serde_json::from_value(original.clone())
            .map_err(|error| OperationError::IncompatibleWorkflow(format!("The saved Workflow has an unsupported shape: {error}. The original has been preserved.")))?;
        let canonical = serde_json::to_value(&saved)
            .map_err(|error| OperationError::Internal(error.to_string()))?;
        Self::validate_fields(&original, &canonical)?;
        saved.validate_snapshot()?;
        Ok(saved)
    }

    fn validate_fields(
        original: &serde_json::Value,
        canonical: &serde_json::Value,
    ) -> Result<(), OperationError> {
        use serde_json::Value;
        let preserved = match (original, canonical) {
            (Value::Object(original), Value::Object(canonical)) => {
                for (name, value) in original {
                    let Some(decoded) = canonical.get(name) else {
                        return Err(OperationError::IncompatibleWorkflow(format!(
                            "The saved Workflow contains an unsupported field {name:?}. The original has been preserved."
                        )));
                    };
                    Self::validate_fields(value, decoded)?;
                }
                true
            }
            (Value::Array(original), Value::Array(canonical))
                if original.len() == canonical.len() =>
            {
                for (value, decoded) in original.iter().zip(canonical) {
                    Self::validate_fields(value, decoded)?;
                }
                true
            }
            _ => original == canonical,
        };
        if preserved {
            Ok(())
        } else {
            Err(OperationError::IncompatibleWorkflow("The saved Workflow cannot be decoded without changing its contents. The original has been preserved.".to_owned()))
        }
    }

    fn validate_snapshot(&self) -> Result<(), OperationError> {
        if self.snapshot.workflow_id() != self.workflow.id()
            || self.snapshot.workflow_revision() != self.workflow.revision()
        {
            return Err(OperationError::IncompatibleWorkflow("The saved Workflow snapshot refers to a different graph revision. The original has been preserved.".to_owned()));
        }
        Ok(())
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
    cancel: CancellationToken,
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
            .load_workflow_graph(id)
            .await
            .map_err(OperationError::External)?
            .ok_or_else(|| {
                OperationError::InvalidInput("That workflow no longer exists.".into())
            })?;
        let saved = SavedWorkflow::for_id(id, &stored)?;
        let conversation = match self
            .inner
            .storage
            .load_workflow_conversation(id)
            .await
            .map_err(OperationError::External)?
        {
            Some(stored) => WorkflowConversation::load(&stored, id)
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
            .load_workflow_graph(id)
            .await
            .map_err(OperationError::External)?;
        let revision = previous
            .as_ref()
            .map(|stored| SavedWorkflow::for_id(id, stored))
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
            snapshot: WorkflowRunSnapshot::idle_for(document.workflow()),
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
        let validation = workflow.validate();
        let error = if validation.valid() {
            GraphExecution::validate_schema(self.inner.storage.workflow_store(), workflow)
                .await
                .err()
        } else {
            Some("Fix graph validation errors before planning.".into())
        };
        WorkflowPlan::new(validation, error)
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
                "Fix graph validation errors before running.".into(),
            ));
        }
        let mut runs = self.inner.runs.recover_lock();
        if runs.active.is_some() {
            return Err(OperationError::Conflict(
                "A workflow run is already active.".into(),
            ));
        }
        let id = format!("run-{}", uuid::Uuid::new_v4());
        let cancel = CancellationToken::new();
        let snapshot = WorkflowRunSnapshot::started(&workflow, id.clone())
            .with_nodes(
                workflow
                    .nodes()
                    .iter()
                    .map(|node| {
                        WorkflowNodeSnapshot::new(node.id(), WorkflowNodePhase::Pending, "Pending")
                    })
                    .collect(),
            )
            .with_started_ms(SystemTime::now().unix_millis());
        let record = WorkflowRun::new(workflow.clone(), snapshot);
        runs.snapshots.insert(id.clone(), record.clone());
        let manager = self.clone();
        let task_id = id.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            manager.execute(task_id, (workflow, task_cancel)).await;
        });
        runs.active = Some(ActiveRun {
            id,
            cancel,
            task: Some(task),
        });
        Ok(record)
    }
    pub(crate) fn cancel(&self, id: &str) -> Result<WorkflowRun, OperationError> {
        let mut runs = self.inner.runs.recover_lock();
        if let Some(active) = runs.active.as_ref().filter(|active| active.id == id) {
            active.cancel.cancel();
            if let Some(record) = runs.snapshots.get_mut(id) {
                let snapshot = record.snapshot_mut();
                snapshot.set_phase(WorkflowRunPhase::Cancelling);
                snapshot.set_updated_ms(SystemTime::now().unix_millis());
            }
        }
        runs.snapshots
            .get(id)
            .cloned()
            .ok_or_else(|| OperationError::InvalidInput("Unknown workflow run.".into()))
    }
    pub(crate) async fn shutdown(&self) {
        let task = {
            let mut runs = self.inner.runs.recover_lock();
            runs.active.as_mut().and_then(|active| {
                active.cancel.cancel();
                active.task.take()
            })
        };
        if let Some(task) = task {
            let _ = task.await;
        }
    }
    async fn execute(&self, id: String, execution: (WorkflowSpec, CancellationToken)) {
        let (workflow, cancel) = execution;
        let (events, mut receiver) = mpsc::unbounded_channel();
        let graph =
            GraphExecution::new(self.inner.storage.workflow_store(), cancel.clone(), events);
        self.update(&id, |snapshot| {
            if snapshot.phase() != WorkflowRunPhase::Cancelling {
                snapshot.set_phase(WorkflowRunPhase::Running);
            }
        });
        let run = async {
            let result = graph.run(&workflow).await;
            drop(graph);
            result
        };
        let progress = async {
            while let Some(event) = receiver.recv().await {
                self.node_event(&id, event);
            }
        };
        let (result, ()) = tokio::join!(run, progress);
        // Graph streams are dropped before releasing active ownership, including on cancellation.
        self.finish(&id, result);
    }
    fn update(&self, id: &str, change: impl FnOnce(&mut WorkflowRunSnapshot)) {
        if let Some(record) = self.inner.runs.recover_lock().snapshots.get_mut(id) {
            let snapshot = record.snapshot_mut();
            change(snapshot);
            snapshot.set_updated_ms(SystemTime::now().unix_millis());
        }
    }
    fn node_event(&self, run_id: &str, event: NodeEvent) {
        self.update(run_id, |snapshot| {
            if snapshot.phase() == WorkflowRunPhase::Cancelling {
                return;
            }
            let id = match &event {
                NodeEvent::Started(id) | NodeEvent::FusedComplete(id) => id,
                NodeEvent::Failed { id, .. } | NodeEvent::Complete { id, .. } => id,
            };
            let Some(node) = snapshot
                .nodes_mut()
                .iter_mut()
                .find(|node| node.node_id() == id)
            else {
                return;
            };
            match event {
                NodeEvent::Started(_) => {
                    node.set_phase(WorkflowNodePhase::Running);
                    node.set_detail("Executing Arrow operator");
                }
                NodeEvent::FusedComplete(_) => {
                    node.set_phase(WorkflowNodePhase::Complete);
                    node.set_detail("Executed with downstream SQL");
                }
                NodeEvent::Failed { error, .. } => {
                    node.set_phase(WorkflowNodePhase::Failed);
                    node.set_error(Some(error));
                }
                NodeEvent::Complete {
                    rows, elapsed_ms, ..
                } => {
                    node.set_phase(WorkflowNodePhase::Complete);
                    node.set_rows(Some(rows));
                    node.set_elapsed_ms(elapsed_ms);
                    node.set_detail(format!("Produced {rows} rows"));
                }
            }
        });
    }
    fn finish(&self, id: &str, result: Result<SpiralQuerySnapshot, String>) {
        let mut runs = self.inner.runs.recover_lock();
        let Some(active) = runs.active.as_ref().filter(|active| active.id == id) else {
            return;
        };
        let cancelled = active.cancel.is_cancelled();
        let snapshot = runs
            .snapshots
            .get_mut(id)
            .expect("active run has a snapshot")
            .snapshot_mut();
        if cancelled {
            snapshot.set_phase(WorkflowRunPhase::Cancelled);
        } else {
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
        }
        for node in snapshot.nodes_mut() {
            if matches!(
                node.phase(),
                WorkflowNodePhase::Pending | WorkflowNodePhase::Running
            ) {
                node.set_phase(WorkflowNodePhase::Cancelled);
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
