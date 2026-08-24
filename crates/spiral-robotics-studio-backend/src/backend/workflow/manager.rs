//! Persisted Table, Model, Query, and Result execution over Spiral relations.

use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use spiral_robotics_contracts::api::SpiralQueryResponse;
use spiral_robotics_contracts::workflow::{
    CancelWorkflowRequest, RunWorkflowRequest, SaveWorkflowRequest, WorkflowCapabilities,
    WorkflowDocument, WorkflowNodeConfig, WorkflowNodePhase, WorkflowNodeSnapshot,
    WorkflowQuarantineSummary, WorkflowRunPhase, WorkflowRunSnapshot, WorkflowSpec,
    WorkflowStateResponse, WorkflowUpdate, derived_feature_cache_relation, topological_order,
    validate_workflow_spec,
};
use spiral_robotics_core::workflow::{scan_node_placeholders, validate_qualified_relation};
use spiral_robotics_studio_application::error::ApplicationError;
use spiral_robotics_studio_application::workflow::{admit_cancel, admit_run, admit_save};
use tokio::sync::broadcast;

use crate::backend::storage::manager::StorageManager;
use crate::backend::workflow::model::{
    ModelRunEvent, ModelRunFailure, ModelRunRequest, ModelRunResult, run_model,
};

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SCOPE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct WorkflowManager {
    inner: Arc<WorkflowManagerInner>,
}

struct WorkflowManagerInner {
    storage: Arc<StorageManager>,
    scope_id: Mutex<String>,
    document: Mutex<WorkflowDocument>,
    updates: broadcast::Sender<WorkflowUpdate>,
    active: Mutex<Option<ActiveRun>>,
    terminal_persistence_error: Mutex<Option<TerminalPersistenceError>>,
    #[cfg(test)]
    persistence_outcomes: Mutex<VecDeque<Result<(), String>>>,
}

