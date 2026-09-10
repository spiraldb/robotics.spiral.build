//! In-memory workflow editing and execution, with explicit graph/chat checkpoints.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio_util::sync::CancellationToken;

use crate::OperationError;
use crate::clock::SystemTimeMillis;
use crate::sync::MutexRecovery;

use serde::{Deserialize, Serialize};
use spiral_robotics_common::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_common::workflow::{
    WorkflowCommand, WorkflowNodePhase, WorkflowNodeSnapshot, WorkflowRunPhase,
    WorkflowRunSnapshot, WorkflowSnapshot, WorkflowSpec,
};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

use super::execution::{GraphExecution, NodeEvent};
use spiral_robotics_storage::StorageManager;
use spiral_robotics_storage::workflows::WorkflowJson;

/// Version of the persisted workflow encoding.
///
/// The graph is still evolving and is deliberately not decomposed into node
/// and edge relations yet, so its persisted form is one versioned payload.
/// Bump this constant when the payload's shape or persisted relation naming
/// changes. Unsupported versions are refused rather than rewritten.
const WORKFLOW_ENCODING_VERSION: u32 = 3;

/// Persistence is a backend implementation detail. The shared API exposes the
/// graph and observation, while the manager decides that they commit together.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedWorkflow {
    workflow: WorkflowSpec,
    snapshot: WorkflowRunSnapshot,
}

impl Default for SavedWorkflow {
    fn default() -> Self {
        let workflow = WorkflowSpec::default();
        let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        Self { workflow, snapshot }
    }
}

impl SavedWorkflow {
    fn fresh() -> Self {
        let workflow = WorkflowSpec::new(
            format!("workflow-{}", uuid::Uuid::new_v4()),
            "Untitled workflow",
        );
        let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        Self::new(workflow, snapshot)
    }

