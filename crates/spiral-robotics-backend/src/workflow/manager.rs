//! Persisted Table, Model, Query, and Result execution over Spiral relations.

use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::OperationError;
use crate::clock::SystemTimeMillis;
use crate::sync::MutexRecovery;
use crate::workflow::service::{
    WorkflowCancellationAdmission, WorkflowRunAdmission, WorkflowSaveAdmission,
};
use serde::{Deserialize, Serialize};
use spiral_robotics_api::relations::{RelationCatalog, SpiralQueryResponse};
use spiral_robotics_api::sql::{QualifiedRelation, WorkflowSql};
use spiral_robotics_api::workflow::{
    WorkflowCommand, WorkflowNodeConfig, WorkflowNodePhase, WorkflowNodeSnapshot,
    WorkflowQuarantineSummary, WorkflowRunPhase, WorkflowRunSnapshot, WorkflowSpec,
    WorkflowStateResponse,
};
use tokio::sync::{Mutex as AsyncMutex, broadcast, mpsc};

use crate::workflow::model::{
    ModelRunContext, ModelRunEvent, ModelRunFailure, ModelRunRequest, ModelRunResult,
};
use spiral_robotics_storage::StorageManager;

/// Persistence is a backend implementation detail. The shared API exposes the
/// graph and observation, while the manager decides that they commit together.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct WorkflowDocument {
    workflow: WorkflowSpec,
    snapshot: WorkflowRunSnapshot,
}

impl Default for WorkflowDocument {
    fn default() -> Self {
        let workflow = WorkflowSpec::default();
        let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        Self { workflow, snapshot }
    }
}

impl WorkflowDocument {
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
}

struct WorkflowSave {
    scope_id: String,
    expected_revision: u64,
    workflow: WorkflowSpec,
}

#[derive(Clone)]
pub(crate) struct WorkflowManager {
    inner: Arc<WorkflowManagerInner>,
}

struct WorkflowManagerInner {
    storage: Arc<StorageManager>,
    identity: WorkflowIdentity,
    document: Mutex<WorkflowDocument>,
    updates: broadcast::Sender<WorkflowStateResponse>,
    active: Mutex<Option<ActiveRun>>,
    persistence: WorkflowPersistenceState,
}

struct WorkflowPersistenceState {
    mutation: AsyncMutex<()>,
    terminal_error: Mutex<Option<TerminalPersistenceError>>,
    #[cfg(test)]
    test_outcomes: Mutex<VecDeque<Result<(), String>>>,
}