struct ActiveRun {
    run_id: String,
    cancel: Arc<AtomicBool>,
    publication_gate: Arc<Mutex<()>>,
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

impl WorkflowManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        let (updates, _) = broadcast::channel(128);
        Self {
            inner: Arc::new(WorkflowManagerInner {
                storage,
                scope_id: Mutex::new(next_scope_id()),
                document: Mutex::new(WorkflowDocument::default()),
                updates,
                active: Mutex::new(None),
                terminal_persistence_error: Mutex::new(None),
                #[cfg(test)]
                persistence_outcomes: Mutex::new(VecDeque::new()),
            }),
        }
    }

    /// Restore the saved graph and latest snapshot. Local SQL execution is not
    /// resumable, so an in-flight snapshot becomes explicitly interrupted.
    pub(crate) async fn hydrate_from_storage(&self) -> Result<(), String> {
        let storage = self.inner.storage.clone();
        let document = tokio::task::spawn_blocking(move || storage.load_workflow_document())
            .await
            .map_err(|error| format!("Workflow history task stopped: {error}"))??;
        self.install_hydrated_document(document)
    }

    pub(crate) fn reload_from_storage(&self) -> Result<(), String> {
        let document = self.inner.storage.load_workflow_document()?;
        self.install_hydrated_document(document)
    }

    fn install_hydrated_document(&self, mut document: WorkflowDocument) -> Result<(), String> {
        if document.snapshot.phase.active() {
            document.snapshot.phase = WorkflowRunPhase::Interrupted;
            document.snapshot.error = Some(
                "Studio restarted before this Workflow run finished. Run it again.".to_owned(),
            );
            document.snapshot.finished_ms = now_millis();
            document.snapshot.updated_ms = document.snapshot.finished_ms;
            for node in &mut document.snapshot.nodes {
                if matches!(
                    node.phase,
                    WorkflowNodePhase::Validating
                        | WorkflowNodePhase::Loading
                        | WorkflowNodePhase::Running
                        | WorkflowNodePhase::Cancelling
                ) {
                    node.phase = WorkflowNodePhase::Failed;
                    node.detail = "Interrupted by Studio restart".to_owned();
                    node.error = document.snapshot.error.clone();
                }
            }
            self.inner.storage.save_workflow_document(&document)?;
        }
        *lock(&self.inner.document) = document;
        *lock(&self.inner.scope_id) = next_scope_id();
        *lock(&self.inner.terminal_persistence_error) = None;
        let update = WorkflowUpdate {
            state: self.state(),
        };
        let _ = self.inner.updates.send(update);
        Ok(())
    }

    pub(crate) fn state(&self) -> WorkflowStateResponse {
        let document = lock(&self.inner.document);
        self.state_response(&document)
    }

    pub(crate) fn scope_id(&self) -> String {
        lock(&self.inner.scope_id).clone()
    }

    pub(crate) async fn relations(
        &self,
    ) -> Result<spiral_robotics_contracts::workflow::WorkflowRelationsResponse, String> {
        let scope_id = self.scope_id();
        let mut response = self.inner.storage.workflow_relations().await?;
        if self.scope_id() != scope_id {
            return Err(
                "The Spiral project changed while Workflow relations were loading. Reload them."
                    .to_owned(),
            );
        }
        response.scope_id = scope_id;
        Ok(response)
    }

    pub(crate) fn is_active(&self) -> bool {
        lock(&self.inner.document).snapshot.phase.active()
    }

    pub(crate) fn capabilities(&self) -> WorkflowCapabilities {
        WorkflowCapabilities::default()
    }

    /// Resolve server-owned Model outputs and validate an agent-authored graph
    /// without mutating the saved Workflow.
    pub(crate) fn prepare_candidate(
        &self,
        mut workflow: WorkflowSpec,
    ) -> Result<
        (
            WorkflowSpec,
            spiral_robotics_contracts::workflow::WorkflowValidationReport,
        ),
        ApplicationError,
    > {
        let current = lock(&self.inner.document);
        if workflow.id != current.workflow.id {
            return Err(ApplicationError::InvalidInput(format!(
                "Workflow identity {:?} does not match the server-owned identity {:?}",
                workflow.id, current.workflow.id
            )));
        }
        workflow.revision = current.workflow.revision;
        derive_model_outputs(&mut workflow);
        let report = validate_workflow_spec(&workflow);
        Ok((workflow, report))
    }

    pub(crate) fn subscription(
        &self,
    ) -> (WorkflowStateResponse, broadcast::Receiver<WorkflowUpdate>) {
        let document = lock(&self.inner.document);
        let updates = self.inner.updates.subscribe();
        (self.state_response(&document), updates)
    }

    pub(crate) fn save(
        &self,
        request: SaveWorkflowRequest,
    ) -> Result<WorkflowStateResponse, ApplicationError> {
        self.require_scope(&request.scope_id)?;
        let mut current = lock(&self.inner.document);
        if request.workflow.id != current.workflow.id {
            return Err(ApplicationError::InvalidInput(format!(
                "Workflow identity {:?} does not match the server-owned identity {:?}",
                request.workflow.id, current.workflow.id
            )));
        }
        let next_revision = admit_save(
            request.expected_revision,
            current.workflow.revision,
            current.snapshot.phase.active(),
        )?;
        let mut workflow = request.workflow;
        workflow.revision = next_revision;
        derive_model_outputs(&mut workflow);
        let validation = validate_workflow_spec(&workflow);
        let empty_draft = workflow.nodes.is_empty() && workflow.edges.is_empty();
        if !workflow_save_allowed(&workflow, validation.valid()) {
            let issue = validation
                .errors
                .first()
                .map(|issue| format!("{} {}", issue.message, issue.recovery))
                .unwrap_or_else(|| "Workflow validation failed.".to_owned());
            return Err(ApplicationError::InvalidInput(issue));
        }

        let changed = workflow_content_changed(&current.workflow, &workflow);
        let mut snapshot = current.snapshot.clone();
        snapshot.workflow_revision = workflow.revision;
        snapshot.updated_ms = now_millis();
        if changed {
            let had_result = snapshot.result.is_some() && !empty_draft;
            snapshot.phase = WorkflowRunPhase::Idle;
            snapshot.result_stale = had_result;
            snapshot.error = None;
            snapshot.nodes = saved_graph_node_snapshots(&current.snapshot, &workflow, had_result);
            if empty_draft {
                snapshot.result = None;
            }
        }
        let candidate = WorkflowDocument { workflow, snapshot };
        self.inner
            .storage
            .save_workflow_document(&candidate)
            .map_err(ApplicationError::External)?;
        *current = candidate;
        let response = self.state_response(&current);
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: response.clone(),
        });
        Ok(response)
    }

    pub(crate) fn start(
        &self,
        request: RunWorkflowRequest,
    ) -> Result<WorkflowRunSnapshot, ApplicationError> {
        self.require_scope(&request.scope_id)?;
        // When both locks are required, active-run admission always precedes
        // the document. Cancel and terminal publication use the same order.
        let mut active = lock(&self.inner.active);
        let mut document = lock(&self.inner.document);
        let validation = validate_workflow_spec(&document.workflow);
        admit_run(
            request.revision,
            document.workflow.revision,
            active.is_some() || document.snapshot.phase.active(),
            validation.valid(),
        )?;

        let run_id = next_run_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let publication_gate = Arc::new(Mutex::new(()));
        let snapshot = starting_snapshot(&document.workflow, &document.snapshot, run_id.clone());
        let candidate = WorkflowDocument {
            workflow: document.workflow.clone(),
            snapshot: snapshot.clone(),
        };
        self.inner
            .storage
            .save_workflow_document(&candidate)
            .map_err(ApplicationError::External)?;
        document.snapshot = snapshot.clone();
        *active = Some(ActiveRun {
            run_id: run_id.clone(),
            cancel: cancel.clone(),
            publication_gate: publication_gate.clone(),
            task: None,
            terminal_persistence_failed: false,
        });
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: self.state_response(&document),
        });
        drop(document);
        drop(active);

        let manager = self.clone();
        let task_run_id = run_id.clone();
        let task = tokio::spawn(async move {
            manager.execute(run_id, cancel, publication_gate).await;
        });
        if let Some(active) = lock(&self.inner.active)
            .as_mut()
            .filter(|active| active.run_id == task_run_id)
        {
            active.task = Some(task);
        }
        Ok(snapshot)
    }

    pub(crate) async fn shutdown(&self) {
        let task = {
            let mut active = lock(&self.inner.active);
            active.as_mut().and_then(|run| {
                let _publication = lock(&run.publication_gate);
                run.cancel.store(true, Ordering::SeqCst);
                run.task.take()
            })
        };
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    pub(crate) fn cancel(
        &self,
        request: CancelWorkflowRequest,
    ) -> Result<WorkflowRunSnapshot, ApplicationError> {
        self.require_scope(&request.scope_id)?;
        // Keep admission and Cancelling publication in one active -> document
        // critical section. Otherwise execution could publish Cancelled and
        // clear `active` between these steps, only for this request to regress
        // the persisted snapshot back to Cancelling.
        let active = lock(&self.inner.active);
        admit_cancel(active.is_some())?;
        let control = active.as_ref().expect("admission requires an active run");
        let recover_terminal_persistence = control.terminal_persistence_failed;
        let run_id = control.run_id.clone();
        let _publication = lock(&control.publication_gate);
        control.cancel.store(true, Ordering::SeqCst);
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(control.run_id.as_str()) {
            return Err(ApplicationError::Conflict(
                "The active Workflow run changed before cancellation.".to_owned(),
            ));
        }
        let mut candidate = document.clone();
        candidate.snapshot.phase = WorkflowRunPhase::Cancelling;
        candidate.snapshot.updated_ms = now_millis();
        for node in &mut candidate.snapshot.nodes {
            if matches!(
                node.phase,
                WorkflowNodePhase::Validating
                    | WorkflowNodePhase::Loading
                    | WorkflowNodePhase::Running
            ) {
                node.phase = WorkflowNodePhase::Cancelling;
                node.detail = "Cancelling after the current Spiral operation".to_owned();
            }
        }
        self.save_document(&candidate)
            .map_err(ApplicationError::External)?;
        *document = candidate;
        let snapshot = document.snapshot.clone();
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: self.state_response(&document),
        });
        drop(document);
        drop(_publication);
        drop(active);
        if recover_terminal_persistence {
            self.finish_cancelled(&run_id);
            return Ok(self.state().snapshot);
        }
        Ok(snapshot)
    }

    async fn execute(
        &self,
        run_id: String,
        cancel: Arc<AtomicBool>,
        publication_gate: Arc<Mutex<()>>,
    ) {
        let workflow = lock(&self.inner.document).workflow.clone();
        let order = match topological_order(&workflow) {
            Ok(order) => order,
            Err(error) => {
                self.finish_failed(&run_id, None, error);
                return;
            }
        };
        let discovered = match self.inner.storage.workflow_relations().await {
            Ok(relations) => relations,
            Err(error) => {
                self.finish_failed(
                    &run_id,
                    first_table_id(&workflow),
                    format!("Discovering Spiral relations: {error}"),
                );
                return;
            }
        };
        let discovered = discovered
            .relations
            .into_iter()
            .map(|relation| (relation.name, relation.columns.len()))
            .collect::<BTreeMap<_, _>>();
        let mut outputs = BTreeMap::new();
        let mut query_outputs: BTreeMap<String, SpiralQueryResponse> = BTreeMap::new();
        let mut terminal_result = None;

        if self.cancelled(&run_id, &cancel) {
            self.finish_cancelled(&run_id);
            return;
        }
        self.set_run_phase(&run_id, WorkflowRunPhase::Running);
        for node_id in order {
            if self.cancelled(&run_id, &cancel) {
                self.finish_cancelled(&run_id);
                return;
            }
            let Some(node) = workflow.nodes.iter().find(|node| node.id == node_id) else {
                self.finish_failed(&run_id, None, format!("Unknown node {node_id:?}"));
                return;
            };
            let node_started = now_millis();
            match &node.config {
                WorkflowNodeConfig::Table { relation } => {
                    self.set_node_phase(
                        &run_id,
                        &node.id,
                        WorkflowNodePhase::Validating,
                        "Resolving Spiral relation".to_owned(),
                        None,
                    );
                    let Some(field_count) = discovered.get(relation) else {
                        self.finish_failed(
                            &run_id,
                            Some(&node.id),
                            format!(
                                "Spiral relation {relation:?} is not in the active project catalog. Choose another relation and Apply."
                            ),
                        );
                        return;
                    };
                    outputs.insert(
                        node.id.clone(),
                        ResolvedNodeOutput::Relation(relation.clone()),
                    );
                    self.complete_node(
                        &run_id,
                        &node.id,
                        node_started,
                        format!("Resolved {field_count} fields"),
                        None,
                    );
                }
                WorkflowNodeConfig::Query { sql, max_rows } => {
                    let substituted = match substitute_outputs(sql, &outputs) {
                        Ok(sql) => sql,
                        Err(error) => {
                            self.finish_failed(&run_id, Some(&node.id), error.to_string());
                            return;
                        }
                    };
                    let feeds_result = workflow.edges.iter().any(|edge| {
                        edge.source == node.id
                            && workflow.nodes.iter().any(|target| {
                                target.id == edge.target
                                    && matches!(target.config, WorkflowNodeConfig::Result)
                            })
                    });
                    if !feeds_result {
                        outputs.insert(node.id.clone(), ResolvedNodeOutput::Query(substituted));
                        self.complete_node(
                            &run_id,
                            &node.id,
                            node_started,
                            "Resolved read-only candidate SQL".to_owned(),
                            None,
                        );
                        continue;
                    }
                    self.set_node_phase(
                        &run_id,
                        &node.id,
                        WorkflowNodePhase::Running,
                        "Running bounded read-only SQL".to_owned(),
                        None,
                    );
                    let response = match self.inner.storage.query(&substituted, *max_rows).await {
                        Ok(response) => response,
                        Err(error) => {
                            if self.cancelled(&run_id, &cancel) {
                                self.finish_cancelled(&run_id);
                            } else {
                                self.finish_failed(&run_id, Some(&node.id), error);
                            }
                            return;
                        }
                    };
                    if self.cancelled(&run_id, &cancel) {
                        self.finish_cancelled(&run_id);
                        return;
                    }
                    let rows = response.rows.len();
                    query_outputs.insert(node.id.clone(), response);
                    outputs.insert(node.id.clone(), ResolvedNodeOutput::Query(substituted));
                    self.complete_node(
                        &run_id,
                        &node.id,
                        node_started,
                        format!("Returned {rows} rows"),
                        Some(rows),
                    );
                }
                WorkflowNodeConfig::Result => {
                    self.set_node_phase(
                        &run_id,
                        &node.id,
                        WorkflowNodePhase::Running,
                        "Preparing result payload".to_owned(),
                        None,
                    );
                    let source = workflow
                        .edges
                        .iter()
                        .find(|edge| edge.target == node.id)
                        .map(|edge| edge.source.as_str());
                    let Some(result) = source.and_then(|source| query_outputs.get(source)).cloned()
                    else {
                        self.finish_failed(
                            &run_id,
                            Some(&node.id),
                            "Result has no completed Query payload.".to_owned(),
                        );
                        return;
                    };
                    let rows = result.rows.len();
                    terminal_result = Some(result);
                    self.complete_node(
                        &run_id,
                        &node.id,
                        node_started,
                        format!("Ready · {rows} rows"),
                        Some(rows),
                    );
                }
                WorkflowNodeConfig::Model {
                    features,
                    feature_cache_relation,
                    ..
                } => {
                    let source = workflow
                        .edges
                        .iter()
                        .find(|edge| edge.target == node.id)
                        .map(|edge| edge.source.as_str());
                    let Some(candidate_sql) = source
                        .and_then(|source| outputs.get(source))
                        .and_then(|output| match output {
                            ResolvedNodeOutput::Query(sql) => Some(sql.clone()),
                            ResolvedNodeOutput::Relation(_) => None,
                        })
                    else {
                        self.finish_failed(
                            &run_id,
                            Some(&node.id),
                            "Model has no resolved upstream candidate Query.".to_owned(),
                        );
                        return;
                    };
                    let Some(output_relation) = feature_cache_relation.clone() else {
                        self.finish_failed(
                            &run_id,
                            Some(&node.id),
                            "Model output relation was not derived by the server. Apply the workflow again."
                                .to_owned(),
                        );
                        return;
                    };
                    let request = ModelRunRequest {
                        candidate_sql,
                        feature_cache_relation: output_relation.clone(),
                        features: features.clone(),
                    };
                    let repository = self.inner.storage.workflow_model_repository();
                    let event_manager = self.clone();
                    let event_run_id = run_id.clone();
                    let event_node_id = node.id.clone();
                    let worker_cancel = cancel.clone();
                    let worker_publication_gate = publication_gate.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        run_model(
                            repository,
                            request,
                            worker_cancel,
                            worker_publication_gate,
                            |event| {
                                event_manager.set_model_event(&event_run_id, &event_node_id, event);
                            },
                        )
                    })
                    .await;
                    let result = match result {
                        Ok(Ok(result)) => result,
                        Ok(Err(ModelRunFailure::Cancelled)) => {
                            self.finish_cancelled(&run_id);
                            return;
                        }
                        Ok(Err(ModelRunFailure::Failed(error))) => {
                            self.finish_model_failed(&run_id, &node.id, error);
                            return;
                        }
                        Err(error) => {
                            self.finish_failed(
                                &run_id,
                                Some(&node.id),
                                format!("Workflow Model worker stopped: {error}"),
                            );
                            return;
                        }
                    };
                    outputs.insert(
                        node.id.clone(),
                        ResolvedNodeOutput::Relation(output_relation),
                    );
                    self.complete_model_node(&run_id, &node.id, node_started, result);
                    if self.cancelled(&run_id, &cancel) {
                        // A successful worker result means the cache+registry
                        // transaction committed before cancellation acquired
                        // the publication gate. Preserve that Model truth, then
                        // stop before the next node.
                        self.finish_cancelled(&run_id);
                        return;
                    }
                }
            }
        }
        self.finish_completed(&run_id, terminal_result);
    }

    fn set_model_event(&self, run_id: &str, node_id: &str, event: ModelRunEvent) {
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id)
            || matches!(
                document.snapshot.phase,
                WorkflowRunPhase::Cancelling | WorkflowRunPhase::Cancelled
            )
        {
            return;
        }
        let Some(node) = document
            .snapshot
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
        else {
            return;
        };
        if node.phase == WorkflowNodePhase::Cancelling {
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
        let phase_changed = node.phase != phase;
        node.phase = phase;
        node.detail = detail;
        node.progress = progress;
        node.error = None;
        if materialization_invalidated {
            node.model = None;
            node.quarantine = WorkflowQuarantineSummary::default();
            node.warnings.clear();
        }
        document.snapshot.updated_ms = now_millis();
        if phase_changed || materialization_invalidated {
            self.persist_publish(&document);
        } else {
            let _ = self.inner.updates.send(WorkflowUpdate {
                state: self.state_response(&document),
            });
        }
    }

    fn complete_model_node(
        &self,
        run_id: &str,
        node_id: &str,
        started_ms: u64,
        result: ModelRunResult,
    ) {
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id) {
            return;
        }
        if let Some(node) = document
            .snapshot
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
        {
            let has_warnings = !result.warnings.is_empty() || result.quarantine.count > 0;
            node.phase = if has_warnings {
                WorkflowNodePhase::CompleteWithWarnings
            } else {
                WorkflowNodePhase::Complete
            };
            node.detail = if result.metadata.reused {
                format!(
                    "Feature cache hit · {} cells reused",
                    result.reused_feature_cells
                )
            } else if has_warnings {
                format!(
                    "Backfilled {} feature cells with warnings · {} cache rows",
                    result.computed_feature_cells, result.rows
                )
            } else {
                format!(
                    "Backfilled {} feature cells · {} reused · {} cache rows",
                    result.computed_feature_cells, result.reused_feature_cells, result.rows
                )
            };
            node.elapsed_ms = now_millis().saturating_sub(started_ms);
            node.rows = Some(result.rows);
            node.error = None;
            node.progress = Some(spiral_robotics_contracts::workflow::WorkflowNodeProgress {
                completed: result
                    .computed_feature_cells
                    .saturating_add(result.reused_feature_cells),
                total: result.requested_feature_cells,
                unit: "feature cells".to_owned(),
            });
            node.quarantine = result.quarantine;
            node.model = Some(result.metadata);
            node.warnings = result.warnings;
        }
        document.snapshot.updated_ms = now_millis();
        self.persist_publish(&document);
    }

    fn set_run_phase(&self, run_id: &str, phase: WorkflowRunPhase) {
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id) {
            return;
        }
        document.snapshot.phase = phase;
        document.snapshot.updated_ms = now_millis();
        self.persist_publish(&document);
    }

    fn set_node_phase(
        &self,
        run_id: &str,
        node_id: &str,
        phase: WorkflowNodePhase,
        detail: String,
        error: Option<String>,
    ) {
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id) {
            return;
        }
        if let Some(node) = document
            .snapshot
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
        {
            node.phase = phase;
            node.detail = detail;
            node.error = error;
        }
        document.snapshot.updated_ms = now_millis();
        self.persist_publish(&document);
    }

    fn complete_node(
        &self,
        run_id: &str,
        node_id: &str,
        started_ms: u64,
        detail: String,
        rows: Option<usize>,
    ) {
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id) {
            return;
        }
        if let Some(node) = document
            .snapshot
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
        {
            node.phase = WorkflowNodePhase::Complete;
            node.detail = detail;
            node.elapsed_ms = now_millis().saturating_sub(started_ms);
            node.rows = rows;
            node.error = None;
        }
        document.snapshot.updated_ms = now_millis();
        self.persist_publish(&document);
    }

    fn finish_completed(&self, run_id: &str, result: Option<SpiralQueryResponse>) {
        // Linearize terminal publication against cancellation. Whichever
        // claims the active-run mutex first owns the final outcome; a cancel
        // that arrives first prevents the just-finished ADBC payload from
        // replacing the previous stale result.
        let mut active = lock(&self.inner.active);
        let Some(control) = active.as_mut().filter(|active| active.run_id == run_id) else {
            return;
        };
        if control.cancel.load(Ordering::SeqCst) {
            drop(active);
            self.finish_cancelled(run_id);
            return;
        }
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id) {
            return;
        }
        let mut candidate = document.clone();
        let finished = now_millis();
        candidate.snapshot.phase = WorkflowRunPhase::Completed;
        candidate.snapshot.finished_ms = finished;
        candidate.snapshot.updated_ms = finished;
        candidate.snapshot.error = None;
        candidate.snapshot.result = result;
        candidate.snapshot.result_stale = false;
        if let Err(error) = self.save_document(&candidate) {
            control.terminal_persistence_failed = true;
            self.report_terminal_persistence_failure(run_id, &document, error);
            return;
        }
        *document = candidate;
        self.publish_committed_terminal(&document);
        drop(document);
        *active = None;
    }

    fn finish_failed(&self, run_id: &str, node_id: Option<&str>, error: String) {
        self.finish_failed_with_policy(
            run_id,
            node_id,
            error,
            FailureCancellationPolicy::CancelWins,
        );
    }

    fn finish_model_failed(&self, run_id: &str, node_id: &str, error: String) {
        // `run_model` classifies cancellation while it still owns the
        // publication boundary. A typed Failed outcome—including an uncertain
        // rollback—must not be rewritten as Cancelled by a token set after
        // that failure crossed the gate.
        self.finish_failed_with_policy(
            run_id,
            Some(node_id),
            error,
            FailureCancellationPolicy::FailureWins,
        );
    }

    fn finish_failed_with_policy(
        &self,
        run_id: &str,
        node_id: Option<&str>,
        error: String,
        policy: FailureCancellationPolicy,
    ) {
        let mut active = lock(&self.inner.active);
        let Some(control) = active.as_mut().filter(|active| active.run_id == run_id) else {
            return;
        };
        if cancellation_supersedes_failure(policy, control.cancel.load(Ordering::SeqCst)) {
            drop(active);
            self.finish_cancelled(run_id);
            return;
        }
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id) {
            return;
        }
        let mut candidate = document.clone();
        if let Some(node_id) = node_id
            && let Some(node) = candidate
                .snapshot
                .nodes
                .iter_mut()
                .find(|node| node.node_id == node_id)
        {
            node.phase = WorkflowNodePhase::Failed;
            node.detail = "Failed".to_owned();
            node.error = Some(error.clone());
        }
        let finished = now_millis();
        candidate.snapshot.phase = WorkflowRunPhase::Failed;
        candidate.snapshot.finished_ms = finished;
        candidate.snapshot.updated_ms = finished;
        candidate.snapshot.error = Some(error);
        if let Err(error) = self.save_document(&candidate) {
            control.terminal_persistence_failed = true;
            self.report_terminal_persistence_failure(run_id, &document, error);
            return;
        }
        *document = candidate;
        self.publish_committed_terminal(&document);
        drop(document);
        *active = None;
    }

    fn finish_cancelled(&self, run_id: &str) {
        let mut active = lock(&self.inner.active);
        let Some(control) = active.as_mut().filter(|active| active.run_id == run_id) else {
            return;
        };
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id) {
            return;
        }
        let mut candidate = document.clone();
        mark_inflight_nodes_cancelled(&mut candidate.snapshot.nodes);
        let finished = now_millis();
        candidate.snapshot.phase = WorkflowRunPhase::Cancelled;
        candidate.snapshot.finished_ms = finished;
        candidate.snapshot.updated_ms = finished;
        candidate.snapshot.error = None;
        if let Err(error) = self.save_document(&candidate) {
            control.terminal_persistence_failed = true;
            self.report_terminal_persistence_failure(run_id, &document, error);
            return;
        }
        *document = candidate;
        self.publish_committed_terminal(&document);
        drop(document);
        *active = None;
    }

    fn cancelled(&self, run_id: &str, cancel: &AtomicBool) -> bool {
        cancel.load(Ordering::SeqCst)
            || lock(&self.inner.active)
                .as_ref()
                .is_none_or(|active| active.run_id != run_id)
    }

    fn persist_publish(&self, document: &WorkflowDocument) {
        if let Err(error) = self.save_document(document) {
            tracing::error!(%error, "persisting Workflow snapshot");
        }
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: self.state_response(document),
        });
    }

    fn save_document(&self, document: &WorkflowDocument) -> Result<(), String> {
        #[cfg(test)]
        if let Some(outcome) = lock(&self.inner.persistence_outcomes).pop_front() {
            return outcome;
        }
        self.inner.storage.save_workflow_document(document)
    }

    fn require_scope(&self, requested_scope_id: &str) -> Result<(), ApplicationError> {
        if requested_scope_id != self.scope_id() {
            return Err(ApplicationError::Conflict(
                "The Spiral project changed before the Workflow command arrived. Reload Workflow and try again."
                    .to_owned(),
            ));
        }
        Ok(())
    }

    fn report_terminal_persistence_failure(
        &self,
        run_id: &str,
        document: &WorkflowDocument,
        error: String,
    ) {
        let detail = format!(
            "Could not durably finalize this Workflow run: {error} The run remains active; choose Cancel to retry durable finalization."
        );
        tracing::error!(%error, %run_id, "persisting terminal Workflow snapshot");
        *lock(&self.inner.terminal_persistence_error) = Some(TerminalPersistenceError {
            run_id: run_id.to_owned(),
            detail,
        });
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: self.state_response(document),
        });
    }

    fn publish_committed_terminal(&self, document: &WorkflowDocument) {
        *lock(&self.inner.terminal_persistence_error) = None;
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: self.state_response(document),
        });
    }

    fn state_response(&self, document: &WorkflowDocument) -> WorkflowStateResponse {
        let mut response = state_response(document);
        response.scope_id = self.scope_id();
        if let Some(error) = lock(&self.inner.terminal_persistence_error).as_ref()
            && response.snapshot.run_id.as_deref() == Some(error.run_id.as_str())
            && response.snapshot.phase.active()
        {
            response.snapshot.error = Some(error.detail.clone());
        }
        response
    }
}