    fn require_idle(&self) -> Result<(), OperationError> {
        if self.snapshot().phase().active() {
            Err(OperationError::Conflict(
                "Stop the active Workflow run before changing this workflow.".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    fn require_revision(&self, expected: u64) -> Result<(), OperationError> {
        if expected != self.workflow().revision() {
            Err(OperationError::Conflict(
                "The workflow changed. Reload it before applying this edit.".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    fn new(workflow: WorkflowSpec, snapshot: WorkflowRunSnapshot) -> Self {
        Self { workflow, snapshot }
    }

    const fn workflow(&self) -> &WorkflowSpec {
        &self.workflow
    }

    #[cfg(test)]
    fn workflow_mut(&mut self) -> &mut WorkflowSpec {
        &mut self.workflow
    }

    const fn snapshot(&self) -> &WorkflowRunSnapshot {
        &self.snapshot
    }

    fn snapshot_mut(&mut self) -> &mut WorkflowRunSnapshot {
        &mut self.snapshot
    }

    fn persisted(&self) -> Result<WorkflowJson, String> {
        serde_json::to_string(self)
            .map(|payload| WorkflowJson::new(WORKFLOW_ENCODING_VERSION, payload))
            .map_err(|error| format!("encoding the saved Workflow: {error}"))
    }

    fn revised(mut self, workflow: WorkflowSpec, snapshot: WorkflowRunSnapshot) -> Self {
        self.workflow = workflow;
        self.snapshot = snapshot;
        self
    }

    fn for_id(id: &str, stored: &WorkflowJson) -> Result<Self, OperationError> {
        let saved = Self::restored(stored)?;
        if saved.workflow().id() != id {
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

pub(super) struct GraphEdit {
    scope_id: String,
    expected_revision: u64,
    workflow: WorkflowSpec,
}

impl GraphEdit {
    pub(super) fn new(scope_id: String, expected_revision: u64, workflow: WorkflowSpec) -> Self {
        Self {
            scope_id,
            expected_revision,
            workflow,
        }
    }
}

#[derive(Clone)]
pub(crate) struct WorkflowManager {
    inner: Arc<WorkflowManagerInner>,
}

struct WorkflowManagerInner {
    storage: Arc<StorageManager>,
    identity: WorkflowIdentity,
    saved: Mutex<SavedWorkflow>,
    workflows: Mutex<Vec<spiral_robotics_common::workflow::WorkflowSummary>>,
    active: Mutex<Option<ActiveRun>>,
    persistence: WorkflowPersistenceState,
}

struct WorkflowPersistenceState {
    loaded: tokio::sync::OnceCell<()>,
    mutation: AsyncMutex<()>,
    checkpoint: Mutex<Option<SavedWorkflow>>,
}

impl WorkflowPersistenceState {
    fn new() -> Self {
        Self {
            loaded: tokio::sync::OnceCell::new(),
            mutation: AsyncMutex::new(()),
            checkpoint: Mutex::new(None),
        }
    }
}

struct WorkflowIdentity {
    scope_id: Mutex<String>,
    run_sequence: AtomicU64,
    scope_sequence: AtomicU64,
}

impl WorkflowIdentity {
    fn new() -> Self {
        Self {
            scope_id: Mutex::new(format!(
                "workflow-scope-{}-0",
                SystemTime::now().unix_millis()
            )),
            run_sequence: AtomicU64::new(0),
            scope_sequence: AtomicU64::new(1),
        }
    }

    fn next_run_id(&self) -> String {
        format!(
            "workflow-{}-{}",
            SystemTime::now().unix_millis(),
            self.run_sequence.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn advance_scope(&self) {
        *self.scope_id.recover_lock() = format!(
            "workflow-scope-{}-{}",
            SystemTime::now().unix_millis(),
            self.scope_sequence.fetch_add(1, Ordering::Relaxed)
        );
    }
}

struct ActiveRun {
    run_id: String,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

struct WorkflowExecution {
    run_id: String,
    cancel: CancellationToken,
}

impl WorkflowExecution {
    fn new(run_id: String, cancel: CancellationToken) -> Self {
        Self { run_id, cancel }
    }
}

#[derive(Clone, Copy)]
struct WorkflowNodeTarget<'a> {
    run_id: &'a str,
    node_id: &'a str,
}

impl<'a> WorkflowNodeTarget<'a> {
    fn new(run_id: &'a str, node_id: &'a str) -> Self {
        Self { run_id, node_id }
    }
}

struct WorkflowNodePhaseChange<'a> {
    target: WorkflowNodeTarget<'a>,
    phase: WorkflowNodePhase,
    detail: String,
    error: Option<String>,
}

impl<'a> WorkflowNodePhaseChange<'a> {
    fn new(target: WorkflowNodeTarget<'a>, phase: WorkflowNodePhase, detail: String) -> Self {
        Self {
            target,
            phase,
            detail,
            error: None,
        }
    }
}

struct WorkflowNodeCompletion<'a> {
    target: WorkflowNodeTarget<'a>,
    started_ms: u64,
    detail: String,
    rows: Option<usize>,
}

impl<'a> WorkflowNodeCompletion<'a> {
    fn new(target: WorkflowNodeTarget<'a>, started_ms: u64, detail: String) -> Self {
        Self {
            target,
            started_ms,
            detail,
            rows: None,
        }
    }

    fn with_rows(mut self, rows: Option<usize>) -> Self {
        self.rows = rows;
        self
    }
}

struct WorkflowFailure<'a> {
    run_id: &'a str,
    error: String,
}

impl<'a> WorkflowFailure<'a> {
    fn new(run_id: &'a str, error: String) -> Self {
        Self { run_id, error }
    }
}

impl WorkflowManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            inner: Arc::new(WorkflowManagerInner {
                storage,
                identity: WorkflowIdentity::new(),
                saved: Mutex::new(SavedWorkflow::default()),
                workflows: Mutex::new(Vec::new()),
                active: Mutex::new(None),
                persistence: WorkflowPersistenceState::new(),
            }),
        }
    }

    pub(crate) async fn ensure_loaded(&self) -> Result<(), OperationError> {
        self.inner
            .persistence
            .loaded
            .get_or_try_init(|| async {
                self.prepare_from_storage().await?;
                Ok::<(), OperationError>(())
            })
            .await
            .map(|_| ())
    }

    pub(crate) async fn prepare_from_storage(&self) -> Result<(), OperationError> {
        let saved = self.load_latest().await?;
        self.install_saved(saved)
            .await
            .map_err(OperationError::External)
    }

    async fn load_latest(&self) -> Result<SavedWorkflow, OperationError> {
        let workflows = self
            .inner
            .storage
            .workflows()
            .await
            .map_err(OperationError::External)?;
        let saved = match workflows.first() {
            Some(workflow) => self.load_saved(workflow.id()).await?,
            None => SavedWorkflow::fresh(),
        };
        *self.inner.persistence.checkpoint.recover_lock() =
            (!workflows.is_empty()).then(|| saved.clone());
        *self.inner.workflows.recover_lock() = workflows;
        Ok(saved)
    }

    async fn load_saved(&self, id: &str) -> Result<SavedWorkflow, OperationError> {
        let stored = self
            .inner
            .storage
            .load_workflow_graph(id)
            .await
            .map_err(OperationError::External)?
            .ok_or_else(|| {
                OperationError::InvalidInput("That workflow no longer exists.".to_owned())
            })?;
        SavedWorkflow::for_id(id, &stored)
    }

    fn remember_workflow(&self, workflow: &WorkflowSpec) {
        let mut workflows = self.inner.workflows.recover_lock();
        workflows.retain(|summary| summary.id() != workflow.id());
        workflows.insert(
            0,
            spiral_robotics_common::workflow::WorkflowSummary::new(workflow.id(), workflow.name()),
        );
    }

    async fn select(
        &self,
        scope_id: &str,
        workflow_id: Option<&str>,
    ) -> Result<WorkflowSnapshot, OperationError> {
        let _mutation = self.inner.persistence.mutation.lock().await;
        self.require_scope(scope_id)?;
        if self.is_active() {
            return Err(OperationError::Conflict(
                "Stop the active Workflow run before switching workflows.".to_owned(),
            ));
        }
        let candidate = match workflow_id {
            Some(id) => self.load_saved(id).await?,
            None => SavedWorkflow::fresh(),
        };
        if self.inner.persistence.checkpoint.recover_lock().is_none() {
            let previous_id = self.inner.saved.recover_lock().workflow().id().to_owned();
            self.inner
                .workflows
                .recover_lock()
                .retain(|workflow| workflow.id() != previous_id);
        }
        *self.inner.persistence.checkpoint.recover_lock() = workflow_id.map(|_| candidate.clone());
        self.install_saved(candidate)
            .await
            .map_err(OperationError::External)?;
        Ok(self.state())
    }

    #[cfg(test)]
    async fn checkpoint(
        &self,
        conversation: &spiral_robotics_storage::workflow_assistant::WorkflowConversation,
    ) -> Result<WorkflowSnapshot, OperationError> {
        let _mutation = self.inner.persistence.mutation.lock().await;
        let current = self.inner.saved.recover_lock().clone();
        self.commit_checkpoint(current, conversation).await
    }

    async fn commit_checkpoint(
        &self,
        current: SavedWorkflow,
        conversation: &spiral_robotics_storage::workflow_assistant::WorkflowConversation,
    ) -> Result<WorkflowSnapshot, OperationError> {
        current.require_idle()?;
        if current.workflow().id() != conversation.workflow_id() {
            return Err(OperationError::Conflict(
                "The selected conversation changed.".into(),
            ));
        }
        self.inner
            .storage
            .save_workflow(
                current.workflow(),
                (
                    &current.persisted().map_err(OperationError::External)?,
                    &conversation.persisted().map_err(OperationError::External)?,
                ),
            )
            .await
            .map_err(OperationError::External)?;
        self.remember_workflow(current.workflow());
        *self.inner.saved.recover_lock() = current.clone();
        *self.inner.persistence.checkpoint.recover_lock() = Some(current);
        Ok(self.state())
    }

    async fn discard(&self, scope_id: &str) -> Result<WorkflowSnapshot, OperationError> {
        let _mutation = self.inner.persistence.mutation.lock().await;
        self.require_scope(scope_id)?;
        self.inner.saved.recover_lock().require_idle()?;
        let checkpoint = self.inner.persistence.checkpoint.recover_lock().clone();
        let candidate = checkpoint.unwrap_or_else(|| {
            let workflow = WorkflowSpec::new(
                self.inner.saved.recover_lock().workflow().id(),
                "Untitled workflow",
            );
            let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
            SavedWorkflow::new(workflow, snapshot)
        });
        self.install_saved(candidate)
            .await
            .map_err(OperationError::External)?;
        Ok(self.state())
    }

    async fn install_saved(&self, mut saved: SavedWorkflow) -> Result<(), String> {
        let interrupted = saved.snapshot().phase().active();
        if interrupted {
            let interruption =
                "Studio restarted before this Workflow run finished. Run it again.".to_owned();
            let finished_ms = SystemTime::now().unix_millis();
            let snapshot = saved.snapshot_mut();
            snapshot.set_phase(WorkflowRunPhase::Interrupted);
            snapshot.set_error(Some(interruption.clone()));
            snapshot.set_finished_ms(finished_ms);
            snapshot.set_updated_ms(finished_ms);
            for node in snapshot.nodes_mut() {
                if matches!(
                    node.phase(),
                    WorkflowNodePhase::Validating
                        | WorkflowNodePhase::Loading
                        | WorkflowNodePhase::Running
                        | WorkflowNodePhase::Cancelling
                ) {
                    node.set_phase(WorkflowNodePhase::Failed);
                    node.set_detail("Interrupted by Studio restart");
                    node.set_error(Some(interruption.clone()));
                }
            }
        }
        self.remember_workflow(saved.workflow());
        *self.inner.saved.recover_lock() = saved;
        self.inner.identity.advance_scope();

        Ok(())
    }

    pub(crate) fn state(&self) -> WorkflowSnapshot {
        let saved = self.inner.saved.recover_lock();
        self.state_response(&saved)
    }

    pub(crate) fn scope_id(&self) -> String {
        self.inner.identity.scope_id.recover_lock().clone()
    }

    pub(crate) async fn relations(&self) -> Result<RelationCatalog, String> {
        let scope_id = self.scope_id();
        let response = self.inner.storage.relation_catalog().await?;
        if self.scope_id() != scope_id {
            return Err(
                "The Spiral project changed while Workflow relations were loading. Reload them."
                    .to_owned(),
            );
        }
        Ok(response.with_scope(scope_id))
    }

    pub(crate) fn is_active(&self) -> bool {
        self.inner.saved.recover_lock().snapshot().phase().active()
    }

    /// Validate an authored graph against its server-owned identity and revision
    /// without mutating the saved workflow.
    pub(crate) fn prepare_candidate(
        &self,
        mut workflow: WorkflowSpec,
    ) -> Result<
        (
            WorkflowSpec,
            spiral_robotics_common::workflow::WorkflowValidationReport,
        ),
        OperationError,
    > {
        let current = self.inner.saved.recover_lock();
        if workflow.id() != current.workflow().id() {
            return Err(OperationError::InvalidInput(format!(
                "Workflow identity {:?} does not match the server-owned identity {:?}",
                workflow.id(),
                current.workflow().id()
            )));
        }
        workflow.set_revision(current.workflow().revision());
        let report = workflow.validate();
        Ok((workflow, report))
    }

    pub(crate) async fn command(
        &self,
        command: WorkflowCommand,
    ) -> Result<WorkflowSnapshot, OperationError> {
        match command {
            WorkflowCommand::Save { .. } => Err(OperationError::InvalidInput(
                "Save requires the graph and conversation together.".into(),
            )),
            WorkflowCommand::Discard { scope_id } => self.discard(&scope_id).await,
            WorkflowCommand::Create { scope_id } => self.select(&scope_id, None).await,
            WorkflowCommand::Select {
                scope_id,
                workflow_id,
            } => self.select(&scope_id, Some(&workflow_id)).await,
            WorkflowCommand::Apply {
                scope_id,
                expected_revision,
                workflow,
            } => {
                self.apply(GraphEdit {
                    scope_id,
                    expected_revision,
                    workflow,
                })
                .await
            }
            WorkflowCommand::Run { scope_id, revision } => {
                self.start(&scope_id, revision).await?;
                Ok(self.state())
            }
            WorkflowCommand::Cancel { scope_id } => {
                self.cancel(&scope_id).await?;
                Ok(self.state())
            }
        }
    }

    fn edit(&self, request: GraphEdit) -> Result<SavedWorkflow, OperationError> {
        let GraphEdit {
            scope_id,
            expected_revision,
            workflow: requested_workflow,
        } = request;
        self.require_scope(&scope_id)?;
        let current = self.inner.saved.recover_lock().clone();
        if requested_workflow.id() != current.workflow().id() {
            return Err(OperationError::InvalidInput(format!(
                "Workflow identity {:?} does not match the server-owned identity {:?}",
                requested_workflow.id(),
                current.workflow().id()
            )));
        }
        current.require_idle()?;
        current.require_revision(expected_revision)?;
        if !current.workflow().content_changed(&requested_workflow) {
            return Ok(current);
        }
        let next_revision = current
            .workflow()
            .revision()
            .checked_add(1)
            .ok_or_else(|| OperationError::Internal("Workflow revision overflowed.".to_owned()))?;
        let mut workflow = requested_workflow;
        workflow.set_revision(next_revision);
        let validation = workflow.validate();
        let empty_draft = workflow.nodes().is_empty() && workflow.edges().is_empty();
        if !workflow.save_allowed(validation.valid()) {
            let issue = validation
                .errors()
                .first()
                .map(|issue| format!("{} {}", issue.message(), issue.recovery()))
                .unwrap_or_else(|| "Workflow validation failed.".to_owned());
            return Err(OperationError::InvalidInput(issue));
        }

        let changed = current.workflow().content_changed(&workflow);
        let mut snapshot = current.snapshot().clone();
        snapshot.set_workflow_revision(workflow.revision());
        snapshot.set_updated_ms(SystemTime::now().unix_millis());
        if changed {
            let had_result = snapshot.result().is_some() && !empty_draft;
            snapshot.set_phase(WorkflowRunPhase::Idle);
            snapshot.set_result_stale(had_result);
            snapshot.set_error(None);
            *snapshot.nodes_mut() = workflow.saved_node_snapshots(current.snapshot(), had_result);
            if empty_draft {
                snapshot.set_result(None);
            }
        }
        let candidate = current.revised(workflow, snapshot);
        Ok(candidate)
    }

    async fn apply(&self, request: GraphEdit) -> Result<WorkflowSnapshot, OperationError> {
        let _mutation = self.inner.persistence.mutation.lock().await;
        let candidate = self.edit(request)?;
        self.remember_workflow(candidate.workflow());
        *self.inner.saved.recover_lock() = candidate;
        Ok(self.state())
    }

    pub(super) async fn save(
        &self,
        request: GraphEdit,
        conversation: &spiral_robotics_storage::workflow_assistant::WorkflowConversation,
    ) -> Result<WorkflowSnapshot, OperationError> {
        let _mutation = self.inner.persistence.mutation.lock().await;
        let candidate = self.edit(request)?;
        self.commit_checkpoint(candidate, conversation).await
    }

    async fn start(
        &self,
        scope_id: &str,
        revision: u64,
    ) -> Result<WorkflowRunSnapshot, OperationError> {
        let _mutation = self.inner.persistence.mutation.lock().await;
        self.require_scope(scope_id)?;
        // When both locks are required, active-run admission always precedes
        // the saved. Cancel and terminal publication use the same order.
        let active_exists = self.inner.active.recover_lock().is_some();
        let saved = self.inner.saved.recover_lock().clone();
        let validation = saved.workflow().validate();
        if active_exists {
            return Err(OperationError::Conflict(
                "A Workflow run is already active.".to_owned(),
            ));
        }
        saved.require_idle()?;
        saved.require_revision(revision)?;
        if !validation.valid() {
            return Err(OperationError::InvalidInput(
                "Apply a valid graph before running.".to_owned(),
            ));
        }

        let run_id = self.inner.identity.next_run_id();
        let cancel = CancellationToken::new();
        let snapshot = saved
            .workflow()
            .starting_snapshot(saved.snapshot(), run_id.clone());
        let candidate = saved
            .clone()
            .revised(saved.workflow().clone(), snapshot.clone());
        *self.inner.saved.recover_lock() = candidate;
        *self.inner.active.recover_lock() = Some(ActiveRun {
            run_id: run_id.clone(),
            cancel: cancel.clone(),
            task: None,
        });

        let manager = self.clone();
        let task_run_id = run_id.clone();
        let execution = WorkflowExecution::new(run_id, cancel);
        let task = tokio::spawn(async move {
            manager.execute(execution).await;
        });
        if let Some(active) = self
            .inner
            .active
            .recover_lock()
            .as_mut()
            .filter(|active| active.run_id == task_run_id)
        {
            active.task = Some(task);
        }
        Ok(snapshot)
    }

    pub(crate) async fn shutdown(&self) {
        let control = {
            let mut active = self.inner.active.recover_lock();
            active
                .as_mut()
                .map(|run| (run.cancel.clone(), run.task.take()))
        };
        if let Some((cancel, task)) = control {
            cancel.cancel();
            if let Some(task) = task {
                let _ = task.await;
            }
        }
    }

    async fn cancel(&self, scope_id: &str) -> Result<WorkflowRunSnapshot, OperationError> {
        let _mutation = self.inner.persistence.mutation.lock().await;
        self.require_scope(scope_id)?;
        // Keep admission and Cancelling publication in one active -> saved
        // critical section. Otherwise execution could publish Cancelled and
        // clear `active` between these steps, only for this request to regress
        // the persisted snapshot back to Cancelling.
        let (run_id, cancel) = {
            let active = self.inner.active.recover_lock();
            if active.is_none() {
                return Err(OperationError::Conflict(
                    "No Workflow run is active.".to_owned(),
                ));
            }
            let control = active.as_ref().expect("admission requires an active run");
            (control.run_id.clone(), control.cancel.clone())
        };
        cancel.cancel();
        let saved = self.inner.saved.recover_lock().clone();
        if saved.snapshot().run_id() != Some(run_id.as_str()) {
            return Err(OperationError::Conflict(
                "The active Workflow run changed before cancellation.".to_owned(),
            ));
        }
        let mut candidate = saved.clone();
        candidate
            .snapshot_mut()
            .set_phase(WorkflowRunPhase::Cancelling);
        candidate
            .snapshot_mut()
            .set_updated_ms(SystemTime::now().unix_millis());
        for node in candidate.snapshot_mut().nodes_mut() {
            if matches!(
                node.phase(),
                WorkflowNodePhase::Validating
                    | WorkflowNodePhase::Loading
                    | WorkflowNodePhase::Running
            ) {
                node.set_phase(WorkflowNodePhase::Cancelling);
                node.set_detail("Cancelling after the current Spiral operation");
            }
        }
        let snapshot = candidate.snapshot().clone();
        *self.inner.saved.recover_lock() = candidate;
        Ok(snapshot)
    }

    async fn execute(&self, execution: WorkflowExecution) {
        let WorkflowExecution { run_id, cancel, .. } = execution;
        let workflow = self.inner.saved.recover_lock().workflow().clone();
        let (events, mut receiver) = mpsc::unbounded_channel();
        let graph =
            GraphExecution::new(self.inner.storage.workflow_store(), cancel.clone(), events);
        self.set_run_phase(&run_id, WorkflowRunPhase::Running).await;
        let run = async {
            let result = graph.run(&workflow).await;
            drop(graph);
            result
        };
        let progress = async {
            while let Some(event) = receiver.recv().await {
                match event {
                    NodeEvent::Started(id) => {
                        self.set_node_phase(WorkflowNodePhaseChange::new(
                            WorkflowNodeTarget::new(&run_id, &id),
                            WorkflowNodePhase::Running,
                            "Executing Arrow operator".into(),
                        ))
                        .await
                    }
                    NodeEvent::FusedComplete(id) => {
                        self.complete_node(WorkflowNodeCompletion::new(
                            WorkflowNodeTarget::new(&run_id, &id),
                            SystemTime::now().unix_millis(),
                            "Executed with downstream SQL".into(),
                        ))
                        .await
                    }
                    NodeEvent::Failed { id, error } => {
                        let mut change = WorkflowNodePhaseChange::new(
                            WorkflowNodeTarget::new(&run_id, &id),
                            WorkflowNodePhase::Failed,
                            "Failed".into(),
                        );
                        change.error = Some(error);
                        self.set_node_phase(change).await;
                    }
                    NodeEvent::Complete {
                        id,
                        rows,
                        elapsed_ms,
                    } => {
                        self.complete_node(
                            WorkflowNodeCompletion::new(
                                WorkflowNodeTarget::new(&run_id, &id),
                                SystemTime::now().unix_millis().saturating_sub(elapsed_ms),
                                format!("Produced {rows} rows"),
                            )
                            .with_rows(Some(rows)),
                        )
                        .await
                    }
                }
            }
        };
        let (result, ()) = tokio::join!(run, progress);
        if self.cancelled(&run_id, &cancel) {
            self.finish_cancelled(&run_id).await;
        } else {
            match result {
                Ok(result) => self.finish_completed(&run_id, Some(result)).await,
                Err(error) => {
                    self.finish_failed(WorkflowFailure::new(&run_id, error))
                        .await
                }
            }
        }
    }

    async fn set_run_phase(&self, run_id: &str, phase: WorkflowRunPhase) {
        let _mutation = self.inner.persistence.mutation.lock().await;
        let mut saved = self.inner.saved.recover_lock().clone();
        if saved.snapshot().run_id() != Some(run_id)
            || matches!(
                saved.snapshot().phase(),
                WorkflowRunPhase::Cancelling | WorkflowRunPhase::Cancelled
            )
        {
            return;
        }
        saved.snapshot_mut().set_phase(phase);
        saved
            .snapshot_mut()
            .set_updated_ms(SystemTime::now().unix_millis());
        *self.inner.saved.recover_lock() = saved;
    }

    async fn set_node_phase(&self, change: WorkflowNodePhaseChange<'_>) {
        let WorkflowNodePhaseChange {
            target: WorkflowNodeTarget { run_id, node_id },
            phase,
            detail,
            error,
        } = change;
        let _mutation = self.inner.persistence.mutation.lock().await;
        let mut saved = self.inner.saved.recover_lock().clone();
        if saved.snapshot().run_id() != Some(run_id)
            || matches!(
                saved.snapshot().phase(),
                WorkflowRunPhase::Cancelling | WorkflowRunPhase::Cancelled
            )
        {
            return;
        }
        if let Some(node) = saved
            .snapshot_mut()
            .nodes_mut()
            .iter_mut()
            .find(|node| node.node_id() == node_id)
        {
            node.set_phase(phase);
            node.set_detail(detail);
            node.set_error(error);
        }
        saved
            .snapshot_mut()
            .set_updated_ms(SystemTime::now().unix_millis());
        *self.inner.saved.recover_lock() = saved;
    }

    async fn complete_node(&self, completion: WorkflowNodeCompletion<'_>) {
        let WorkflowNodeCompletion {
            target: WorkflowNodeTarget { run_id, node_id },
            started_ms,
            detail,
            rows,
        } = completion;
        let _mutation = self.inner.persistence.mutation.lock().await;
        let mut saved = self.inner.saved.recover_lock().clone();
        if saved.snapshot().run_id() != Some(run_id) {
            return;
        }
        if let Some(node) = saved
            .snapshot_mut()
            .nodes_mut()
            .iter_mut()
            .find(|node| node.node_id() == node_id)
        {
            node.set_phase(WorkflowNodePhase::Complete);
            node.set_detail(detail);
            node.set_elapsed_ms(SystemTime::now().unix_millis().saturating_sub(started_ms));
            node.set_rows(rows);
            node.set_error(None);
        }
        saved
            .snapshot_mut()
            .set_updated_ms(SystemTime::now().unix_millis());
        *self.inner.saved.recover_lock() = saved;
    }

    async fn finish_completed(&self, run_id: &str, result: Option<SpiralQuerySnapshot>) {
        // Linearize terminal publication against cancellation. Whichever
        // claims the active-run mutex first owns the final outcome; a cancel
        // that arrives first prevents the just-finished database payload from
        // replacing the previous stale result.
        let mutation = self.inner.persistence.mutation.lock().await;
        let Some(cancelled) = self
            .inner
            .active
            .recover_lock()
            .as_ref()
            .filter(|active| active.run_id == run_id)
            .map(|control| control.cancel.is_cancelled())
        else {
            return;
        };
        if cancelled {
            drop(mutation);
            self.finish_cancelled(run_id).await;
            return;
        }
        let saved = self.inner.saved.recover_lock().clone();
        if saved.snapshot().run_id() != Some(run_id) {
            return;
        }
        let mut candidate = saved.clone();
        let finished = SystemTime::now().unix_millis();
        candidate
            .snapshot_mut()
            .set_phase(WorkflowRunPhase::Completed);
        candidate.snapshot_mut().set_finished_ms(finished);
        candidate.snapshot_mut().set_updated_ms(finished);
        candidate.snapshot_mut().set_error(None);
        candidate.snapshot_mut().set_result(result);
        candidate.snapshot_mut().set_result_stale(false);
        *self.inner.saved.recover_lock() = candidate;
        *self.inner.active.recover_lock() = None;
    }

    async fn finish_failed(&self, failure: WorkflowFailure<'_>) {
        let WorkflowFailure { run_id, error } = failure;
        let mutation = self.inner.persistence.mutation.lock().await;
        let Some(cancelled) = self
            .inner
            .active
            .recover_lock()
            .as_ref()
            .filter(|active| active.run_id == run_id)
            .map(|control| control.cancel.is_cancelled())
        else {
            return;
        };
        if cancelled {
            drop(mutation);
            self.finish_cancelled(run_id).await;
            return;
        }
        let saved = self.inner.saved.recover_lock().clone();
        if saved.snapshot().run_id() != Some(run_id) {
            return;
        }
        let mut candidate = saved.clone();
        candidate
            .snapshot_mut()
            .nodes_mut()
            .mark_inflight_cancelled();
        let finished = SystemTime::now().unix_millis();
        candidate.snapshot_mut().set_phase(WorkflowRunPhase::Failed);
        candidate.snapshot_mut().set_finished_ms(finished);
        candidate.snapshot_mut().set_updated_ms(finished);
        candidate.snapshot_mut().set_error(Some(error));
        *self.inner.saved.recover_lock() = candidate;
        *self.inner.active.recover_lock() = None;
    }

    async fn finish_cancelled(&self, run_id: &str) {
        let _mutation = self.inner.persistence.mutation.lock().await;
        if self
            .inner
            .active
            .recover_lock()
            .as_ref()
            .is_none_or(|active| active.run_id != run_id)
        {
            return;
        }
        let saved = self.inner.saved.recover_lock().clone();
        if saved.snapshot().run_id() != Some(run_id) {
            return;
        }
        let mut candidate = saved.clone();
        candidate
            .snapshot_mut()
            .nodes_mut()
            .mark_inflight_cancelled();
        let finished = SystemTime::now().unix_millis();
        candidate
            .snapshot_mut()
            .set_phase(WorkflowRunPhase::Cancelled);
        candidate.snapshot_mut().set_finished_ms(finished);
        candidate.snapshot_mut().set_updated_ms(finished);
        candidate.snapshot_mut().set_error(None);
        *self.inner.saved.recover_lock() = candidate;
        *self.inner.active.recover_lock() = None;
    }

    fn cancelled(&self, run_id: &str, cancel: &CancellationToken) -> bool {
        cancel.is_cancelled()
            || self
                .inner
                .active
                .recover_lock()
                .as_ref()
                .is_none_or(|active| active.run_id != run_id)
    }

    fn require_scope(&self, requested_scope_id: &str) -> Result<(), OperationError> {
        if requested_scope_id != self.scope_id() {
            return Err(OperationError::Conflict(
                "The selected workflow or project changed. Reload Workflow and try again."
                    .to_owned(),
            ));
        }
        Ok(())
    }

    fn state_response(&self, saved: &SavedWorkflow) -> WorkflowSnapshot {
        let mut response = saved
            .state_response()
            .with_workflows(self.inner.workflows.recover_lock().clone());
        response.set_scope_id(self.scope_id());
        response.with_unsaved_changes(
            match self.inner.persistence.checkpoint.recover_lock().as_ref() {
                Some(checkpoint) => checkpoint != saved,
                None => {
                    saved.workflow().revision() != 0
                        || !saved.workflow().nodes().is_empty()
                        || saved.snapshot().phase() != WorkflowRunPhase::Idle
                }
            },
        )
    }
}

trait WorkflowNodeSnapshots {
    fn mark_inflight_cancelled(&mut self);
}

impl WorkflowNodeSnapshots for [WorkflowNodeSnapshot] {
    fn mark_inflight_cancelled(&mut self) {
        for node in self {
            if matches!(
                node.phase(),
                WorkflowNodePhase::Pending
                    | WorkflowNodePhase::Validating
                    | WorkflowNodePhase::Loading
                    | WorkflowNodePhase::Running
                    | WorkflowNodePhase::Cancelling
            ) {
                node.set_phase(WorkflowNodePhase::Cancelled);
                node.set_detail("Cancelled");
            }
        }
    }
}

impl SavedWorkflow {
    fn state_response(&self) -> WorkflowSnapshot {
        let mut response = WorkflowSnapshot::from_workflow(String::new(), self.workflow().clone());
        response.set_snapshot(self.snapshot().clone());
        response
    }
}

trait WorkflowOutputs {
    fn content_changed(&self, next: &Self) -> bool;
    fn saved_node_snapshots(
        &self,
        _previous: &WorkflowRunSnapshot,
        had_result: bool,
    ) -> Vec<WorkflowNodeSnapshot>;
    fn starting_snapshot(
        &self,
        previous: &WorkflowRunSnapshot,
        run_id: String,
    ) -> WorkflowRunSnapshot;
    fn save_allowed(&self, validation_valid: bool) -> bool;
}

impl WorkflowOutputs for WorkflowSpec {
    fn content_changed(&self, next: &Self) -> bool {
        let mut normalized = next.clone();
        normalized.set_revision(self.revision());
        self != &normalized
    }

    fn saved_node_snapshots(
        &self,
        _previous: &WorkflowRunSnapshot,
        had_result: bool,
    ) -> Vec<WorkflowNodeSnapshot> {
        self.nodes()
            .iter()
            .map(|node| {
                WorkflowNodeSnapshot::new(
                    node.id(),
                    if had_result {
                        WorkflowNodePhase::Stale
                    } else {
                        WorkflowNodePhase::Pending
                    },
                    if had_result {
                        "Saved graph changed; run again".to_owned()
                    } else {
                        "Not run".to_owned()
                    },
                )
            })
            .collect()
    }

    fn starting_snapshot(
        &self,
        previous: &WorkflowRunSnapshot,
        run_id: String,
    ) -> WorkflowRunSnapshot {
        let started_ms = SystemTime::now().unix_millis();
        let nodes = self
            .nodes()
            .iter()
            .map(|node| WorkflowNodeSnapshot::new(node.id(), WorkflowNodePhase::Pending, "Pending"))
            .collect();
        let mut snapshot = WorkflowRunSnapshot::started(self, run_id)
            .with_nodes(nodes)
            .with_started_ms(started_ms);
        snapshot.set_result(previous.result().cloned());
        snapshot.set_result_stale(previous.result().is_some());
        snapshot
    }

    fn save_allowed(&self, validation_valid: bool) -> bool {
        validation_valid
            || (self.nodes().is_empty()
                && self.edges().is_empty()
                && !self.name().trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use arrow_array::{ArrayRef, Int64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use spiral_robotics_common::relations::SpiralQuerySnapshot;
    use spiral_robotics_common::workflow::{
        WorkflowNodeConfig, WorkflowNodeSpec, WorkflowRunPhase,
    };

    use spiral_robotics_storage::ProjectStore;
    use spiral_robotics_storage::{BackendStateStore, ServiceEndpoint, SpiralConnectionSettings};

    /// The stored saved declares its own encoding. A newer version is
    /// refused so an operator's saved graph is never replaced by an empty one.
    #[test]
    fn stored_workflow_versions_are_explicit_and_unsupported_documents_are_preserved() {
        let saved = SavedWorkflow::default();
        let stored = saved.persisted().unwrap();
        assert_eq!(stored.version(), 3);
        assert_eq!(
            SavedWorkflow::restored(&WorkflowJson::new(2, stored.payload().to_owned())).unwrap(),
            saved
        );
        assert_eq!(SavedWorkflow::restored(&stored).unwrap(), saved);
        for version in [1, 4] {
            let unsupported = WorkflowJson::new(version, stored.payload().to_owned());
            let original = unsupported.payload().to_owned();
            let error = SavedWorkflow::restored(&unsupported).unwrap_err();
            assert_eq!(
                error.code(),
                spiral_robotics_common::command_failure::CommandFailureCode::IncompatibleWorkflow
            );
            assert_eq!(unsupported.payload(), original);
        }
        let damaged = WorkflowJson::new(2, r#"{"concepts":["cube"]}"#.to_owned());
        assert!(matches!(
            SavedWorkflow::restored(&damaged),
            Err(OperationError::IncompatibleWorkflow(_))
        ));
    }

    #[test]
    fn saved_workflow_rejects_graph_history_and_mismatched_snapshots() {
        let saved = SavedWorkflow::fresh();
        let original = serde_json::to_value(&saved).unwrap();
        let mut history = original.clone();
        history["history"] = serde_json::json!([]);
        let mut mismatched = original.clone();
        mismatched["snapshot"]["workflow_revision"] = serde_json::json!(99);
        let mut unknown = original;
        unknown["workflow"]["future_field"] = serde_json::json!(true);
        for payload in [history, mismatched, unknown] {
            assert!(
                SavedWorkflow::restored(&WorkflowJson::new(
                    WORKFLOW_ENCODING_VERSION,
                    payload.to_string()
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn revision_increment_alone_does_not_stale_a_result() {
        let current = WorkflowSpec::default();
        let mut next = current.clone();
        next.set_revision(next.revision() + 1);
        assert!(!current.content_changed(&next));
        next.set_name("Changed");
        assert!(current.content_changed(&next));
    }

    #[test]
    fn an_intentionally_empty_draft_can_save_but_remains_non_runnable() {
        let mut workflow = WorkflowSpec::default();
        workflow.nodes_mut().clear();
        workflow.edges_mut().clear();
        let validation = workflow.validate();

        assert!(!validation.valid());
        assert!(workflow.save_allowed(validation.valid()));

        workflow.nodes_mut().push(WorkflowNodeSpec::new(
            "orphan",
            "Orphan table",
            WorkflowNodeConfig::Table {
                relation: "robotics.frames".to_owned(),
            },
        ));
        let invalid = workflow.validate();
        assert!(!workflow.save_allowed(invalid.valid()));
    }

    #[test]
    fn rerun_keeps_previous_rows_visibly_stale_until_result_replacement() {
        let workflow = WorkflowSpec::default();
        let mut previous = WorkflowRunSnapshot::idle_for(&workflow);
        previous.set_result(Some(
            SpiralQuerySnapshot::new(Vec::new(), vec![serde_json::Map::new()], false)
                .with_elapsed_ms(4),
        ));
        let next = workflow.starting_snapshot(&previous, "run".to_owned());
        assert_eq!(next.result(), previous.result());
        assert!(next.result_stale());
        assert_eq!(next.phase(), WorkflowRunPhase::Validating);
    }

    #[tokio::test]
    async fn draft_execution_needs_no_saved_row_and_failed_save_keeps_the_draft() {
        let store = Arc::new(ProjectStore::workflow_test());
        let manager = WorkflowManager::new(Arc::new(StorageManager::new(store)));
        let fresh = manager
            .command(WorkflowCommand::create(manager.scope_id()))
            .await
            .unwrap();
        let mut graph = fresh.workflow().clone();
        graph
            .nodes_mut()
            .push(spiral_robotics_common::workflow::WorkflowNodeSpec::new(
                "source",
                "Source",
                WorkflowNodeConfig::Query {
                    sql: "SELECT 42 AS answer".into(),
                    max_rows: 10,
                },
            ));
        graph
            .nodes_mut()
            .push(spiral_robotics_common::workflow::WorkflowNodeSpec::new(
                "result",
                "Result",
                WorkflowNodeConfig::Result,
            ));
        graph
            .edges_mut()
            .push(spiral_robotics_common::workflow::WorkflowEdgeSpec::new(
                "source", "result",
            ));
        let draft = manager
            .command(WorkflowCommand::apply(manager.scope_id(), 0, graph))
            .await
            .unwrap();
        assert!(draft.unsaved_changes());
        manager
            .start(&manager.scope_id(), draft.workflow().revision())
            .await
            .unwrap();
        let result = wait_for_terminal(&manager).await;
        assert_eq!(result.phase(), WorkflowRunPhase::Completed);
        assert_eq!(result.result().unwrap().rows()[0]["answer"], 42);
        let before = manager.state();
        let mut renamed = before.workflow().clone();
        renamed.set_name("Keep this edit after save fails");
        let chat =
            spiral_robotics_storage::workflow_assistant::WorkflowConversation::new(renamed.id());
        assert!(
            manager
                .save(
                    GraphEdit::new(manager.scope_id(), renamed.revision(), renamed),
                    &chat
                )
                .await
                .is_err(),
            "this fixture has no durable workflows table"
        );
        assert_eq!(
            manager.state(),
            before,
            "failed Save must not advance the server revision or discard the draft"
        );
        manager
            .command(WorkflowCommand::Discard {
                scope_id: manager.scope_id(),
            })
            .await
            .unwrap();
        assert!(manager.state().workflow().nodes().is_empty());
        assert!(!manager.state().unsaved_changes());
    }

    #[tokio::test]
    async fn progress_and_cancel_do_not_wait_for_live_arrow_read_leases() {
        let store = Arc::new(ProjectStore::workflow_test());
        let manager = WorkflowManager::new(Arc::new(StorageManager::new(store.clone())));
        let workflow = WorkflowSpec::default();
        let snapshot = WorkflowRunSnapshot::started(&workflow, "lease-test");
        *manager.inner.saved.recover_lock() = SavedWorkflow::new(workflow, snapshot);
        *manager.inner.active.recover_lock() = Some(ActiveRun {
            run_id: "lease-test".into(),
            cancel: CancellationToken::new(),
            task: None,
        });
        let _live_input = store.connect().await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(2),
            manager.set_node_phase(WorkflowNodePhaseChange::new(
                WorkflowNodeTarget::new("lease-test", "query"),
                WorkflowNodePhase::Running,
                "Running".into(),
            )),
        )
        .await
        .expect("progress must not request a catalog write lease");
        let result =
            tokio::time::timeout(Duration::from_secs(2), manager.cancel(&manager.scope_id()))
                .await
                .expect("Cancel must return before input streams release their leases")
                .unwrap();
        assert_eq!(result.phase(), WorkflowRunPhase::Cancelling);
        assert!(
            manager
                .inner
                .active
                .recover_lock()
                .as_ref()
                .unwrap()
                .cancel
                .is_cancelled()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authoritative_project_reload_gets_a_new_scope_for_a_lower_revision() {
        let project_store = Arc::new(
            {
                let settings = SpiralConnectionSettings::new(
                    "https://api.spiraldb.dev".to_owned(),
                    "https://spfs.spiraldb.dev".to_owned(),
                )
                .expect("the unconfigured test connection is valid");
                let database =
                    Arc::new(spiral_robotics_storage::AdbcDatabase::new(settings.clone()));
                ProjectStore::new(settings, "workflow_scope_reload".to_owned(), database)
            }
            .expect("the unconfigured test project_store is valid"),
        );
        let manager = WorkflowManager::new(Arc::new(StorageManager::new(project_store)));
        let mut project_a = SavedWorkflow::default();
        project_a.workflow_mut().set_revision(9);
        project_a.snapshot_mut().set_workflow_revision(9);
        manager
            .install_saved(project_a)
            .await
            .expect("the first project installs");
        let scope_a = manager.state().scope_id().to_owned();

        let mut project_b = SavedWorkflow::default();
        project_b.workflow_mut().set_revision(1);
        project_b.snapshot_mut().set_workflow_revision(1);
        manager
            .install_saved(project_b)
            .await
            .expect("the replacement project installs");
        let state_b = manager.state();

        assert_ne!(state_b.scope_id(), scope_a);
        assert_eq!(state_b.workflow().revision(), 1);
        assert!(matches!(
            manager
                .command(WorkflowCommand::apply(
                    scope_a,
                    state_b.workflow().revision(),
                    state_b.workflow().clone(),
                ))
                .await,
            Err(OperationError::Conflict(_))
        ));
    }

    /// Run against a disposable Spiral project with:
    ///
    /// ```text
    /// SPIRAL_PROJECT_ID=project-id cargo test -p spiral-robotics-backend \
    ///   backend::workflow::manager::tests::spiral_sql_workflows_persist_and_cancel \
    ///   -- --ignored --exact
    /// ```
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a real SpiralDB server and disposable project access"]
    async fn spiral_sql_workflows_persist_and_cancel() {
        let stored = BackendStateStore::open_default().ok();
        let project = std::env::var("SPIRAL_PROJECT_ID")
            .ok()
            .filter(|project| !project.trim().is_empty())
            .or_else(|| stored.as_ref().and_then(BackendStateStore::project_id))
            .expect("SPIRAL_PROJECT_ID or a project saved in Studio Settings is required");
        let table_prefix = format!(
            "workflow_e2e_{}_{}",
            std::process::id(),
            SystemTime::now().unix_millis()
        );
        let connection = SpiralConnectionSettings::new(
            stored
                .as_ref()
                .and_then(BackendStateStore::spiral_endpoint)
                .unwrap_or_else(|| ServiceEndpoint::environment_spiral().into_string()),
            stored
                .as_ref()
                .and_then(BackendStateStore::spiral_spfs_endpoint)
                .unwrap_or_else(|| ServiceEndpoint::environment_spiral_spfs().into_string()),
        )
        .and_then(|settings| settings.with_project(&project))
        .expect("the Workflow E2E connection must be valid")
        .with_credentials(stored.as_ref().and_then(BackendStateStore::credentials));
        let project_store = Arc::new(
            {
                let settings = connection;
                let database =
                    Arc::new(spiral_robotics_storage::AdbcDatabase::new(settings.clone()));
                ProjectStore::new(settings, table_prefix.clone(), database)
            }
            .expect("the Workflow E2E project store must be valid"),
        );
        let seed_relation = format!("{table_prefix}_seed_values");
        let mut cleanup = WorkflowTestTables::new(
            project_store.clone(),
            vec![seed_relation.clone(), format!("{table_prefix}_workflows")],
            table_prefix.clone(),
        );
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(Int64Array::from(vec![3_i64, 1, 2])) as ArrayRef],
        )
        .expect("the seed batch must match its schema");
        let target = seed_relation.clone();
        let mut connection = project_store
            .connect_exclusive()
            .await
            .expect("the Workflow seed project must connect");
        connection
            .replace_or_create_batches(&target, vec![batch])
            .await
            .expect("seeding the Workflow relation must succeed");
        // The app provisions managed relations at project setup; this isolated
        // test namespace provisions only the workflow relation it exercises.
        let workflow_schema = Arc::new(Schema::new(vec![
            Field::new("workflow_id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("graph_version", DataType::UInt32, false),
            Field::new("graph_json", DataType::Utf8, false),
            Field::new("conversation_version", DataType::UInt32, true),
            Field::new("conversation_json", DataType::Utf8, true),
            Field::new("updated_ms", DataType::Int64, false),
        ]));
        connection
            .replace_or_create_batches(
                &format!("{table_prefix}_workflows"),
                vec![RecordBatch::new_empty(workflow_schema)],
            )
            .await
            .unwrap();
        drop(connection);

        let storage = Arc::new(StorageManager::new(project_store.clone()));
        let manager = WorkflowManager::new(storage.clone());
        manager
            .prepare_from_storage()
            .await
            .expect("an empty workflow namespace must hydrate");
        let initial = WorkflowSpec::default();
        let mut workflow = WorkflowSpec::new(manager.state().workflow().id(), "Ordered samples");
        workflow.nodes_mut().extend_from_slice(initial.nodes());
        workflow.edges_mut().extend_from_slice(initial.edges());
        let WorkflowNodeConfig::Table { relation } = workflow.nodes_mut()[0].config_mut() else {
            unreachable!("the default workflow starts with a Table")
        };
        *relation = seed_relation;
        let saved = manager
            .command(WorkflowCommand::apply(manager.scope_id(), 0, workflow))
            .await
            .expect("the valid SQL workflow must save");
        let scope_id = manager.scope_id().to_owned();
        let revision = saved.workflow().revision();
        manager
            .start(&scope_id, revision)
            .await
            .expect("the first run must start");
        assert!(
            matches!(
                manager.start(&scope_id, revision).await,
                Err(OperationError::Conflict(_))
            ),
            "one workflow cannot have two active runs"
        );
        let completed = wait_for_terminal(&manager).await;
        assert_eq!(
            completed.phase(),
            WorkflowRunPhase::Completed,
            "Workflow failed: {:?}",
            completed.error()
        );
        assert_eq!(
            completed.result().map(|result| result.rows().len()),
            Some(3)
        );

        assert!(
            storage.workflows().await.unwrap().is_empty(),
            "authoring and running must not autosave"
        );
        let checkpoint_chat =
            spiral_robotics_storage::workflow_assistant::WorkflowConversation::new(
                saved.workflow().id(),
            );
        manager.checkpoint(&checkpoint_chat).await.unwrap();
        assert!(!manager.state().unsaved_changes());
        let reloaded = WorkflowManager::new(storage.clone());
        reloaded
            .prepare_from_storage()
            .await
            .expect("the saved graph and result must reload");
        assert_eq!(reloaded.state().workflow(), saved.workflow());
        assert_eq!(reloaded.state().snapshot().result(), completed.result());

        reloaded
            .start(&reloaded.scope_id(), revision)
            .await
            .expect("the persisted workflow must run again");
        let cancelling = reloaded
            .cancel(&reloaded.scope_id())
            .await
            .expect("the active run must cancel");
        assert_eq!(cancelling.phase(), WorkflowRunPhase::Cancelling);
        let cancelled = wait_for_terminal(&reloaded).await;
        assert_eq!(cancelled.phase(), WorkflowRunPhase::Cancelled);
        assert!(cancelled.result_stale());
        assert_eq!(cancelled.result(), completed.result());
        // Named workflow storage and conversation ownership use the same real
        // Spiral transaction path as the graph and result above.
        let first_id = saved.workflow().id().to_owned();
        let mut first_chat =
            spiral_robotics_storage::workflow_assistant::WorkflowConversation::new(&first_id);
        first_chat.provider_messages_mut().push(
            spiral_robotics_storage::workflow_assistant::ProviderMessage::text(
                "user",
                "Keep the first workflow context",
            ),
        );
        reloaded.checkpoint(&first_chat).await.unwrap();
        let first_scope = reloaded.scope_id();
        let second = reloaded
            .command(WorkflowCommand::create(&first_scope))
            .await
            .unwrap();
        let second_id = second.workflow().id().to_owned();
        assert_ne!(first_id, second_id);
        assert!(second.workflow().nodes().is_empty());
        let mut renamed = second.workflow().clone();
        renamed.set_name("Second workflow");
        reloaded
            .command(WorkflowCommand::apply(reloaded.scope_id(), 0, renamed))
            .await
            .unwrap();
        let second_chat =
            spiral_robotics_storage::workflow_assistant::WorkflowConversation::new(&second_id);
        reloaded.checkpoint(&second_chat).await.unwrap();
        assert!(
            reloaded
                .command(WorkflowCommand::apply(
                    first_scope,
                    revision,
                    saved.workflow().clone()
                ))
                .await
                .is_err()
        );
        let list = reloaded.state();
        assert_eq!(list.workflows().len(), 2);
        let restarted = WorkflowManager::new(storage.clone());
        restarted.ensure_loaded().await.unwrap();
        assert_eq!(restarted.state().workflow().id(), second_id);
        assert_eq!(restarted.state().workflow().name(), "Second workflow");
        restarted
            .command(WorkflowCommand::select(restarted.scope_id(), &first_id))
            .await
            .unwrap();
        let reselected = WorkflowManager::new(storage.clone());
        reselected.ensure_loaded().await.unwrap();
        assert_eq!(
            reselected.state().workflow().id(),
            second_id,
            "selection does not save; reconnect opens the most recently saved workflow"
        );
        let first_json = storage
            .load_workflow_conversation(&first_id)
            .await
            .unwrap()
            .unwrap();
        let first_chat = spiral_robotics_storage::workflow_assistant::WorkflowConversation::load(
            &first_json,
            &first_id,
        )
        .unwrap();
        assert_eq!(
            first_chat.provider_messages()[0].content(),
            Some("Keep the first workflow context")
        );
        let second_json = storage
            .load_workflow_conversation(&second_id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            spiral_robotics_storage::workflow_assistant::WorkflowConversation::load(
                &second_json,
                &second_id
            )
            .unwrap()
            .provider_messages()
            .is_empty()
        );
        restarted.shutdown().await;
        reselected.shutdown().await;
        reloaded.shutdown().await;
        manager.shutdown().await;
        cleanup
            .cleanup_now()
            .await
            .expect("the SQL E2E tables must clean up");
        cleanup
            .assert_absent()
            .await
            .expect("the SQL E2E must leave no catalog resources behind");
    }

    async fn wait_for_terminal(manager: &WorkflowManager) -> WorkflowRunSnapshot {
        for _ in 0..2400 {
            let snapshot = manager.state().snapshot().clone();
            if snapshot.phase().terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "Workflow run did not reach a terminal state within 60 seconds: {:?}",
            manager.state().snapshot()
        );
    }

    struct WorkflowTestTables {
        project_store: Arc<ProjectStore>,
        relations: Vec<String>,
        table_prefix: String,
        cleaned: bool,
    }

    impl WorkflowTestTables {
        fn new(
            project_store: Arc<ProjectStore>,
            relations: Vec<String>,
            table_prefix: String,
        ) -> Self {
            Self {
                project_store,
                relations,
                table_prefix,
                cleaned: false,
            }
        }

        async fn cleanup_now(&mut self) -> Result<(), String> {
            if self.cleaned {
                return Ok(());
            }
            for relation in self.relations.clone() {
                self.drop_cleanup_table(&relation).await?;
            }
            self.cleaned = true;
            Ok(())
        }

        async fn drop_cleanup_table(&self, relation: &str) -> Result<(), String> {
            const MAX_ATTEMPTS: usize = 32;
            for attempt in 1..=MAX_ATTEMPTS {
                let mut connection = self.project_store.connect_exclusive().await?;
                let result = connection
                    .execute_update_sql(&format!("DROP TABLE IF EXISTS {relation}"))
                    .await;
                match result {
                    Ok(()) => return Ok(()),
                    Err(error)
                        if error.contains("project namespace compare-and-swap conflict")
                            && attempt < MAX_ATTEMPTS =>
                    {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    Err(error) => return Err(error),
                }
            }
            unreachable!("the bounded cleanup loop returns on its final attempt")
        }

        async fn assert_absent(&self) -> Result<(), String> {
            let table_prefix = self.table_prefix.clone();
            let relations = self.relations.clone();
            let mut connection = self.project_store.connect_exclusive().await?;
            let leaked = connection
                .catalog_relation_names()
                .await?
                .into_iter()
                .filter(|relation_name| {
                    relation_name == &table_prefix
                        || relation_name.starts_with(&format!("{table_prefix}_"))
                        || relations.iter().any(|relation| relation == relation_name)
                })
                .collect::<Vec<_>>();
            if leaked.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "Workflow test leaked catalog resources: {}",
                    leaked.join(", ")
                ))
            }
        }
    }
}