impl WorkflowPersistenceState {
    fn new() -> Self {
        Self {
            mutation: AsyncMutex::new(()),
            terminal_error: Mutex::new(None),
            #[cfg(test)]
            test_outcomes: Mutex::new(VecDeque::new()),
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
    cancel: Arc<AtomicBool>,
    publication_gate: Arc<AsyncMutex<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
    terminal_persistence_failed: bool,
}

#[derive(Clone)]
struct TerminalPersistenceError {
    run_id: String,
    detail: String,
}

#[derive(Clone, Copy)]
enum FailureCancellationPolicy {
    CancelWins,
    FailureWins,
}

impl FailureCancellationPolicy {
    fn supersedes_failure(self, cancel_requested: bool) -> bool {
        cancel_requested && matches!(self, Self::CancelWins)
    }
}

struct WorkflowExecution {
    run_id: String,
    cancel: Arc<AtomicBool>,
    publication_gate: Arc<AsyncMutex<()>>,
}

impl WorkflowExecution {
    fn new(run_id: String, cancel: Arc<AtomicBool>, publication_gate: Arc<AsyncMutex<()>>) -> Self {
        Self {
            run_id,
            cancel,
            publication_gate,
        }
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

struct ModelNodeCompletion<'a> {
    target: WorkflowNodeTarget<'a>,
    started_ms: u64,
    result: ModelRunResult,
}

impl<'a> ModelNodeCompletion<'a> {
    fn new(target: WorkflowNodeTarget<'a>, started_ms: u64, result: ModelRunResult) -> Self {
        Self {
            target,
            started_ms,
            result,
        }
    }
}

struct WorkflowFailure<'a> {
    run_id: &'a str,
    node_id: Option<&'a str>,
    error: String,
    policy: FailureCancellationPolicy,
}

impl<'a> WorkflowFailure<'a> {
    fn new(run_id: &'a str, error: String) -> Self {
        Self {
            run_id,
            node_id: None,
            error,
            policy: FailureCancellationPolicy::CancelWins,
        }
    }

    fn at_node(mut self, node_id: Option<&'a str>) -> Self {
        self.node_id = node_id;
        self
    }

    fn with_policy(mut self, policy: FailureCancellationPolicy) -> Self {
        self.policy = policy;
        self
    }
}

struct TerminalPersistenceFailure<'a> {
    run_id: &'a str,
    document: &'a WorkflowDocument,
    error: String,
}

impl<'a> TerminalPersistenceFailure<'a> {
    fn new(run_id: &'a str, document: &'a WorkflowDocument, error: String) -> Self {
        Self {
            run_id,
            document,
            error,
        }
    }
}

impl WorkflowManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        let (updates, _) = broadcast::channel(128);
        Self {
            inner: Arc::new(WorkflowManagerInner {
                storage,
                identity: WorkflowIdentity::new(),
                document: Mutex::new(WorkflowDocument::default()),
                updates,
                active: Mutex::new(None),
                persistence: WorkflowPersistenceState::new(),
            }),
        }
    }

    /// Restore the saved graph and latest snapshot. Local SQL execution is not
    /// resumable, so an in-flight snapshot becomes explicitly interrupted.
    pub(crate) async fn hydrate_from_storage(&self) -> Result<(), String> {
        let document = self.inner.storage.load_workflow_document().await?;
        self.install_hydrated_document(document).await
    }

    pub(crate) async fn reload_from_storage(&self) -> Result<(), String> {
        let document = self.inner.storage.load_workflow_document().await?;
        self.install_hydrated_document(document).await
    }

    async fn install_hydrated_document(
        &self,
        mut document: WorkflowDocument,
    ) -> Result<(), String> {
        if document.snapshot().phase().active() {
            let interruption =
                "Studio restarted before this Workflow run finished. Run it again.".to_owned();
            let finished_ms = SystemTime::now().unix_millis();
            let snapshot = document.snapshot_mut();
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
            self.inner.storage.save_workflow_document(&document).await?;
        }
        *self.inner.document.recover_lock() = document;
        self.inner.identity.advance_scope();
        *self.inner.persistence.terminal_error.recover_lock() = None;
        let _ = self.inner.updates.send(self.state());
        Ok(())
    }

    pub(crate) fn state(&self) -> WorkflowStateResponse {
        let document = self.inner.document.recover_lock();
        self.state_response(&document)
    }

    pub(crate) fn scope_id(&self) -> String {
        self.inner.identity.scope_id.recover_lock().clone()
    }

    pub(crate) async fn relations(&self) -> Result<RelationCatalog, String> {
        let scope_id = self.scope_id();
        let response = self.inner.storage.workflow_relations().await?;
        if self.scope_id() != scope_id {
            return Err(
                "The Spiral project changed while Workflow relations were loading. Reload them."
                    .to_owned(),
            );
        }
        Ok(response.with_scope(scope_id))
    }

    pub(crate) fn is_active(&self) -> bool {
        self.inner
            .document
            .recover_lock()
            .snapshot()
            .phase()
            .active()
    }

    /// Resolve server-owned Model outputs and validate an agent-authored graph
    /// without mutating the saved Workflow.
    pub(crate) fn prepare_candidate(
        &self,
        mut workflow: WorkflowSpec,
    ) -> Result<
        (
            WorkflowSpec,
            spiral_robotics_api::workflow::WorkflowValidationReport,
        ),
        OperationError,
    > {
        let current = self.inner.document.recover_lock();
        if workflow.id() != current.workflow().id() {
            return Err(OperationError::InvalidInput(format!(
                "Workflow identity {:?} does not match the server-owned identity {:?}",
                workflow.id(),
                current.workflow().id()
            )));
        }
        workflow.set_revision(current.workflow().revision());
        workflow.derive_model_outputs();
        let report = workflow.validate();
        Ok((workflow, report))
    }

    pub(crate) fn subscription(
        &self,
    ) -> (
        WorkflowStateResponse,
        broadcast::Receiver<WorkflowStateResponse>,
    ) {
        let document = self.inner.document.recover_lock();
        let updates = self.inner.updates.subscribe();
        (self.state_response(&document), updates)
    }

    pub(crate) async fn command(
        &self,
        command: WorkflowCommand,
    ) -> Result<WorkflowStateResponse, OperationError> {
        match command {
            WorkflowCommand::Apply {
                scope_id,
                expected_revision,
                workflow,
            } => {
                self.save(WorkflowSave {
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

    async fn save(&self, request: WorkflowSave) -> Result<WorkflowStateResponse, OperationError> {
        let WorkflowSave {
            scope_id,
            expected_revision,
            workflow: requested_workflow,
        } = request;
        self.require_scope(&scope_id)?;
        let _mutation = self.inner.persistence.mutation.lock().await;
        let current = self.inner.document.recover_lock().clone();
        if requested_workflow.id() != current.workflow().id() {
            return Err(OperationError::InvalidInput(format!(
                "Workflow identity {:?} does not match the server-owned identity {:?}",
                requested_workflow.id(),
                current.workflow().id()
            )));
        }
        let next_revision =
            WorkflowSaveAdmission::new(expected_revision, current.workflow().revision())
                .with_run_active(current.snapshot().phase().active())
                .next_revision()?;
        let mut workflow = requested_workflow;
        workflow.set_revision(next_revision);
        workflow.derive_model_outputs();
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
        let candidate = WorkflowDocument::new(workflow, snapshot);
        self.inner
            .storage
            .save_workflow_document(&candidate)
            .await
            .map_err(OperationError::External)?;
        let response = self.state_response(&candidate);
        *self.inner.document.recover_lock() = candidate;
        let _ = self.inner.updates.send(response.clone());
        Ok(response)
    }

    async fn start(
        &self,
        scope_id: &str,
        revision: u64,
    ) -> Result<WorkflowRunSnapshot, OperationError> {
        self.require_scope(scope_id)?;
        let _mutation = self.inner.persistence.mutation.lock().await;
        // When both locks are required, active-run admission always precedes
        // the document. Cancel and terminal publication use the same order.
        let active_exists = self.inner.active.recover_lock().is_some();
        let document = self.inner.document.recover_lock().clone();
        let validation = document.workflow().validate();
        WorkflowRunAdmission::new(revision, document.workflow().revision())
            .with_state(
                active_exists || document.snapshot().phase().active(),
                validation.valid(),
            )
            .validate()?;

        let run_id = self.inner.identity.next_run_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let publication_gate = Arc::new(AsyncMutex::new(()));
        let snapshot = document
            .workflow()
            .starting_snapshot(document.snapshot(), run_id.clone());
        let candidate = WorkflowDocument::new(document.workflow().clone(), snapshot.clone());
        self.inner
            .storage
            .save_workflow_document(&candidate)
            .await
            .map_err(OperationError::External)?;
        *self.inner.document.recover_lock() = candidate;
        *self.inner.active.recover_lock() = Some(ActiveRun {
            run_id: run_id.clone(),
            cancel: cancel.clone(),
            publication_gate: publication_gate.clone(),
            task: None,
            terminal_persistence_failed: false,
        });
        let _ = self.inner.updates.send(self.state());

        let manager = self.clone();
        let task_run_id = run_id.clone();
        let execution = WorkflowExecution::new(run_id, cancel, publication_gate);
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
            active.as_mut().map(|run| {
                (
                    run.publication_gate.clone(),
                    run.cancel.clone(),
                    run.task.take(),
                )
            })
        };
        if let Some((publication_gate, cancel, task)) = control {
            let publication = publication_gate.lock().await;
            cancel.store(true, Ordering::SeqCst);
            drop(publication);
            if let Some(task) = task {
                let _ = task.await;
            }
        }
    }

    async fn cancel(&self, scope_id: &str) -> Result<WorkflowRunSnapshot, OperationError> {
        self.require_scope(scope_id)?;
        let _mutation = self.inner.persistence.mutation.lock().await;
        // Keep admission and Cancelling publication in one active -> document
        // critical section. Otherwise execution could publish Cancelled and
        // clear `active` between these steps, only for this request to regress
        // the persisted snapshot back to Cancelling.
        let (recover_terminal_persistence, run_id, publication_gate, cancel) = {
            let active = self.inner.active.recover_lock();
            WorkflowCancellationAdmission::new(active.is_some()).validate()?;
            let control = active.as_ref().expect("admission requires an active run");
            (
                control.terminal_persistence_failed,
                control.run_id.clone(),
                control.publication_gate.clone(),
                control.cancel.clone(),
            )
        };
        let publication = publication_gate.lock().await;
        cancel.store(true, Ordering::SeqCst);
        let document = self.inner.document.recover_lock().clone();
        if document.snapshot().run_id() != Some(run_id.as_str()) {
            return Err(OperationError::Conflict(
                "The active Workflow run changed before cancellation.".to_owned(),
            ));
        }
        let mut candidate = document.clone();
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
        self.save_document(&candidate)
            .await
            .map_err(OperationError::External)?;
        let snapshot = candidate.snapshot().clone();
        let response = self.state_response(&candidate);
        *self.inner.document.recover_lock() = candidate;
        let _ = self.inner.updates.send(response);
        drop(publication);
        if recover_terminal_persistence {
            drop(_mutation);
            self.finish_cancelled(&run_id).await;
            return Ok(self.state().snapshot().clone());
        }
        Ok(snapshot)
    }

    async fn execute(&self, execution: WorkflowExecution) {
        let WorkflowExecution {
            run_id,
            cancel,
            publication_gate,
        } = execution;
        let workflow = self.inner.document.recover_lock().workflow().clone();
        let order = match workflow.topological_order() {
            Ok(order) => order,
            Err(error) => {
                self.finish_failed(WorkflowFailure::new(&run_id, error))
                    .await;
                return;
            }
        };
        let discovered = match self.inner.storage.workflow_relations().await {
            Ok(relations) => relations,
            Err(error) => {
                self.finish_failed(
                    WorkflowFailure::new(&run_id, format!("Discovering Spiral relations: {error}"))
                        .at_node(workflow.first_table_id()),
                )
                .await;
                return;
            }
        };
        let discovered = discovered
            .relations()
            .iter()
            .map(|relation| (relation.name(), relation.columns().len()))
            .collect::<BTreeMap<_, _>>();
        let mut outputs = BTreeMap::new();
        let mut query_outputs: BTreeMap<String, SpiralQueryResponse> = BTreeMap::new();
        let mut terminal_result = None;

        if self.cancelled(&run_id, &cancel) {
            self.finish_cancelled(&run_id).await;
            return;
        }
        self.set_run_phase(&run_id, WorkflowRunPhase::Running).await;
        for node_id in order {
            if self.cancelled(&run_id, &cancel) {
                self.finish_cancelled(&run_id).await;
                return;
            }
            let Some(node) = workflow.nodes().iter().find(|node| node.id() == node_id) else {
                self.finish_failed(WorkflowFailure::new(
                    &run_id,
                    format!("Unknown node {node_id:?}"),
                ))
                .await;
                return;
            };
            let node_started = SystemTime::now().unix_millis();
            match node.config() {
                WorkflowNodeConfig::Table { relation } => {
                    self.set_node_phase(WorkflowNodePhaseChange::new(
                        WorkflowNodeTarget::new(&run_id, node.id()),
                        WorkflowNodePhase::Validating,
                        "Resolving Spiral relation".to_owned(),
                    ))
                    .await;
                    let Some(field_count) = discovered.get(relation.as_str()) else {
                        self.finish_failed(
                            WorkflowFailure::new(
                                &run_id,
                                format!(
                                    "Spiral relation {relation:?} is not in the active project catalog. Choose another relation and Apply."
                                ),
                            )
                            .at_node(Some(node.id())),
                        )
                        .await;
                        return;
                    };
                    outputs.insert(
                        node.id().to_owned(),
                        ResolvedNodeOutput::Relation(relation.clone()),
                    );
                    self.complete_node(WorkflowNodeCompletion::new(
                        WorkflowNodeTarget::new(&run_id, node.id()),
                        node_started,
                        format!("Resolved {field_count} fields"),
                    ))
                    .await;
                }
                WorkflowNodeConfig::Query { sql, max_rows } => {
                    let substituted = match outputs.substitute_in(sql) {
                        Ok(sql) => sql,
                        Err(error) => {
                            self.finish_failed(
                                WorkflowFailure::new(&run_id, error.to_string())
                                    .at_node(Some(node.id())),
                            )
                            .await;
                            return;
                        }
                    };
                    let feeds_result = workflow.edges().iter().any(|edge| {
                        edge.source() == node.id()
                            && workflow.nodes().iter().any(|target| {
                                target.id() == edge.target()
                                    && matches!(target.config(), WorkflowNodeConfig::Result)
                            })
                    });
                    if !feeds_result {
                        outputs
                            .insert(node.id().to_owned(), ResolvedNodeOutput::Query(substituted));
                        self.complete_node(WorkflowNodeCompletion::new(
                            WorkflowNodeTarget::new(&run_id, node.id()),
                            node_started,
                            "Resolved read-only candidate SQL".to_owned(),
                        ))
                        .await;
                        continue;
                    }
                    self.set_node_phase(WorkflowNodePhaseChange::new(
                        WorkflowNodeTarget::new(&run_id, node.id()),
                        WorkflowNodePhase::Running,
                        "Running bounded read-only SQL".to_owned(),
                    ))
                    .await;
                    let response = match self.inner.storage.query(&substituted, *max_rows).await {
                        Ok(response) => response,
                        Err(error) => {
                            if self.cancelled(&run_id, &cancel) {
                                self.finish_cancelled(&run_id).await;
                            } else {
                                self.finish_failed(
                                    WorkflowFailure::new(&run_id, error).at_node(Some(node.id())),
                                )
                                .await;
                            }
                            return;
                        }
                    };
                    if self.cancelled(&run_id, &cancel) {
                        self.finish_cancelled(&run_id).await;
                        return;
                    }
                    let rows = response.rows().len();
                    query_outputs.insert(node.id().to_owned(), response);
                    outputs.insert(node.id().to_owned(), ResolvedNodeOutput::Query(substituted));
                    self.complete_node(
                        WorkflowNodeCompletion::new(
                            WorkflowNodeTarget::new(&run_id, node.id()),
                            node_started,
                            format!("Returned {rows} rows"),
                        )
                        .with_rows(Some(rows)),
                    )
                    .await;
                }
                WorkflowNodeConfig::Result => {
                    self.set_node_phase(WorkflowNodePhaseChange::new(
                        WorkflowNodeTarget::new(&run_id, node.id()),
                        WorkflowNodePhase::Running,
                        "Preparing result payload".to_owned(),
                    ))
                    .await;
                    let source = workflow
                        .edges()
                        .iter()
                        .find(|edge| edge.target() == node.id())
                        .map(|edge| edge.source());
                    let Some(result) = source.and_then(|source| query_outputs.get(source)).cloned()
                    else {
                        self.finish_failed(
                            WorkflowFailure::new(
                                &run_id,
                                "Result has no completed Query payload.".to_owned(),
                            )
                            .at_node(Some(node.id())),
                        )
                        .await;
                        return;
                    };
                    let rows = result.rows().len();
                    terminal_result = Some(result);
                    self.complete_node(
                        WorkflowNodeCompletion::new(
                            WorkflowNodeTarget::new(&run_id, node.id()),
                            node_started,
                            format!("Ready · {rows} rows"),
                        )
                        .with_rows(Some(rows)),
                    )
                    .await;
                }
                WorkflowNodeConfig::Model { features } => {
                    let source = workflow
                        .edges()
                        .iter()
                        .find(|edge| edge.target() == node.id())
                        .map(|edge| edge.source());
                    let Some(candidate_sql) = source
                        .and_then(|source| outputs.get(source))
                        .and_then(|output| match output {
                            ResolvedNodeOutput::Query(sql) => Some(sql.clone()),
                            ResolvedNodeOutput::Relation(_) => None,
                        })
                    else {
                        self.finish_failed(
                            WorkflowFailure::new(
                                &run_id,
                                "Model has no resolved upstream candidate Query.".to_owned(),
                            )
                            .at_node(Some(node.id())),
                        )
                        .await;
                        return;
                    };
                    let Some(output_relation) = node.config().model_output_relation() else {
                        self.finish_failed(
                            WorkflowFailure::new(
                                &run_id,
                                "Model output relation was not derived by the server. Apply the workflow again."
                                    .to_owned(),
                            )
                            .at_node(Some(node.id())),
                        )
                        .await;
                        return;
                    };
                    let request = ModelRunRequest::new(
                        candidate_sql,
                        output_relation.clone(),
                        features.clone(),
                    );
                    let project_store = self.inner.storage.workflow_model_store();
                    let (event_sender, mut events) = mpsc::unbounded_channel();
                    let event_manager = self.clone();
                    let event_run_id = run_id.clone();
                    let event_node_id = node.id().to_owned();
                    let worker_cancel = cancel.clone();
                    let worker_publication_gate = publication_gate.clone();
                    let progress = async move {
                        while let Some(event) = events.recv().await {
                            event_manager
                                .set_model_event(
                                    WorkflowNodeTarget::new(&event_run_id, &event_node_id),
                                    event,
                                )
                                .await;
                        }
                    };
                    let model = ModelRunContext::new(project_store, request, worker_cancel)
                        .with_publication_gate(worker_publication_gate)
                        .run(event_sender);
                    let (result, ()) = tokio::join!(model, progress);
                    let result = match result {
                        Ok(result) => result,
                        Err(ModelRunFailure::Cancelled) => {
                            self.finish_cancelled(&run_id).await;
                            return;
                        }
                        Err(ModelRunFailure::Failed(error)) => {
                            self.finish_model_failed(
                                WorkflowNodeTarget::new(&run_id, node.id()),
                                error,
                            )
                            .await;
                            return;
                        }
                    };
                    outputs.insert(
                        node.id().to_owned(),
                        ResolvedNodeOutput::Relation(output_relation),
                    );
                    self.complete_model_node(ModelNodeCompletion::new(
                        WorkflowNodeTarget::new(&run_id, node.id()),
                        node_started,
                        result,
                    ))
                    .await;
                    if self.cancelled(&run_id, &cancel) {
                        // A successful worker result means the cache+registry
                        // transaction committed before cancellation acquired
                        // the publication gate. Preserve that Model truth, then
                        // stop before the next node.
                        self.finish_cancelled(&run_id).await;
                        return;
                    }
                }
            }
        }
        self.finish_completed(&run_id, terminal_result).await;
    }

    async fn set_model_event(&self, target: WorkflowNodeTarget<'_>, event: ModelRunEvent) {
        let WorkflowNodeTarget { run_id, node_id } = target;
        let _mutation = self.inner.persistence.mutation.lock().await;
        let mut document = self.inner.document.recover_lock().clone();
        if document.snapshot().run_id() != Some(run_id)
            || matches!(
                document.snapshot().phase(),
                WorkflowRunPhase::Cancelling | WorkflowRunPhase::Cancelled
            )
        {
            return;
        }
        let Some(node) = document
            .snapshot_mut()
            .nodes_mut()
            .iter_mut()
            .find(|node| node.node_id() == node_id)
        else {
            return;
        };
        if node.phase() == WorkflowNodePhase::Cancelling {
            return;
        }
        let mut materialization_invalidated = false;
        let (phase, detail, progress) = match event {
            ModelRunEvent::Loading(detail) => (WorkflowNodePhase::Loading, detail, None),
            ModelRunEvent::Validating(detail) => (WorkflowNodePhase::Validating, detail, None),
            ModelRunEvent::Materializing(detail) => {
                materialization_invalidated = true;
                (WorkflowNodePhase::Validating, detail, None)
            }
            ModelRunEvent::Running { detail, progress } => {
                (WorkflowNodePhase::Running, detail, Some(progress))
            }
        };
        let phase_changed = node.phase() != phase;
        node.set_phase(phase);
        node.set_detail(detail);
        node.set_progress(progress);
        node.set_error(None);
        if materialization_invalidated {
            node.set_model(None);
            node.set_quarantine(WorkflowQuarantineSummary::default());
            node.warnings_mut().clear();
        }
        document
            .snapshot_mut()
            .set_updated_ms(SystemTime::now().unix_millis());
        if phase_changed || materialization_invalidated {
            self.persist_publish(document).await;
        } else {
            let _ = self.inner.updates.send(self.state_response(&document));
            *self.inner.document.recover_lock() = document;
        }
    }

    async fn complete_model_node(&self, completion: ModelNodeCompletion<'_>) {
        let ModelNodeCompletion {
            target: WorkflowNodeTarget { run_id, node_id },
            started_ms,
            result,
        } = completion;
        let _mutation = self.inner.persistence.mutation.lock().await;
        let mut document = self.inner.document.recover_lock().clone();
        if document.snapshot().run_id() != Some(run_id) {
            return;
        }
        if let Some(node) = document
            .snapshot_mut()
            .nodes_mut()
            .iter_mut()
            .find(|node| node.node_id() == node_id)
        {
            let has_warnings = !result.warnings().is_empty() || result.quarantine().count() > 0;
            node.set_phase(if has_warnings {
                WorkflowNodePhase::CompleteWithWarnings
            } else {
                WorkflowNodePhase::Complete
            });
            node.set_detail(if result.metadata().reused() {
                format!(
                    "Feature cache hit · {} cells reused",
                    result.reused_feature_cells()
                )
            } else if has_warnings {
                format!(
                    "Backfilled {} feature cells with warnings · {} cache rows",
                    result.computed_feature_cells(),
                    result.rows()
                )
            } else {
                format!(
                    "Backfilled {} feature cells · {} reused · {} cache rows",
                    result.computed_feature_cells(),
                    result.reused_feature_cells(),
                    result.rows()
                )
            });
            node.set_elapsed_ms(SystemTime::now().unix_millis().saturating_sub(started_ms));
            node.set_rows(Some(result.rows()));
            node.set_error(None);
            node.set_progress(Some(
                spiral_robotics_api::workflow::WorkflowNodeProgress::new(
                    result
                        .computed_feature_cells()
                        .saturating_add(result.reused_feature_cells()),
                    result.requested_feature_cells(),
                    "feature cells",
                ),
            ));
            node.set_quarantine(result.quarantine().clone());
            node.set_model(Some(result.metadata().clone()));
            *node.warnings_mut() = result.warnings().to_vec();
        }
        document
            .snapshot_mut()
            .set_updated_ms(SystemTime::now().unix_millis());
        self.persist_publish(document).await;
    }

    async fn set_run_phase(&self, run_id: &str, phase: WorkflowRunPhase) {
        let _mutation = self.inner.persistence.mutation.lock().await;
        let mut document = self.inner.document.recover_lock().clone();
        if document.snapshot().run_id() != Some(run_id) {
            return;
        }
        document.snapshot_mut().set_phase(phase);
        document
            .snapshot_mut()
            .set_updated_ms(SystemTime::now().unix_millis());
        self.persist_publish(document).await;
    }

    async fn set_node_phase(&self, change: WorkflowNodePhaseChange<'_>) {
        let WorkflowNodePhaseChange {
            target: WorkflowNodeTarget { run_id, node_id },
            phase,
            detail,
            error,
        } = change;
        let _mutation = self.inner.persistence.mutation.lock().await;
        let mut document = self.inner.document.recover_lock().clone();
        if document.snapshot().run_id() != Some(run_id) {
            return;
        }
        if let Some(node) = document
            .snapshot_mut()
            .nodes_mut()
            .iter_mut()
            .find(|node| node.node_id() == node_id)
        {
            node.set_phase(phase);
            node.set_detail(detail);
            node.set_error(error);
        }
        document
            .snapshot_mut()
            .set_updated_ms(SystemTime::now().unix_millis());
        self.persist_publish(document).await;
    }

    async fn complete_node(&self, completion: WorkflowNodeCompletion<'_>) {
        let WorkflowNodeCompletion {
            target: WorkflowNodeTarget { run_id, node_id },
            started_ms,
            detail,
            rows,
        } = completion;
        let _mutation = self.inner.persistence.mutation.lock().await;
        let mut document = self.inner.document.recover_lock().clone();
        if document.snapshot().run_id() != Some(run_id) {
            return;
        }
        if let Some(node) = document
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
        document
            .snapshot_mut()
            .set_updated_ms(SystemTime::now().unix_millis());
        self.persist_publish(document).await;
    }

    async fn finish_completed(&self, run_id: &str, result: Option<SpiralQueryResponse>) {
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
            .map(|control| control.cancel.load(Ordering::SeqCst))
        else {
            return;
        };
        if cancelled {
            drop(mutation);
            self.finish_cancelled(run_id).await;
            return;
        }
        let document = self.inner.document.recover_lock().clone();
        if document.snapshot().run_id() != Some(run_id) {
            return;
        }
        let mut candidate = document.clone();
        let finished = SystemTime::now().unix_millis();
        candidate
            .snapshot_mut()
            .set_phase(WorkflowRunPhase::Completed);
        candidate.snapshot_mut().set_finished_ms(finished);
        candidate.snapshot_mut().set_updated_ms(finished);
        candidate.snapshot_mut().set_error(None);
        candidate.snapshot_mut().set_result(result);
        candidate.snapshot_mut().set_result_stale(false);
        if let Err(error) = self.save_document(&candidate).await {
            if let Some(control) = self.inner.active.recover_lock().as_mut() {
                control.terminal_persistence_failed = true;
            }
            self.report_terminal_persistence_failure(TerminalPersistenceFailure::new(
                run_id, &document, error,
            ));
            return;
        }
        self.publish_committed_terminal(&candidate);
        *self.inner.document.recover_lock() = candidate;
        *self.inner.active.recover_lock() = None;
    }

    async fn finish_failed(&self, failure: WorkflowFailure<'_>) {
        self.finish_failed_with_policy(failure).await;
    }

    async fn finish_model_failed(&self, target: WorkflowNodeTarget<'_>, error: String) {
        // `run_model` classifies cancellation while it still owns the
        // publication boundary. A typed Failed outcome—including an uncertain
        // rollback—must not be rewritten as Cancelled by a token set after
        // that failure crossed the gate.
        self.finish_failed_with_policy(
            WorkflowFailure::new(target.run_id, error)
                .at_node(Some(target.node_id))
                .with_policy(FailureCancellationPolicy::FailureWins),
        )
        .await;
    }

    async fn finish_failed_with_policy(&self, failure: WorkflowFailure<'_>) {
        let WorkflowFailure {
            run_id,
            node_id,
            error,
            policy,
        } = failure;
        let mutation = self.inner.persistence.mutation.lock().await;
        let Some(cancelled) = self
            .inner
            .active
            .recover_lock()
            .as_ref()
            .filter(|active| active.run_id == run_id)
            .map(|control| control.cancel.load(Ordering::SeqCst))
        else {
            return;
        };
        if policy.supersedes_failure(cancelled) {
            drop(mutation);
            self.finish_cancelled(run_id).await;
            return;
        }
        let document = self.inner.document.recover_lock().clone();
        if document.snapshot().run_id() != Some(run_id) {
            return;
        }
        let mut candidate = document.clone();
        if let Some(node_id) = node_id
            && let Some(node) = candidate
                .snapshot_mut()
                .nodes_mut()
                .iter_mut()
                .find(|node| node.node_id() == node_id)
        {
            node.set_phase(WorkflowNodePhase::Failed);
            node.set_detail("Failed");
            node.set_error(Some(error.clone()));
        }
        let finished = SystemTime::now().unix_millis();
        candidate.snapshot_mut().set_phase(WorkflowRunPhase::Failed);
        candidate.snapshot_mut().set_finished_ms(finished);
        candidate.snapshot_mut().set_updated_ms(finished);
        candidate.snapshot_mut().set_error(Some(error));
        if let Err(error) = self.save_document(&candidate).await {
            if let Some(control) = self.inner.active.recover_lock().as_mut() {
                control.terminal_persistence_failed = true;
            }
            self.report_terminal_persistence_failure(TerminalPersistenceFailure::new(
                run_id, &document, error,
            ));
            return;
        }
        self.publish_committed_terminal(&candidate);
        *self.inner.document.recover_lock() = candidate;
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
        let document = self.inner.document.recover_lock().clone();
        if document.snapshot().run_id() != Some(run_id) {
            return;
        }
        let mut candidate = document.clone();
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
        if let Err(error) = self.save_document(&candidate).await {
            if let Some(control) = self.inner.active.recover_lock().as_mut() {
                control.terminal_persistence_failed = true;
            }
            self.report_terminal_persistence_failure(TerminalPersistenceFailure::new(
                run_id, &document, error,
            ));
            return;
        }
        self.publish_committed_terminal(&candidate);
        *self.inner.document.recover_lock() = candidate;
        *self.inner.active.recover_lock() = None;
    }

    fn cancelled(&self, run_id: &str, cancel: &AtomicBool) -> bool {
        cancel.load(Ordering::SeqCst)
            || self
                .inner
                .active
                .recover_lock()
                .as_ref()
                .is_none_or(|active| active.run_id != run_id)
    }

    async fn persist_publish(&self, document: WorkflowDocument) {
        if let Err(error) = self.save_document(&document).await {
            tracing::error!(%error, "persisting Workflow snapshot");
        }
        let _ = self.inner.updates.send(self.state_response(&document));
        *self.inner.document.recover_lock() = document;
    }

    async fn save_document(&self, document: &WorkflowDocument) -> Result<(), String> {
        #[cfg(test)]
        if let Some(outcome) = self
            .inner
            .persistence
            .test_outcomes
            .recover_lock()
            .pop_front()
        {
            return outcome;
        }
        self.inner.storage.save_workflow_document(document).await
    }

    fn require_scope(&self, requested_scope_id: &str) -> Result<(), OperationError> {
        if requested_scope_id != self.scope_id() {
            return Err(OperationError::Conflict(
                "The Spiral project changed before the Workflow command arrived. Reload Workflow and try again."
                    .to_owned(),
            ));
        }
        Ok(())
    }

    fn report_terminal_persistence_failure(&self, failure: TerminalPersistenceFailure<'_>) {
        let TerminalPersistenceFailure {
            run_id,
            document,
            error,
        } = failure;
        let detail = format!(
            "Could not durably finalize this Workflow run: {error} The run remains active; choose Cancel to retry durable finalization."
        );
        tracing::error!(%error, %run_id, "persisting terminal Workflow snapshot");
        *self.inner.persistence.terminal_error.recover_lock() = Some(TerminalPersistenceError {
            run_id: run_id.to_owned(),
            detail,
        });
        let _ = self.inner.updates.send(self.state_response(document));
    }

    fn publish_committed_terminal(&self, document: &WorkflowDocument) {
        *self.inner.persistence.terminal_error.recover_lock() = None;
        let _ = self.inner.updates.send(self.state_response(document));
    }

    fn state_response(&self, document: &WorkflowDocument) -> WorkflowStateResponse {
        let mut response = document.state_response();
        response.set_scope_id(self.scope_id());
        if let Some(error) = self
            .inner
            .persistence
            .terminal_error
            .recover_lock()
            .as_ref()
            && response.snapshot().run_id() == Some(error.run_id.as_str())
            && response.snapshot().phase().active()
        {
            let mut snapshot = response.snapshot().clone();
            snapshot.set_error(Some(error.detail.clone()));
            response.set_snapshot(snapshot);
        }
        response
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

trait WorkflowDocumentPresentation {
    fn state_response(&self) -> WorkflowStateResponse;
}

impl WorkflowDocumentPresentation for WorkflowDocument {
    fn state_response(&self) -> WorkflowStateResponse {
        let mut response =
            WorkflowStateResponse::from_workflow(String::new(), self.workflow().clone());
        response.set_snapshot(self.snapshot().clone());
        response
    }
}

#[derive(Clone)]
enum ResolvedNodeOutput {
    Relation(String),
    Query(String),
}

trait ResolvedNodeOutputs {
    fn substitute_in(&self, sql: &str) -> Result<String, String>;
}

impl ResolvedNodeOutputs for BTreeMap<String, ResolvedNodeOutput> {
    fn substitute_in(&self, sql: &str) -> Result<String, String> {
        let mut substituted = sql.to_owned();
        for id in WorkflowSql::from(sql)
            .node_placeholders()
            .map_err(|error| error.to_string())?
        {
            let output = self.get(&id).ok_or_else(|| {
                format!("Workflow query references unresolved node placeholder {id:?}")
            })?;
            let replacement = match output {
                ResolvedNodeOutput::Relation(relation) => {
                    QualifiedRelation::try_from(relation.as_str())
                        .map_err(|error| error.to_string())?;
                    relation.clone()
                }
                ResolvedNodeOutput::Query(query) => format!("({query})"),
            };
            substituted = substituted.replace(&format!("{{{{node:{id}}}}}"), &replacement);
        }
        Ok(substituted)
    }
}

trait WorkflowOutputs {
    fn derive_model_outputs(&mut self);
    fn model_source_relation(&self, model_id: &str) -> Option<String>;
    fn first_table_id(&self) -> Option<&str>;
    fn content_changed(&self, next: &Self) -> bool;
    fn saved_node_snapshots(
        &self,
        previous: &WorkflowRunSnapshot,
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
    fn derive_model_outputs(&mut self) {
        let source_relations = self
            .nodes()
            .iter()
            .filter(|node| matches!(node.config(), WorkflowNodeConfig::Model { .. }))
            .map(|node| (node.id().to_owned(), self.model_source_relation(node.id())))
            .collect::<BTreeMap<_, _>>();
        for node in self.nodes_mut() {
            let node_id = node.id().to_owned();
            if let WorkflowNodeConfig::Model { features } = node.config_mut() {
                let source = source_relations.get(&node_id).cloned().flatten();
                if let Some(source) = source.as_deref() {
                    for feature in features {
                        feature.set_source_relation(source.to_owned());
                    }
                }
            }
        }
    }

    fn model_source_relation(&self, model_id: &str) -> Option<String> {
        let mut sources = std::collections::BTreeSet::new();
        let mut visited = std::collections::BTreeSet::new();
        let mut pending = self
            .edges()
            .iter()
            .filter(|edge| edge.target() == model_id)
            .map(|edge| edge.source())
            .collect::<Vec<_>>();
        while let Some(node_id) = pending.pop() {
            if !visited.insert(node_id) {
                continue;
            }
            let node = self.nodes().iter().find(|node| node.id() == node_id)?;
            match &node.config() {
                WorkflowNodeConfig::Table { relation } => {
                    sources.insert(relation.clone());
                }
                _ => pending.extend(
                    self.edges()
                        .iter()
                        .filter(|edge| edge.target() == node_id)
                        .map(|edge| edge.source()),
                ),
            }
        }
        (sources.len() == 1)
            .then(|| sources.into_iter().next())
            .flatten()
    }

    fn first_table_id(&self) -> Option<&str> {
        self.nodes().iter().find_map(|node| {
            matches!(node.config(), WorkflowNodeConfig::Table { .. }).then_some(node.id())
        })
    }

    fn content_changed(&self, next: &Self) -> bool {
        let mut normalized = next.clone();
        normalized.set_revision(self.revision());
        self != &normalized
    }

    fn saved_node_snapshots(
        &self,
        previous: &WorkflowRunSnapshot,
        had_result: bool,
    ) -> Vec<WorkflowNodeSnapshot> {
        self.nodes()
            .iter()
            .map(|node| {
                let prior = previous
                    .nodes()
                    .iter()
                    .find(|snapshot| snapshot.node_id() == node.id());
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
                .with_quarantine(
                    prior
                        .map(|snapshot| snapshot.quarantine().clone())
                        .unwrap_or_default(),
                )
                .with_artifacts(
                    prior.and_then(|snapshot| snapshot.model().cloned()),
                    Vec::new(),
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
            .map(|node| {
                let prior = previous
                    .nodes()
                    .iter()
                    .find(|snapshot| snapshot.node_id() == node.id());
                WorkflowNodeSnapshot::new(node.id(), WorkflowNodePhase::Pending, "Pending")
                    .with_quarantine(
                        prior
                            .map(|snapshot| snapshot.quarantine().clone())
                            .unwrap_or_default(),
                    )
                    .with_artifacts(
                        prior.and_then(|snapshot| snapshot.model().cloned()),
                        Vec::new(),
                    )
            })
            .collect();
        let mut snapshot = WorkflowRunSnapshot::started(self, run_id)
            .with_nodes(nodes)
            .with_started_ms(started_ms);
        snapshot.set_result(previous.result().cloned());
        snapshot.set_result_stale(previous.result().is_some());
        snapshot
    }

    fn save_allowed(&self, validation_valid: bool) -> bool {
        validation_valid || (self.nodes().is_empty() && self.edges().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use arrow_array::{ArrayRef, Int64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use spiral_robotics_api::relations::SpiralQueryResponse;
    use spiral_robotics_api::workflow::{
        WorkflowEdgeSpec, WorkflowFeatureApplicability, WorkflowFeatureDefinition,
        WorkflowModelRunMetadata, WorkflowNodeConfig, WorkflowNodeSpec, WorkflowRunPhase,
    };

    use crate::robotics::video::VideoAsset;
    use spiral_robotics_storage::ProjectStore;
    use spiral_robotics_storage::SpiralSql;
    use spiral_robotics_storage::video::{VideoSegment, VideoSize, VideoTiming};
    use spiral_robotics_storage::{BackendStateStore, ServiceEndpoint, SpiralConnectionSettings};

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
    fn query_outputs_substitute_as_scoped_subqueries_while_tables_remain_identifiers() {
        let outputs = BTreeMap::from([
            (
                "table".to_owned(),
                ResolvedNodeOutput::Relation("robotics.frames".to_owned()),
            ),
            (
                "candidates".to_owned(),
                ResolvedNodeOutput::Query(
                    "SELECT episode_id, frame_index FROM robotics.frames".to_owned(),
                ),
            ),
        ]);
        let sql = outputs
            .substitute_in(
                "SELECT * FROM {{node:candidates}} c JOIN {{node:table}} t ON c.frame_index = t.frame_index",
            )
            .unwrap();
        assert!(sql.contains("FROM (SELECT episode_id, frame_index FROM robotics.frames) c"));
        assert!(sql.contains("JOIN robotics.frames t"));
        assert!(!sql.contains("{{node:"));
    }

    #[test]
    fn model_output_is_one_shared_server_owned_feature_cache() {
        let mut workflow = siglip_video_workflow("media.manifest", "model");
        workflow.derive_model_outputs();
        let config = workflow
            .nodes()
            .iter()
            .find(|node| node.id() == "model")
            .unwrap()
            .config();
        assert_eq!(
            config.model_output_relation(),
            Some(WorkflowFeatureApplicability::derived_cache_relation(
                "media.manifest"
            ))
        );
    }

    #[test]
    fn rerun_keeps_previous_rows_visibly_stale_until_result_replacement() {
        let workflow = WorkflowSpec::default();
        let mut previous = WorkflowRunSnapshot::idle_for(&workflow);
        previous.set_result(Some(
            SpiralQueryResponse::new(Vec::new(), vec![serde_json::Map::new()], false)
                .with_elapsed_ms(4),
        ));
        let next = workflow.starting_snapshot(&previous, "run".to_owned());
        assert_eq!(next.result(), previous.result());
        assert!(next.result_stale());
        assert_eq!(next.phase(), WorkflowRunPhase::Validating);
    }

    #[test]
    fn cancellation_after_model_publication_preserves_the_completed_model_truth() {
        let mut nodes = [
            WorkflowNodeSnapshot::new(
                "model",
                WorkflowNodePhase::Complete,
                "Backfilled 4 feature cells",
            ),
            WorkflowNodeSnapshot::new(
                "query",
                WorkflowNodePhase::Running,
                "Running read-only query",
            ),
        ];
        nodes.mark_inflight_cancelled();
        assert_eq!(nodes[0].phase(), WorkflowNodePhase::Complete);
        assert_eq!(nodes[0].detail(), "Backfilled 4 feature cells");
        assert_eq!(nodes[1].phase(), WorkflowNodePhase::Cancelled);
    }

    #[test]
    fn late_cancel_does_not_hide_a_model_publication_failure() {
        assert!(FailureCancellationPolicy::CancelWins.supersedes_failure(true));
        assert!(
            !FailureCancellationPolicy::FailureWins.supersedes_failure(true),
            "a rollback or publication failure remains Failed after a later cancellation token"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn terminal_save_failure_keeps_the_run_active_and_cancel_can_retry_finalization() {
        let project_store = Arc::new(
            ProjectStore::new(
                SpiralConnectionSettings::new(
                    "https://api.spiraldb.dev".to_owned(),
                    "https://spfs.spiraldb.dev".to_owned(),
                )
                .expect("the unconfigured test connection is valid"),
                "workflow_terminal_failure".to_owned(),
            )
            .expect("the unconfigured test project_store is valid"),
        );
        let manager = WorkflowManager::new(Arc::new(StorageManager::new(project_store)));
        let workflow = WorkflowSpec::default();
        let run_id = "terminal-save-failure".to_owned();
        let mut snapshot =
            workflow.starting_snapshot(&WorkflowRunSnapshot::idle_for(&workflow), run_id.clone());
        snapshot.set_phase(WorkflowRunPhase::Running);
        *manager.inner.document.recover_lock() = WorkflowDocument::new(workflow, snapshot);
        *manager.inner.active.recover_lock() = Some(ActiveRun {
            run_id: run_id.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            publication_gate: Arc::new(AsyncMutex::new(())),
            task: None,
            terminal_persistence_failed: false,
        });
        let (_, mut updates) = manager.subscription();
        manager
            .inner
            .persistence
            .test_outcomes
            .recover_lock()
            .push_back(Err("injected terminal write failure".to_owned()));

        manager.finish_completed(&run_id, None).await;

        let state = manager.state();
        assert_eq!(state.snapshot().phase(), WorkflowRunPhase::Running);
        assert!(state.snapshot().error().is_some_and(|error| {
            error.contains("Could not durably finalize") && error.contains("choose Cancel")
        }));
        assert!(manager.inner.active.lock().unwrap().is_some());
        let published = updates
            .try_recv()
            .expect("the recovery state must be published");
        assert_eq!(published.snapshot().phase(), WorkflowRunPhase::Running);

        manager
            .inner
            .persistence
            .test_outcomes
            .recover_lock()
            .extend([Ok(()), Ok(())]);
        let recovered = manager
            .cancel(&manager.scope_id())
            .await
            .expect("Cancel must retry the durable terminal transition");
        assert_eq!(recovered.phase(), WorkflowRunPhase::Cancelled);
        assert!(recovered.error().is_none());
        assert!(manager.inner.active.lock().unwrap().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authoritative_project_reload_gets_a_new_scope_for_a_lower_revision() {
        let project_store = Arc::new(
            ProjectStore::new(
                SpiralConnectionSettings::new(
                    "https://api.spiraldb.dev".to_owned(),
                    "https://spfs.spiraldb.dev".to_owned(),
                )
                .expect("the unconfigured test connection is valid"),
                "workflow_scope_reload".to_owned(),
            )
            .expect("the unconfigured test project_store is valid"),
        );
        let manager = WorkflowManager::new(Arc::new(StorageManager::new(project_store)));
        let mut project_a = WorkflowDocument::default();
        project_a.workflow_mut().set_revision(9);
        project_a.snapshot_mut().set_workflow_revision(9);
        manager
            .install_hydrated_document(project_a)
            .await
            .expect("the first project installs");
        let scope_a = manager.state().scope_id().to_owned();

        let mut project_b = WorkflowDocument::default();
        project_b.workflow_mut().set_revision(1);
        project_b.snapshot_mut().set_workflow_revision(1);
        manager
            .install_hydrated_document(project_b)
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

    #[test]
    fn query_only_graph_edits_preserve_model_fingerprint_for_safe_reuse() {
        let mut workflow = siglip_video_workflow("media.manifest", "model");
        workflow.derive_model_outputs();
        let mut previous = WorkflowRunSnapshot::idle_for(&workflow);
        let metadata = WorkflowModelRunMetadata::new("siglip-base", "revision")
            .with_cache_relation(WorkflowFeatureApplicability::derived_cache_relation(
                "media.manifest",
            ))
            .with_features(Vec::new())
            .with_device("Metal")
            .with_fingerprint("fingerprint")
            .with_row_counts(2, 2)
            .with_timings(1, 1);
        previous
            .nodes_mut()
            .iter_mut()
            .find(|node| node.node_id() == "model")
            .unwrap()
            .set_model(Some(metadata.clone()));
        let WorkflowNodeConfig::Query { sql, .. } = workflow
            .nodes_mut()
            .iter_mut()
            .find(|node| node.id() == "query")
            .unwrap()
            .config_mut()
        else {
            unreachable!()
        };
        sql.push_str(" LIMIT 1");

        let saved = workflow.saved_node_snapshots(&previous, true);
        assert_eq!(
            saved
                .iter()
                .find(|node| node.node_id() == "model")
                .and_then(WorkflowNodeSnapshot::model),
            Some(&metadata)
        );
    }

    #[test]
    fn model_cache_identity_has_no_caller_controlled_field() {
        let current = siglip_video_workflow("media.manifest", "model");
        let config = current
            .nodes()
            .iter()
            .find(|node| node.id() == "model")
            .unwrap()
            .config();
        assert_eq!(
            config.model_output_relation(),
            Some(WorkflowFeatureApplicability::derived_cache_relation(
                "media.manifest"
            ))
        );
    }

    #[test]
    fn model_cache_and_feature_source_are_server_derived() {
        let mut workflow = siglip_video_workflow("pick_place.camera_frames", "score-node");
        let WorkflowNodeConfig::Model { features, .. } = workflow
            .nodes_mut()
            .iter_mut()
            .find(|node| node.id() == "score-node")
            .unwrap()
            .config_mut()
        else {
            unreachable!()
        };
        features[0].set_source_relation("provider.chosen");
        workflow.derive_model_outputs();
        let config = workflow
            .nodes()
            .iter()
            .find(|node| node.id() == "score-node")
            .unwrap()
            .config();
        let WorkflowNodeConfig::Model { features } = config else {
            unreachable!()
        };
        assert_eq!(
            config.model_output_relation(),
            Some(WorkflowFeatureApplicability::derived_cache_relation(
                "pick_place.camera_frames"
            ))
        );
        assert_eq!(
            features[0].applicability().source_relation(),
            "pick_place.camera_frames"
        );
    }

    /// Run against a disposable Spiral project with:
    ///
    /// ```text
    /// SPIRAL_PROJECT_ID=project-id cargo test -p spiral-robotics-backend \
    ///   backend::workflow::manager::tests::spiral_sql_workflow_runs_persists_and_cancels \
    ///   -- --ignored --exact
    /// ```
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a real SpiralDB server and disposable project access"]
    async fn spiral_sql_workflow_runs_persists_and_cancels() {
        let stored = BackendStateStore::open_default().ok();
        let project = std::env::var("SPIRAL_PROJECT_ID")
            .ok()
            .filter(|project| !project.trim().is_empty())
            .or_else(|| stored.as_ref().and_then(BackendStateStore::project_id))
            .expect("SPIRAL_PROJECT_ID or a project saved in Studio Settings is required");
        let namespace = format!(
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
            ProjectStore::new(connection, namespace.clone())
                .expect("the Workflow E2E project store must be valid"),
        );
        let seed_relation = format!("{namespace}.seed_values");
        let mut cleanup = WorkflowTestNamespace::new(
            project_store.clone(),
            vec![seed_relation.clone(), format!("{namespace}.workflow_runs")],
            namespace.clone(),
            false,
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
        let dataset = namespace.clone();
        let mut connection = project_store
            .connect_exclusive()
            .await
            .expect("the Workflow seed project must connect");
        connection
            .ensure_dataset_namespace(&dataset)
            .await
            .expect("the Workflow seed namespace must exist");
        connection
            .ingest_batch(&target, batch)
            .await
            .expect("seeding the Workflow relation must succeed");
        drop(connection);

        let storage = Arc::new(StorageManager::new(project_store.clone()));
        let manager = WorkflowManager::new(storage.clone());
        manager
            .hydrate_from_storage()
            .await
            .expect("an empty Workflow document must hydrate");
        let mut workflow = WorkflowSpec::default();
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

        let reloaded = WorkflowManager::new(storage);
        reloaded
            .hydrate_from_storage()
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
        reloaded.shutdown().await;
        manager.shutdown().await;
        cleanup
            .cleanup_now()
            .await
            .expect("the SQL E2E namespace must clean up");
        cleanup
            .assert_absent()
            .await
            .expect("the SQL E2E must leave no catalog resources behind");
    }

    /// Live Stage-5 acceptance: a tiny deterministic MP4 is stored as a Spiral
    /// media relation, decoded by candidate SQL, backfilled into the shared
    /// feature cache, queried downstream, reused, reloaded, and cancelled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires FFmpeg, cached/downloadable SigLIP artifacts, and disposable real Spiral project access"]
    async fn spiral_siglip_video_workflow_decodes_scores_reuses_and_reloads() {
        let stored = BackendStateStore::open_default().ok();
        let project = std::env::var("SPIRAL_PROJECT_ID")
            .ok()
            .filter(|project| !project.trim().is_empty())
            .or_else(|| stored.as_ref().and_then(BackendStateStore::project_id))
            .expect("SPIRAL_PROJECT_ID or a project saved in Studio Settings is required");
        let namespace = format!(
            "workflow_siglip_{}_{}",
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
        .expect("the Workflow SigLIP connection must be valid")
        .with_credentials(stored.as_ref().and_then(BackendStateStore::credentials));
        let project_store = Arc::new(
            ProjectStore::new(connection, namespace.clone())
                .expect("the Workflow SigLIP project store must be valid"),
        );
        let manifest_relation = format!("{namespace}.video_assets");
        let media_relation = format!("{namespace}.video.assets");
        let empty_relation = format!("{namespace}.empty_video_assets");
        let model_node_id = format!(
            "model_{}_{}",
            std::process::id(),
            SystemTime::now().unix_millis()
        );
        let mut workflow = siglip_video_workflow(&manifest_relation, &model_node_id);
        set_video_candidate_sql(&mut workflow, &media_relation);
        workflow.derive_model_outputs();
        let output_relation = workflow
            .nodes()
            .iter()
            .find_map(|node| node.config().model_output_relation())
            .expect("the server derivation must produce a Model output relation");
        let empty_output_relation =
            spiral_robotics_api::workflow::WorkflowFeatureApplicability::derived_cache_relation(
                &empty_relation,
            );
        // Install cleanup before the first catalog mutation so a seeding or
        // Dataset-build failure cannot strand a test namespace.
        let mut cleanup = WorkflowTestNamespace::new(
            project_store.clone(),
            vec![
                manifest_relation.clone(),
                empty_relation.clone(),
                format!("{namespace}.workflow_runs"),
                output_relation.clone(),
                empty_output_relation,
                "workflow.feature_registry".to_owned(),
            ],
            namespace.clone(),
            true,
        );
        let asset = VideoAsset::new(
            "camera.main".to_owned(),
            "camera-e2e".to_owned(),
            tiny_test_mp4(),
        )
        .with_size(VideoSize::new(32, 32))
        .with_timing(VideoTiming::new(2, 2))
        .with_segment(VideoSegment::new(0, Some(0.0)));
        let media_batch = VideoAsset::publication_batch("episode-e2e", &[asset], &media_relation)
            .expect("the video fixture batch must be valid");
        let empty_batch = VideoAsset::publication_batch("episode-empty", &[], &empty_relation)
            .expect("the empty video fixture schema must be valid");
        let media_target = manifest_relation.clone();
        let empty_target = empty_relation.clone();
        let dataset = namespace.clone();
        let mut connection = project_store
            .connect_exclusive()
            .await
            .expect("the video Workflow project must connect");
        connection
            .ensure_dataset_namespace(&dataset)
            .await
            .expect("the video Workflow namespace must exist");
        connection
            .ingest_batch(&media_target, media_batch)
            .await
            .expect("the video manifest must save");
        connection
            .replace_batches(&empty_target, vec![empty_batch])
            .await
            .expect("the empty video manifest must save");
        connection
            .rebuild_video_dataset(&dataset, false)
            .await
            .expect("seeding the video manifest relations must succeed");
        drop(connection);

        let storage = Arc::new(StorageManager::new(project_store.clone()));
        let manager = WorkflowManager::new(storage.clone());
        manager
            .hydrate_from_storage()
            .await
            .expect("an empty Workflow document must hydrate");
        let saved = manager
            .command(WorkflowCommand::apply(manager.scope_id(), 0, workflow))
            .await
            .expect("the SigLIP video workflow must save");

        let scope_id = manager.scope_id().to_owned();
        let revision = saved.workflow().revision();
        manager
            .start(&scope_id, revision)
            .await
            .expect("the SigLIP video run must start");
        let completed = wait_for_model_terminal(&manager).await;
        assert_eq!(
            completed.phase(),
            WorkflowRunPhase::Completed,
            "Workflow failed: {:?}",
            completed.error()
        );
        let result = completed.result().expect("Result must own real rows");
        assert_eq!(result.rows().len(), 2);
        for row in result.rows() {
            for column in ["is_black_square_feature", "is_white_square_feature"] {
                let score = row
                    .get(column)
                    .and_then(serde_json::Value::as_f64)
                    .expect("SigLIP score columns must be numeric");
                assert!((0.0..=1.0).contains(&score));
            }
        }
        let metadata = completed
            .nodes()
            .iter()
            .find(|node| node.node_id() == model_node_id)
            .and_then(WorkflowNodeSnapshot::model)
            .expect("the Model snapshot must expose measured metadata");
        assert_eq!(metadata.input_rows(), 2);
        assert_eq!(metadata.output_rows(), 2);
        assert!(
            metadata.decode_ms() > 0,
            "Spiral decode timing must be measured"
        );
        assert!(
            metadata.inference_ms() > 0,
            "SigLIP timing must be measured"
        );
        eprintln!(
            "workflow SigLIP real smoke: device={}, rows=2, decode_ms={}, inference_ms={}, decode_fps={:.2}",
            metadata.device(),
            metadata.decode_ms(),
            metadata.inference_ms(),
            2_000.0 / metadata.decode_ms() as f64,
        );
        manager
            .start(&scope_id, revision)
            .await
            .expect("an unchanged run must start for reuse");
        let reused = wait_for_model_terminal(&manager).await;
        assert!(
            reused
                .nodes()
                .iter()
                .find(|node| node.node_id() == model_node_id)
                .and_then(WorkflowNodeSnapshot::model)
                .is_some_and(WorkflowModelRunMetadata::reused),
            "the unchanged fingerprint must reuse the durable relation"
        );

        let reloaded = WorkflowManager::new(storage);
        reloaded
            .hydrate_from_storage()
            .await
            .expect("the SigLIP workflow and snapshot must reload");
        assert_eq!(reloaded.state().snapshot().result(), reused.result());
        assert!(
            reloaded
                .state()
                .snapshot()
                .nodes()
                .iter()
                .find(|node| node.node_id() == model_node_id)
                .and_then(WorkflowNodeSnapshot::model)
                .is_some_and(WorkflowModelRunMetadata::reused)
        );

        let mut empty_workflow = siglip_video_workflow(&empty_relation, &model_node_id);
        set_video_candidate_sql(&mut empty_workflow, &media_relation);
        let empty_saved = reloaded
            .command(WorkflowCommand::apply(
                reloaded.scope_id(),
                saved.workflow().revision(),
                empty_workflow,
            ))
            .await
            .expect("the empty manifest workflow must save");
        reloaded
            .start(&reloaded.scope_id(), empty_saved.workflow().revision())
            .await
            .expect("the zero-row replacement run must start");
        let empty = wait_for_model_terminal(&reloaded).await;
        assert_eq!(empty.phase(), WorkflowRunPhase::Completed);
        assert_eq!(empty.result().map(|result| result.rows().len()), Some(0));
        let empty_metadata = empty
            .nodes()
            .iter()
            .find(|node| node.node_id() == model_node_id)
            .and_then(WorkflowNodeSnapshot::model)
            .expect("the zero-row run must retain a typed Model snapshot");
        assert_eq!(empty_metadata.output_rows(), 0);
        assert_eq!(
            empty_metadata.device(),
            "not loaded (empty applicable cohort)"
        );

        let mut restored_workflow = siglip_video_workflow(&manifest_relation, &model_node_id);
        set_video_candidate_sql(&mut restored_workflow, &media_relation);
        let changed_saved = reloaded
            .command(WorkflowCommand::apply(
                reloaded.scope_id(),
                empty_saved.workflow().revision(),
                restored_workflow,
            ))
            .await
            .expect("restoring the media workflow must save");
        reloaded
            .start(&reloaded.scope_id(), changed_saved.workflow().revision())
            .await
            .expect("the cancellable model run must start");
        let cancelling = reloaded
            .cancel(&reloaded.scope_id())
            .await
            .expect("the active Model run must cancel");
        assert_eq!(cancelling.phase(), WorkflowRunPhase::Cancelling);
        assert_eq!(
            wait_for_model_terminal(&reloaded).await.phase(),
            WorkflowRunPhase::Cancelled
        );

        // Removing a Model must not delete the source-scoped cache: a later
        // workflow over the same source owns the same registered features.
        let mut without_model = reloaded.state().workflow().clone();
        without_model
            .nodes_mut()
            .retain(|node| node.id() != model_node_id);
        without_model
            .edges_mut()
            .retain(|edge| edge.source() != model_node_id && edge.target() != model_node_id);
        without_model
            .edges_mut()
            .insert(0, WorkflowEdgeSpec::new("table", "query"));
        let WorkflowNodeConfig::Query { sql, .. } = without_model
            .nodes_mut()
            .iter_mut()
            .find(|node| node.id() == "query")
            .expect("the test workflow retains its Query")
            .config_mut()
        else {
            unreachable!()
        };
        *sql = "SELECT * FROM {{node:table}} ORDER BY episode_id".to_owned();
        let removed = reloaded
            .command(WorkflowCommand::apply(
                reloaded.scope_id(),
                without_model.revision(),
                without_model,
            ))
            .await
            .expect("removing the Model must save without deleting shared cache state");
        let manifest_check = manifest_relation.clone();
        let output_check = output_relation.clone();
        let mut connection = project_store
            .connect()
            .await
            .expect("the post-Model-removal project must connect");
        assert!(
            connection
                .table_exists(&manifest_check)
                .await
                .expect("the input manifest catalog lookup must succeed"),
            "hostile stored output text dropped the input manifest"
        );
        assert!(
            connection
                .table_exists(&output_check)
                .await
                .expect("the feature cache catalog lookup must succeed"),
            "removing one Model deleted the shared feature cache"
        );
        drop(connection);
        let mut renamed = removed.workflow().clone();
        renamed.set_name("Post-cleanup write");
        let renamed = reloaded
            .command(WorkflowCommand::apply(
                reloaded.scope_id(),
                renamed.revision(),
                renamed,
            ))
            .await
            .expect("ordinary persistence must work after cleanup restores autocommit");
        reloaded
            .reload_from_storage()
            .await
            .expect("the post-cleanup document must remain readable");
        assert_eq!(
            reloaded.state().workflow().name(),
            renamed.workflow().name()
        );
        reloaded.shutdown().await;
        manager.shutdown().await;
        cleanup
            .cleanup_now()
            .await
            .expect("the SigLIP E2E namespace must clean up");
        cleanup
            .assert_absent()
            .await
            .expect("the SigLIP E2E must leave no catalog resources behind");
    }

    fn siglip_video_workflow(relation: &str, model_node_id: &str) -> WorkflowSpec {
        let mut workflow = WorkflowSpec::new("default", "SigLIP video E2E");
        workflow.nodes_mut().extend([
                WorkflowNodeSpec::new(
                    "table",
                    "Video manifest",
                    WorkflowNodeConfig::Table {
                        relation: relation.to_owned(),
                    },
                ),
                WorkflowNodeSpec::new(
                    "candidates",
                    "Decode candidate frames",
                    WorkflowNodeConfig::Query {
                        sql: "SELECT episode_id, stream_id, segment_index, frame_index, episode_time_ns, image FROM {{node:table}}".to_owned(),
                        max_rows: 100,
                    },
                ),
                WorkflowNodeSpec::new(
                    model_node_id,
                    "Backfill visual features",
                    WorkflowNodeConfig::Model {
                        features: vec![
                            feature_definition(
                                relation,
                                "is_black_square_feature",
                                "black square",
                            ),
                            feature_definition(
                                relation,
                                "is_white_square_feature",
                                "white square",
                            ),
                        ],
                    },
                ),
                WorkflowNodeSpec::new(
                    "query",
                    "Inspect scores",
                    WorkflowNodeConfig::Query {
                        sql: format!(
                            "SELECT candidates.*, features.is_black_square_feature, features.is_white_square_feature FROM {{{{node:candidates}}}} candidates JOIN {{{{node:{model_node_id}}}}} features ON candidates.episode_id = features.episode_id AND candidates.stream_id = features.stream_id AND candidates.segment_index = features.segment_index AND candidates.frame_index = features.frame_index AND candidates.episode_time_ns = features.episode_time_ns ORDER BY candidates.frame_index"
                        ),
                        max_rows: 100,
                    },
                ),
                WorkflowNodeSpec::new("result", "Scored frames", WorkflowNodeConfig::Result),
            ]);
        workflow.edges_mut().extend([
            WorkflowEdgeSpec::new("table", "candidates"),
            WorkflowEdgeSpec::new("candidates", model_node_id),
            WorkflowEdgeSpec::new("candidates", "query"),
            WorkflowEdgeSpec::new(model_node_id, "query"),
            WorkflowEdgeSpec::new("query", "result"),
        ]);
        workflow
    }

    fn set_video_candidate_sql(workflow: &mut WorkflowSpec, media_relation: &str) {
        let WorkflowNodeConfig::Query { sql, .. } = workflow
            .nodes_mut()
            .iter_mut()
            .find(|node| node.id() == "candidates")
            .expect("the test workflow has a candidate Query")
            .config_mut()
        else {
            unreachable!()
        };
        *sql = format!(
            "SELECT m.episode_id, m.stream_id, m.segment_index, CAST(f.request_index AS UINT64) AS frame_index, COALESCE(m.timeline_offset_ns, CAST(0 AS UINT64)) + (CAST(f.request_index AS UINT64) * CAST(1000000000 AS UINT64) / CAST(m.fps AS UINT64)) AS episode_time_ns, spql.video.decode(f.frame, 'auto') AS image FROM {{{{node:table}}}} m JOIN {media_relation} a ON a.episode_id = m.episode_id AND a.stream_id = m.stream_id AND a.segment_index = m.segment_index CROSS JOIN LATERAL spql.video.frames(a.media, spql.video.frame_slice(CAST(0 AS BIGINT), CAST(m.frames AS BIGINT), CAST(1 AS BIGINT))) f"
        );
    }

    fn feature_definition(
        source_relation: &str,
        name: &str,
        concept: &str,
    ) -> WorkflowFeatureDefinition {
        WorkflowFeatureDefinition::new(
            name,
            format!("Probability that the frame shows {concept}"),
            concept,
        )
        .with_applicability(WorkflowFeatureApplicability::new(source_relation, "image"))
    }

    fn tiny_test_mp4() -> Vec<u8> {
        let ffmpeg = std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned());
        let output = std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=32x32:r=2:d=1",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "frag_keyframe+empty_moov+default_base_moof",
                "-f",
                "mp4",
                "pipe:1",
            ])
            .output()
            .unwrap_or_else(|error| panic!("could not run {ffmpeg:?}: {error}"));
        assert!(
            output.status.success(),
            "FFmpeg fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty(), "FFmpeg fixture emitted no MP4");
        output.stdout
    }

    async fn wait_for_model_terminal(manager: &WorkflowManager) -> WorkflowRunSnapshot {
        for _ in 0..2_400 {
            let snapshot = manager.state().snapshot().clone();
            if snapshot.phase().terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("Workflow Model run did not reach a terminal state within 60 seconds");
    }

    async fn wait_for_terminal(manager: &WorkflowManager) -> WorkflowRunSnapshot {
        for _ in 0..400 {
            let snapshot = manager.state().snapshot().clone();
            if snapshot.phase().terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("Workflow run did not reach a terminal state within 10 seconds");
    }

    struct WorkflowTestNamespace {
        project_store: Arc<ProjectStore>,
        relations: Vec<String>,
        namespace: String,
        video_dataset: bool,
        cleaned: bool,
    }

    impl WorkflowTestNamespace {
        fn new(
            project_store: Arc<ProjectStore>,
            relations: Vec<String>,
            namespace: String,
            video_dataset: bool,
        ) -> Self {
            Self {
                project_store,
                relations,
                namespace,
                video_dataset,
                cleaned: false,
            }
        }

        async fn cleanup_now(&mut self) -> Result<(), String> {
            if self.cleaned {
                return Ok(());
            }
            if self.video_dataset {
                let dataset = format!("{}.video", self.namespace);
                self.execute_cleanup_sql(&SpiralSql::drop_video_dataset(&dataset))
                    .await?;
                self.execute_cleanup_sql(&format!(
                    "DROP VIEW IF EXISTS {}_video_sources",
                    self.namespace
                ))
                .await?;
            }
            for relation in self.relations.clone() {
                self.drop_cleanup_table(&relation).await?;
            }
            self.drop_cleanup_table(&self.namespace).await?;
            self.cleaned = true;
            Ok(())
        }

        async fn execute_cleanup_sql(&self, sql: &str) -> Result<(), String> {
            const MAX_ATTEMPTS: usize = 32;
            for attempt in 1..=MAX_ATTEMPTS {
                let mut connection = self.project_store.connect_exclusive().await?;
                let result = connection.execute_update_sql(sql).await;
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

        async fn drop_cleanup_table(&self, relation: &str) -> Result<(), String> {
            const MAX_ATTEMPTS: usize = 32;
            for attempt in 1..=MAX_ATTEMPTS {
                let mut connection = self.project_store.connect_exclusive().await?;
                let result: Result<(), String> = async {
                    if connection.table_exists(relation).await? {
                        connection
                            .execute_update_sql(&format!("DROP TABLE {relation}"))
                            .await?;
                    }
                    Ok(())
                }
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
            let namespace = self.namespace.clone();
            let relations = self.relations.clone();
            let mut connection = self.project_store.connect_exclusive().await?;
            let leaked = connection
                .catalog_relation_names()
                .await?
                .into_iter()
                .filter(|relation_name| {
                    relation_name == &namespace
                        || relation_name.starts_with(&format!("{namespace}."))
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