fn mark_inflight_nodes_cancelled(nodes: &mut [WorkflowNodeSnapshot]) {
    for node in nodes {
        if matches!(
            node.phase,
            WorkflowNodePhase::Pending
                | WorkflowNodePhase::Validating
                | WorkflowNodePhase::Loading
                | WorkflowNodePhase::Running
                | WorkflowNodePhase::Cancelling
        ) {
            node.phase = WorkflowNodePhase::Cancelled;
            node.detail = "Cancelled".to_owned();
        }
    }
}

fn state_response(document: &WorkflowDocument) -> WorkflowStateResponse {
    WorkflowStateResponse {
        scope_id: String::new(),
        workflow: document.workflow.clone(),
        validation: validate_workflow_spec(&document.workflow),
        snapshot: document.snapshot.clone(),
    }
}

#[derive(Clone)]
enum ResolvedNodeOutput {
    Relation(String),
    Query(String),
}

fn substitute_outputs(
    sql: &str,
    outputs: &BTreeMap<String, ResolvedNodeOutput>,
) -> Result<String, String> {
    let mut substituted = sql.to_owned();
    for id in scan_node_placeholders(sql).map_err(|error| error.to_string())? {
        let output = outputs.get(&id).ok_or_else(|| {
            format!("Workflow query references unresolved node placeholder {id:?}")
        })?;
        let replacement = match output {
            ResolvedNodeOutput::Relation(relation) => {
                validate_qualified_relation(relation).map_err(|error| error.to_string())?;
                relation.clone()
            }
            ResolvedNodeOutput::Query(query) => format!("({query})"),
        };
        substituted = substituted.replace(&format!("{{{{node:{id}}}}}"), &replacement);
    }
    Ok(substituted)
}

fn derive_model_outputs(workflow: &mut WorkflowSpec) {
    let source_relations = workflow
        .nodes
        .iter()
        .filter(|node| matches!(node.config, WorkflowNodeConfig::Model { .. }))
        .map(|node| (node.id.clone(), model_source_relation(workflow, &node.id)))
        .collect::<BTreeMap<_, _>>();
    for node in &mut workflow.nodes {
        if let WorkflowNodeConfig::Model {
            features,
            feature_cache_relation,
            ..
        } = &mut node.config
        {
            let source = source_relations.get(&node.id).cloned().flatten();
            if let Some(source) = source.as_deref() {
                for feature in features {
                    feature.applicability.source_relation = source.to_owned();
                }
            }
            *feature_cache_relation = source.map(|source| derived_feature_cache_relation(&source));
        }
    }
}

fn model_source_relation(workflow: &WorkflowSpec, model_id: &str) -> Option<String> {
    let mut sources = std::collections::BTreeSet::new();
    let mut visited = std::collections::BTreeSet::new();
    let mut pending = workflow
        .edges
        .iter()
        .filter(|edge| edge.target == model_id)
        .map(|edge| edge.source.as_str())
        .collect::<Vec<_>>();
    while let Some(node_id) = pending.pop() {
        if !visited.insert(node_id) {
            continue;
        }
        let node = workflow.nodes.iter().find(|node| node.id == node_id)?;
        match &node.config {
            WorkflowNodeConfig::Table { relation } => {
                sources.insert(relation.clone());
            }
            _ => pending.extend(
                workflow
                    .edges
                    .iter()
                    .filter(|edge| edge.target == node_id)
                    .map(|edge| edge.source.as_str()),
            ),
        }
    }
    (sources.len() == 1)
        .then(|| sources.into_iter().next())
        .flatten()
}

fn first_table_id(workflow: &WorkflowSpec) -> Option<&str> {
    workflow.nodes.iter().find_map(|node| {
        matches!(node.config, WorkflowNodeConfig::Table { .. }).then_some(node.id.as_str())
    })
}

fn workflow_content_changed(current: &WorkflowSpec, next: &WorkflowSpec) -> bool {
    let mut normalized = next.clone();
    normalized.revision = current.revision;
    current != &normalized
}

fn saved_graph_node_snapshots(
    previous: &WorkflowRunSnapshot,
    workflow: &WorkflowSpec,
    had_result: bool,
) -> Vec<WorkflowNodeSnapshot> {
    workflow
        .nodes
        .iter()
        .map(|node| {
            let prior = previous
                .nodes
                .iter()
                .find(|snapshot| snapshot.node_id == node.id);
            WorkflowNodeSnapshot {
                node_id: node.id.clone(),
                phase: if had_result {
                    WorkflowNodePhase::Stale
                } else {
                    WorkflowNodePhase::Pending
                },
                detail: if had_result {
                    "Saved graph changed; run again".to_owned()
                } else {
                    "Not run".to_owned()
                },
                model: prior.and_then(|snapshot| snapshot.model.clone()),
                quarantine: prior
                    .map(|snapshot| snapshot.quarantine.clone())
                    .unwrap_or_default(),
                ..WorkflowNodeSnapshot::default()
            }
        })
        .collect()
}

fn starting_snapshot(
    workflow: &WorkflowSpec,
    previous: &WorkflowRunSnapshot,
    run_id: String,
) -> WorkflowRunSnapshot {
    let started_ms = now_millis();
    WorkflowRunSnapshot {
        workflow_id: workflow.id.clone(),
        workflow_revision: workflow.revision,
        run_id: Some(run_id),
        phase: WorkflowRunPhase::Validating,
        nodes: workflow
            .nodes
            .iter()
            .map(|node| {
                let prior = previous
                    .nodes
                    .iter()
                    .find(|snapshot| snapshot.node_id == node.id);
                WorkflowNodeSnapshot {
                    node_id: node.id.clone(),
                    phase: WorkflowNodePhase::Pending,
                    detail: "Pending".to_owned(),
                    model: prior.and_then(|snapshot| snapshot.model.clone()),
                    quarantine: prior
                        .map(|snapshot| snapshot.quarantine.clone())
                        .unwrap_or_default(),
                    ..WorkflowNodeSnapshot::default()
                }
            })
            .collect(),
        result: previous.result.clone(),
        error: None,
        result_stale: previous.result.is_some(),
        started_ms,
        finished_ms: 0,
        updated_ms: started_ms,
    }
}

fn next_run_id() -> String {
    format!(
        "workflow-{}-{}",
        now_millis(),
        RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn next_scope_id() -> String {
    format!(
        "workflow-scope-{}-{}",
        now_millis(),
        SCOPE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn cancellation_supersedes_failure(
    policy: FailureCancellationPolicy,
    cancel_requested: bool,
) -> bool {
    cancel_requested && matches!(policy, FailureCancellationPolicy::CancelWins)
}

fn workflow_save_allowed(workflow: &WorkflowSpec, validation_valid: bool) -> bool {
    validation_valid || (workflow.nodes.is_empty() && workflow.edges.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use arrow_array::{ArrayRef, Int64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use spiral_robotics_contracts::api::SpiralQueryResponse;
    use spiral_robotics_contracts::workflow::{
        WorkflowEdgeSpec, WorkflowFeatureApplicability, WorkflowFeatureDefinition,
        WorkflowModelRunMetadata, WorkflowNodeConfig, WorkflowNodeSpec, WorkflowRunPhase,
    };

    use crate::backend::robotics::video::VideoAsset;
    use crate::backend::storage::manager::{
        ensure_dataset_namespace, execute_update_sql, ingest_batch, rebuild_video_dataset,
        replace_batches, table_exists, video_assets_batch,
    };
    use crate::backend::storage::repository::SpiralRepository;
    use crate::backend::storage::settings::{
        BackendStateStore, environment_spiral_endpoint, environment_spiral_spfs_endpoint,
    };
    use crate::backend::storage::sql;

    #[test]
    fn revision_increment_alone_does_not_stale_a_result() {
        let current = WorkflowSpec::default();
        let mut next = current.clone();
        next.revision += 1;
        assert!(!workflow_content_changed(&current, &next));
        next.name = "Changed".to_owned();
        assert!(workflow_content_changed(&current, &next));
    }

    #[test]
    fn an_intentionally_empty_draft_can_save_but_remains_non_runnable() {
        let mut workflow = WorkflowSpec::default();
        workflow.nodes.clear();
        workflow.edges.clear();
        let validation = validate_workflow_spec(&workflow);

        assert!(!validation.valid());
        assert!(workflow_save_allowed(&workflow, validation.valid()));

        workflow.nodes.push(WorkflowNodeSpec {
            id: "orphan".to_owned(),
            name: "Orphan table".to_owned(),
            config: WorkflowNodeConfig::Table {
                relation: "robotics.frames".to_owned(),
            },
        });
        let invalid = validate_workflow_spec(&workflow);
        assert!(!workflow_save_allowed(&workflow, invalid.valid()));
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
        let sql = substitute_outputs(
            "SELECT * FROM {{node:candidates}} c JOIN {{node:table}} t ON c.frame_index = t.frame_index",
            &outputs,
        )
        .unwrap();
        assert!(sql.contains("FROM (SELECT episode_id, frame_index FROM robotics.frames) c"));
        assert!(sql.contains("JOIN robotics.frames t"));
        assert!(!sql.contains("{{node:"));
    }

    #[test]
    fn model_output_is_one_shared_server_owned_feature_cache() {
        let mut workflow = siglip_video_workflow("media.manifest", "model");
        derive_model_outputs(&mut workflow);
        let WorkflowNodeConfig::Model {
            feature_cache_relation,
            ..
        } = &workflow
            .nodes
            .iter()
            .find(|node| node.id == "model")
            .unwrap()
            .config
        else {
            unreachable!()
        };
        assert_eq!(
            feature_cache_relation.as_deref(),
            Some(derived_feature_cache_relation("media.manifest").as_str())
        );
    }

    #[test]
    fn rerun_keeps_previous_rows_visibly_stale_until_result_replacement() {
        let workflow = WorkflowSpec::default();
        let mut previous = WorkflowRunSnapshot::idle_for(&workflow);
        previous.result = Some(SpiralQueryResponse {
            columns: Vec::new(),
            rows: vec![serde_json::Map::new()],
            truncated: false,
            elapsed_ms: 4,
        });
        let next = starting_snapshot(&workflow, &previous, "run".to_owned());
        assert_eq!(next.result, previous.result);
        assert!(next.result_stale);
        assert_eq!(next.phase, WorkflowRunPhase::Validating);
    }

    #[test]
    fn cancellation_after_model_publication_preserves_the_completed_model_truth() {
        let mut nodes = vec![
            WorkflowNodeSnapshot {
                node_id: "model".to_owned(),
                phase: WorkflowNodePhase::Complete,
                detail: "Backfilled 4 feature cells".to_owned(),
                ..WorkflowNodeSnapshot::default()
            },
            WorkflowNodeSnapshot {
                node_id: "query".to_owned(),
                phase: WorkflowNodePhase::Running,
                detail: "Running read-only query".to_owned(),
                ..WorkflowNodeSnapshot::default()
            },
        ];
        mark_inflight_nodes_cancelled(&mut nodes);
        assert_eq!(nodes[0].phase, WorkflowNodePhase::Complete);
        assert_eq!(nodes[0].detail, "Backfilled 4 feature cells");
        assert_eq!(nodes[1].phase, WorkflowNodePhase::Cancelled);
    }

    #[test]
    fn late_cancel_does_not_hide_a_model_publication_failure() {
        assert!(cancellation_supersedes_failure(
            FailureCancellationPolicy::CancelWins,
            true
        ));
        assert!(
            !cancellation_supersedes_failure(FailureCancellationPolicy::FailureWins, true),
            "a rollback or publication failure remains Failed after a later cancellation token"
        );
    }

    #[test]
    fn terminal_save_failure_keeps_the_run_active_and_cancel_can_retry_finalization() {
        let repository = Arc::new(
            SpiralRepository::new(
                "https://api.spiraldb.dev".to_owned(),
                "https://spfs.spiraldb.dev".to_owned(),
                None,
                None,
                "workflow_terminal_failure".to_owned(),
            )
            .expect("the unconfigured test repository is valid"),
        );
        let manager = WorkflowManager::new(Arc::new(StorageManager::new(repository)));
        let workflow = WorkflowSpec::default();
        let run_id = "terminal-save-failure".to_owned();
        let mut snapshot = starting_snapshot(
            &workflow,
            &WorkflowRunSnapshot::idle_for(&workflow),
            run_id.clone(),
        );
        snapshot.phase = WorkflowRunPhase::Running;
        *lock(&manager.inner.document) = WorkflowDocument { workflow, snapshot };
        *lock(&manager.inner.active) = Some(ActiveRun {
            run_id: run_id.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            publication_gate: Arc::new(Mutex::new(())),
            task: None,
            terminal_persistence_failed: false,
        });
        let (_, mut updates) = manager.subscription();
        lock(&manager.inner.persistence_outcomes)
            .push_back(Err("injected terminal write failure".to_owned()));

        manager.finish_completed(&run_id, None);

        let state = manager.state();
        assert_eq!(state.snapshot.phase, WorkflowRunPhase::Running);
        assert!(state.snapshot.error.as_deref().is_some_and(|error| {
            error.contains("Could not durably finalize") && error.contains("choose Cancel")
        }));
        assert!(manager.inner.active.lock().unwrap().is_some());
        let published = updates
            .try_recv()
            .expect("the recovery state must be published");
        assert_eq!(published.state.snapshot.phase, WorkflowRunPhase::Running);

        lock(&manager.inner.persistence_outcomes).extend([Ok(()), Ok(())]);
        let recovered = manager
            .cancel(CancelWorkflowRequest {
                scope_id: manager.scope_id(),
            })
            .expect("Cancel must retry the durable terminal transition");
        assert_eq!(recovered.phase, WorkflowRunPhase::Cancelled);
        assert!(recovered.error.is_none());
        assert!(manager.inner.active.lock().unwrap().is_none());
    }

    #[test]
    fn authoritative_project_reload_gets_a_new_scope_for_a_lower_revision() {
        let repository = Arc::new(
            SpiralRepository::new(
                "https://api.spiraldb.dev".to_owned(),
                "https://spfs.spiraldb.dev".to_owned(),
                None,
                None,
                "workflow_scope_reload".to_owned(),
            )
            .expect("the unconfigured test repository is valid"),
        );
        let manager = WorkflowManager::new(Arc::new(StorageManager::new(repository)));
        let mut project_a = WorkflowDocument::default();
        project_a.workflow.revision = 9;
        project_a.snapshot.workflow_revision = 9;
        manager
            .install_hydrated_document(project_a)
            .expect("the first project installs");
        let scope_a = manager.state().scope_id;

        let mut project_b = WorkflowDocument::default();
        project_b.workflow.revision = 1;
        project_b.snapshot.workflow_revision = 1;
        manager
            .install_hydrated_document(project_b)
            .expect("the replacement project installs");
        let state_b = manager.state();

        assert_ne!(state_b.scope_id, scope_a);
        assert_eq!(state_b.workflow.revision, 1);
        assert!(matches!(
            manager.save(SaveWorkflowRequest {
                scope_id: scope_a,
                expected_revision: state_b.workflow.revision,
                workflow: state_b.workflow,
            }),
            Err(ApplicationError::Conflict(_))
        ));
    }

    #[test]
    fn query_only_graph_edits_preserve_model_fingerprint_for_safe_reuse() {
        let mut workflow = siglip_video_workflow("media.manifest", "model");
        derive_model_outputs(&mut workflow);
        let mut previous = WorkflowRunSnapshot::idle_for(&workflow);
        let metadata = WorkflowModelRunMetadata {
            model_id: "siglip-base".to_owned(),
            artifact_revision: "revision".to_owned(),
            feature_cache_relation: derived_feature_cache_relation("media.manifest"),
            features: Vec::new(),
            fingerprint: "fingerprint".to_owned(),
            reused: false,
            device: "Metal".to_owned(),
            input_rows: 2,
            output_rows: 2,
            decode_ms: 1,
            inference_ms: 1,
        };
        previous
            .nodes
            .iter_mut()
            .find(|node| node.node_id == "model")
            .unwrap()
            .model = Some(metadata.clone());
        let WorkflowNodeConfig::Query { sql, .. } = &mut workflow
            .nodes
            .iter_mut()
            .find(|node| node.id == "query")
            .unwrap()
            .config
        else {
            unreachable!()
        };
        sql.push_str(" LIMIT 1");

        let saved = saved_graph_node_snapshots(&previous, &workflow, true);
        assert_eq!(
            saved
                .iter()
                .find(|node| node.node_id == "model")
                .and_then(|node| node.model.as_ref()),
            Some(&metadata)
        );
    }

    #[test]
    fn model_cache_identity_ignores_hostile_persisted_output_text() {
        let mut current = siglip_video_workflow("media.manifest", "model");
        let WorkflowNodeConfig::Model {
            feature_cache_relation,
            ..
        } = &mut current
            .nodes
            .iter_mut()
            .find(|node| node.id == "model")
            .unwrap()
            .config
        else {
            unreachable!()
        };
        *feature_cache_relation = Some("training.episodes".to_owned());
        derive_model_outputs(&mut current);
        let WorkflowNodeConfig::Model {
            feature_cache_relation,
            ..
        } = &current
            .nodes
            .iter()
            .find(|node| node.id == "model")
            .unwrap()
            .config
        else {
            unreachable!()
        };
        assert_eq!(
            feature_cache_relation.as_deref(),
            Some(derived_feature_cache_relation("media.manifest").as_str())
        );
    }

    #[test]
    fn model_cache_and_feature_source_are_server_derived() {
        let mut workflow = siglip_video_workflow("pick_place.camera_frames", "score-node");
        let WorkflowNodeConfig::Model { features, .. } = &mut workflow
            .nodes
            .iter_mut()
            .find(|node| node.id == "score-node")
            .unwrap()
            .config
        else {
            unreachable!()
        };
        features[0].applicability.source_relation = "provider.chosen".to_owned();
        derive_model_outputs(&mut workflow);
        let WorkflowNodeConfig::Model {
            feature_cache_relation,
            features,
            ..
        } = &workflow
            .nodes
            .iter()
            .find(|node| node.id == "score-node")
            .unwrap()
            .config
        else {
            unreachable!()
        };
        assert_eq!(
            feature_cache_relation.as_deref(),
            Some(derived_feature_cache_relation("pick_place.camera_frames").as_str())
        );
        assert_eq!(
            features[0].applicability.source_relation,
            "pick_place.camera_frames"
        );
    }

    /// Run against a disposable Spiral project with:
    ///
    /// ```text
    /// SPIRAL_PROJECT_ID=project-id cargo test -p spiral-robotics-studio-backend \
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
        let namespace = format!("workflow_e2e_{}_{}", std::process::id(), now_millis());
        let repository = Arc::new(
            SpiralRepository::new(
                stored
                    .as_ref()
                    .and_then(BackendStateStore::spiral_endpoint)
                    .unwrap_or_else(environment_spiral_endpoint),
                stored
                    .as_ref()
                    .and_then(BackendStateStore::spiral_spfs_endpoint)
                    .unwrap_or_else(environment_spiral_spfs_endpoint),
                Some(project),
                stored.as_ref().and_then(BackendStateStore::credentials),
                namespace.clone(),
            )
            .expect("the Workflow E2E repository must be valid"),
        );
        let seed_relation = format!("{namespace}.seed_values");
        let mut cleanup = WorkflowTestNamespace::new(
            repository.clone(),
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
        repository
            .run_exclusive_blocking(move |connection| {
                ensure_dataset_namespace(connection, &dataset)?;
                ingest_batch(connection, &target, batch)
            })
            .expect("seeding the Workflow relation must succeed");
        repository.invalidate_idle_connections();

        let storage = Arc::new(StorageManager::new(repository.clone()));
        let manager = WorkflowManager::new(storage.clone());
        manager
            .hydrate_from_storage()
            .await
            .expect("an empty Workflow document must hydrate");
        let mut workflow = WorkflowSpec::default();
        let WorkflowNodeConfig::Table { relation } = &mut workflow.nodes[0].config else {
            unreachable!("the default workflow starts with a Table")
        };
        *relation = seed_relation;
        let saved = manager
            .save(SaveWorkflowRequest {
                scope_id: manager.scope_id(),
                expected_revision: 0,
                workflow,
            })
            .expect("the valid SQL workflow must save");
        let request = RunWorkflowRequest {
            scope_id: manager.scope_id(),
            revision: saved.workflow.revision,
        };
        manager
            .start(request.clone())
            .expect("the first run must start");
        assert!(
            matches!(
                manager.start(request.clone()),
                Err(ApplicationError::Conflict(_))
            ),
            "one workflow cannot have two active runs"
        );
        let completed = wait_for_terminal(&manager).await;
        assert_eq!(
            completed.phase,
            WorkflowRunPhase::Completed,
            "Workflow failed: {:?}",
            completed.error
        );
        assert_eq!(
            completed.result.as_ref().map(|result| result.rows.len()),
            Some(3)
        );

        let reloaded = WorkflowManager::new(storage);
        reloaded
            .hydrate_from_storage()
            .await
            .expect("the saved graph and result must reload");
        assert_eq!(reloaded.state().workflow, saved.workflow);
        assert_eq!(reloaded.state().snapshot.result, completed.result);

        reloaded
            .start(RunWorkflowRequest {
                scope_id: reloaded.scope_id(),
                revision: request.revision,
            })
            .expect("the persisted workflow must run again");
        let cancelling = reloaded
            .cancel(CancelWorkflowRequest {
                scope_id: reloaded.scope_id(),
            })
            .expect("the active run must cancel");
        assert_eq!(cancelling.phase, WorkflowRunPhase::Cancelling);
        let cancelled = wait_for_terminal(&reloaded).await;
        assert_eq!(cancelled.phase, WorkflowRunPhase::Cancelled);
        assert!(cancelled.result_stale);
        assert_eq!(cancelled.result, completed.result);
        reloaded.shutdown().await;
        manager.shutdown().await;
        repository.invalidate_idle_connections();
        cleanup
            .cleanup_now()
            .expect("the SQL E2E namespace must clean up synchronously");
        cleanup
            .assert_absent()
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
        let namespace = format!("workflow_siglip_{}_{}", std::process::id(), now_millis());
        let repository = Arc::new(
            SpiralRepository::new(
                stored
                    .as_ref()
                    .and_then(BackendStateStore::spiral_endpoint)
                    .unwrap_or_else(environment_spiral_endpoint),
                stored
                    .as_ref()
                    .and_then(BackendStateStore::spiral_spfs_endpoint)
                    .unwrap_or_else(environment_spiral_spfs_endpoint),
                Some(project),
                stored.as_ref().and_then(BackendStateStore::credentials),
                namespace.clone(),
            )
            .expect("the Workflow SigLIP E2E repository must be valid"),
        );
        let manifest_relation = format!("{namespace}.video_assets");
        let media_relation = format!("{namespace}.video.assets");
        let empty_relation = format!("{namespace}.empty_video_assets");
        let model_node_id = format!("model_{}_{}", std::process::id(), now_millis());
        let mut workflow = siglip_video_workflow(&manifest_relation, &model_node_id);
        set_video_candidate_sql(&mut workflow, &media_relation);
        derive_model_outputs(&mut workflow);
        let output_relation = workflow
            .nodes
            .iter()
            .find_map(|node| match &node.config {
                WorkflowNodeConfig::Model {
                    feature_cache_relation: Some(relation),
                    ..
                } => Some(relation.clone()),
                _ => None,
            })
            .expect("the server derivation must produce a Model output relation");
        let empty_output_relation = derived_feature_cache_relation(&empty_relation);
        // Install cleanup before the first catalog mutation so a seeding or
        // Dataset-build failure cannot strand a test namespace.
        let mut cleanup = WorkflowTestNamespace::new(
            repository.clone(),
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
        let asset = VideoAsset {
            stream_id: "camera.main".to_owned(),
            camera_id: "camera-e2e".to_owned(),
            payload: tiny_test_mp4(),
            width: 32,
            height: 32,
            fps: 2,
            frames: 2,
            segment_index: 0,
            timeline_offset_seconds: Some(0.0),
        };
        let media_batch = video_assets_batch("episode-e2e", &[asset], &media_relation)
            .expect("the video fixture batch must be valid");
        let empty_batch = video_assets_batch("episode-empty", &[], &empty_relation)
            .expect("the empty video fixture schema must be valid");
        let media_target = manifest_relation.clone();
        let empty_target = empty_relation.clone();
        let dataset = namespace.clone();
        repository
            .run_exclusive_blocking(move |connection| {
                ensure_dataset_namespace(connection, &dataset)?;
                ingest_batch(connection, &media_target, media_batch)?;
                replace_batches(connection, &empty_target, vec![empty_batch])?;
                rebuild_video_dataset(connection, &dataset, false)
            })
            .expect("seeding the video manifest relations must succeed");
        repository.invalidate_idle_connections();

        let storage = Arc::new(StorageManager::new(repository.clone()));
        let manager = WorkflowManager::new(storage.clone());
        manager
            .hydrate_from_storage()
            .await
            .expect("an empty Workflow document must hydrate");
        let saved = manager
            .save(SaveWorkflowRequest {
                scope_id: manager.scope_id(),
                expected_revision: 0,
                workflow,
            })
            .expect("the SigLIP video workflow must save");

        let request = RunWorkflowRequest {
            scope_id: manager.scope_id(),
            revision: saved.workflow.revision,
        };
        manager
            .start(request.clone())
            .expect("the SigLIP video run must start");
        let completed = wait_for_model_terminal(&manager).await;
        assert_eq!(
            completed.phase,
            WorkflowRunPhase::Completed,
            "Workflow failed: {:?}",
            completed.error
        );
        let result = completed
            .result
            .as_ref()
            .expect("Result must own real rows");
        assert_eq!(result.rows.len(), 2);
        for row in &result.rows {
            for column in ["is_black_square_feature", "is_white_square_feature"] {
                let score = row
                    .get(column)
                    .and_then(serde_json::Value::as_f64)
                    .expect("SigLIP score columns must be numeric");
                assert!((0.0..=1.0).contains(&score));
            }
        }
        let metadata = completed
            .nodes
            .iter()
            .find(|node| node.node_id == model_node_id)
            .and_then(|node| node.model.as_ref())
            .expect("the Model snapshot must expose measured metadata");
        assert_eq!(metadata.input_rows, 2);
        assert_eq!(metadata.output_rows, 2);
        assert!(
            metadata.decode_ms > 0,
            "Spiral decode timing must be measured"
        );
        assert!(metadata.inference_ms > 0, "SigLIP timing must be measured");
        eprintln!(
            "workflow SigLIP real smoke: device={}, rows=2, decode_ms={}, inference_ms={}, decode_fps={:.2}",
            metadata.device,
            metadata.decode_ms,
            metadata.inference_ms,
            2_000.0 / metadata.decode_ms as f64,
        );
        manager
            .start(request.clone())
            .expect("an unchanged run must start for reuse");
        let reused = wait_for_model_terminal(&manager).await;
        assert!(
            reused
                .nodes
                .iter()
                .find(|node| node.node_id == model_node_id)
                .and_then(|node| node.model.as_ref())
                .is_some_and(|metadata| metadata.reused),
            "the unchanged fingerprint must reuse the durable relation"
        );

        let reloaded = WorkflowManager::new(storage);
        reloaded
            .hydrate_from_storage()
            .await
            .expect("the SigLIP workflow and snapshot must reload");
        assert_eq!(reloaded.state().snapshot.result, reused.result);
        assert!(
            reloaded
                .state()
                .snapshot
                .nodes
                .iter()
                .find(|node| node.node_id == model_node_id)
                .and_then(|node| node.model.as_ref())
                .is_some_and(|metadata| metadata.reused)
        );

        let mut empty_workflow = siglip_video_workflow(&empty_relation, &model_node_id);
        set_video_candidate_sql(&mut empty_workflow, &media_relation);
        let empty_saved = reloaded
            .save(SaveWorkflowRequest {
                scope_id: reloaded.scope_id(),
                expected_revision: saved.workflow.revision,
                workflow: empty_workflow,
            })
            .expect("the empty manifest workflow must save");
        reloaded
            .start(RunWorkflowRequest {
                scope_id: reloaded.scope_id(),
                revision: empty_saved.workflow.revision,
            })
            .expect("the zero-row replacement run must start");
        let empty = wait_for_model_terminal(&reloaded).await;
        assert_eq!(empty.phase, WorkflowRunPhase::Completed);
        assert_eq!(
            empty.result.as_ref().map(|result| result.rows.len()),
            Some(0)
        );
        let empty_metadata = empty
            .nodes
            .iter()
            .find(|node| node.node_id == model_node_id)
            .and_then(|node| node.model.as_ref())
            .expect("the zero-row run must retain a typed Model snapshot");
        assert_eq!(empty_metadata.output_rows, 0);
        assert_eq!(
            empty_metadata.device,
            "not loaded (empty applicable cohort)"
        );

        let mut restored_workflow = siglip_video_workflow(&manifest_relation, &model_node_id);
        set_video_candidate_sql(&mut restored_workflow, &media_relation);
        let changed_saved = reloaded
            .save(SaveWorkflowRequest {
                scope_id: reloaded.scope_id(),
                expected_revision: empty_saved.workflow.revision,
                workflow: restored_workflow,
            })
            .expect("restoring the media workflow must save");
        reloaded
            .start(RunWorkflowRequest {
                scope_id: reloaded.scope_id(),
                revision: changed_saved.workflow.revision,
            })
            .expect("the cancellable model run must start");
        let cancelling = reloaded
            .cancel(CancelWorkflowRequest {
                scope_id: reloaded.scope_id(),
            })
            .expect("the active Model run must cancel");
        assert_eq!(cancelling.phase, WorkflowRunPhase::Cancelling);
        assert_eq!(
            wait_for_model_terminal(&reloaded).await.phase,
            WorkflowRunPhase::Cancelled
        );

        // Removing a Model must not delete the source-scoped cache: a later
        // workflow over the same source owns the same registered features.
        let mut without_model = reloaded.state().workflow;
        without_model.nodes.retain(|node| node.id != model_node_id);
        without_model
            .edges
            .retain(|edge| edge.source != model_node_id && edge.target != model_node_id);
        without_model.edges.insert(
            0,
            WorkflowEdgeSpec {
                source: "table".to_owned(),
                target: "query".to_owned(),
            },
        );
        let WorkflowNodeConfig::Query { sql, .. } = &mut without_model
            .nodes
            .iter_mut()
            .find(|node| node.id == "query")
            .expect("the test workflow retains its Query")
            .config
        else {
            unreachable!()
        };
        *sql = "SELECT * FROM {{node:table}} ORDER BY episode_id".to_owned();
        let removed = reloaded
            .save(SaveWorkflowRequest {
                scope_id: reloaded.scope_id(),
                expected_revision: without_model.revision,
                workflow: without_model,
            })
            .expect("removing the Model must save without deleting shared cache state");
        let manifest_check = manifest_relation.clone();
        let output_check = output_relation.clone();
        repository
            .run_exclusive_blocking(move |connection| {
                if !table_exists(connection, &manifest_check)? {
                    return Err("hostile stored output text dropped the input manifest".to_owned());
                }
                if !table_exists(connection, &output_check)? {
                    return Err("removing one Model deleted the shared feature cache".to_owned());
                }
                Ok(())
            })
            .expect("Model removal must preserve source and shared cache tables");
        let mut renamed = removed.workflow;
        renamed.name = "Post-cleanup write".to_owned();
        let renamed = reloaded
            .save(SaveWorkflowRequest {
                scope_id: reloaded.scope_id(),
                expected_revision: renamed.revision,
                workflow: renamed,
            })
            .expect("ordinary persistence must work after cleanup restores autocommit");
        reloaded
            .reload_from_storage()
            .expect("the post-cleanup document must remain readable");
        assert_eq!(reloaded.state().workflow.name, renamed.workflow.name);
        reloaded.shutdown().await;
        manager.shutdown().await;
        repository.invalidate_idle_connections();
        cleanup
            .cleanup_now()
            .expect("the SigLIP E2E namespace must clean up synchronously");
        cleanup
            .assert_absent()
            .expect("the SigLIP E2E must leave no catalog resources behind");
    }

    fn siglip_video_workflow(relation: &str, model_node_id: &str) -> WorkflowSpec {
        WorkflowSpec {
            id: "default".to_owned(),
            name: "SigLIP video E2E".to_owned(),
            revision: 0,
            nodes: vec![
                WorkflowNodeSpec {
                    id: "table".to_owned(),
                    name: "Video manifest".to_owned(),
                    config: WorkflowNodeConfig::Table {
                        relation: relation.to_owned(),
                    },
                },
                WorkflowNodeSpec {
                    id: "candidates".to_owned(),
                    name: "Decode candidate frames".to_owned(),
                    config: WorkflowNodeConfig::Query {
                        sql: "SELECT episode_id, stream_id, segment_index, frame_index, episode_time_ns, image FROM {{node:table}}".to_owned(),
                        max_rows: 100,
                    },
                },
                WorkflowNodeSpec {
                    id: model_node_id.to_owned(),
                    name: "Backfill visual features".to_owned(),
                    config: WorkflowNodeConfig::Model {
                        model_id: "siglip-base".to_owned(),
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
                        feature_cache_relation: None,
                    },
                },
                WorkflowNodeSpec {
                    id: "query".to_owned(),
                    name: "Inspect scores".to_owned(),
                    config: WorkflowNodeConfig::Query {
                        sql: format!(
                            "SELECT candidates.*, features.is_black_square_feature, features.is_white_square_feature FROM {{{{node:candidates}}}} candidates JOIN {{{{node:{model_node_id}}}}} features ON candidates.episode_id = features.episode_id AND candidates.stream_id = features.stream_id AND candidates.segment_index = features.segment_index AND candidates.frame_index = features.frame_index AND candidates.episode_time_ns = features.episode_time_ns ORDER BY candidates.frame_index"
                        ),
                        max_rows: 100,
                    },
                },
                WorkflowNodeSpec {
                    id: "result".to_owned(),
                    name: "Scored frames".to_owned(),
                    config: WorkflowNodeConfig::Result,
                },
            ],
            edges: vec![
                WorkflowEdgeSpec {
                    source: "table".to_owned(),
                    target: "candidates".to_owned(),
                },
                WorkflowEdgeSpec {
                    source: "candidates".to_owned(),
                    target: model_node_id.to_owned(),
                },
                WorkflowEdgeSpec {
                    source: "candidates".to_owned(),
                    target: "query".to_owned(),
                },
                WorkflowEdgeSpec {
                    source: model_node_id.to_owned(),
                    target: "query".to_owned(),
                },
                WorkflowEdgeSpec {
                    source: "query".to_owned(),
                    target: "result".to_owned(),
                },
            ],
        }
    }

    fn set_video_candidate_sql(workflow: &mut WorkflowSpec, media_relation: &str) {
        let WorkflowNodeConfig::Query { sql, .. } = &mut workflow
            .nodes
            .iter_mut()
            .find(|node| node.id == "candidates")
            .expect("the test workflow has a candidate Query")
            .config
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
        WorkflowFeatureDefinition {
            name: name.to_owned(),
            description: format!("Probability that the frame shows {concept}"),
            concept: concept.to_owned(),
            applicability: WorkflowFeatureApplicability {
                source_relation: source_relation.to_owned(),
                media_field: "image".to_owned(),
                stream_ids: Vec::new(),
            },
        }
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
            let snapshot = manager.state().snapshot;
            if snapshot.phase.terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("Workflow Model run did not reach a terminal state within 60 seconds");
    }

    async fn wait_for_terminal(manager: &WorkflowManager) -> WorkflowRunSnapshot {
        for _ in 0..400 {
            let snapshot = manager.state().snapshot;
            if snapshot.phase.terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("Workflow run did not reach a terminal state within 10 seconds");
    }

    struct WorkflowTestNamespace {
        repository: Arc<SpiralRepository>,
        relations: Vec<String>,
        namespace: String,
        video_dataset: bool,
        cleaned: bool,
    }

    impl WorkflowTestNamespace {
        fn new(
            repository: Arc<SpiralRepository>,
            relations: Vec<String>,
            namespace: String,
            video_dataset: bool,
        ) -> Self {
            Self {
                repository,
                relations,
                namespace,
                video_dataset,
                cleaned: false,
            }
        }

        fn cleanup_now(&mut self) -> Result<(), String> {
            if self.cleaned {
                return Ok(());
            }
            if self.video_dataset {
                let dataset = format!("{}.video", self.namespace);
                self.execute_cleanup_sql(&sql::drop_video_dataset(&dataset))?;
                self.execute_cleanup_sql(&format!(
                    "DROP VIEW IF EXISTS {}_video_sources",
                    self.namespace
                ))?;
            }
            for relation in self.relations.clone() {
                self.drop_cleanup_table(&relation)?;
            }
            self.drop_cleanup_table(&self.namespace)?;
            self.cleaned = true;
            Ok(())
        }

        fn execute_cleanup_sql(&self, sql: &str) -> Result<(), String> {
            self.retry_cleanup(|connection| execute_update_sql(connection, sql))
        }

        fn drop_cleanup_table(&self, relation: &str) -> Result<(), String> {
            self.retry_cleanup(|connection| {
                if table_exists(connection, relation)? {
                    execute_update_sql(connection, &format!("DROP TABLE {relation}"))?;
                }
                Ok(())
            })
        }

        fn retry_cleanup(
            &self,
            operation: impl Fn(
                &mut crate::backend::storage::repository::RepositoryConnection,
            ) -> Result<(), String>,
        ) -> Result<(), String> {
            const MAX_ATTEMPTS: usize = 32;
            for attempt in 1..=MAX_ATTEMPTS {
                match self.repository.run_exclusive_blocking(|connection| {
                    let result = operation(connection);
                    if result.as_ref().is_err_and(|error| {
                        error.contains("project namespace compare-and-swap conflict")
                    }) {
                        let _ = connection.rollback();
                    }
                    result
                }) {
                    Ok(()) => return Ok(()),
                    Err(error)
                        if error.contains("project namespace compare-and-swap conflict")
                            && attempt < MAX_ATTEMPTS =>
                    {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(error) => return Err(error),
                }
            }
            unreachable!("the bounded cleanup loop returns on its final attempt")
        }

        fn assert_absent(&self) -> Result<(), String> {
            let namespace = self.namespace.clone();
            let relations = self.relations.clone();
            let project = self.repository.project_id();
            self.repository.run_exclusive_blocking(move |connection| {
                let leaked = connection
                    .list_catalog_relations(&project)?
                    .into_iter()
                    .filter(|entry| {
                        entry.name == namespace
                            || entry.name.starts_with(&format!("{namespace}."))
                            || relations.contains(&entry.name)
                    })
                    .map(|entry| entry.name)
                    .collect::<Vec<_>>();
                if leaked.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "Workflow test leaked catalog resources: {}",
                        leaked.join(", ")
                    ))
                }
            })
        }
    }

    impl Drop for WorkflowTestNamespace {
        fn drop(&mut self) {
            let _ = self.cleanup_now();
        }
    }
}
