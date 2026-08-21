//! Persisted Table, Model, Query, and Result execution over Spiral relations.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use spiral_robotics_contracts::api::SpiralQueryResponse;
use spiral_robotics_contracts::workflow::{
    RunWorkflowRequest, SaveWorkflowRequest, WorkflowCapabilities, WorkflowDocument,
    WorkflowNodeConfig, WorkflowNodePhase, WorkflowNodeSnapshot, WorkflowQuarantineSummary,
    WorkflowRunPhase, WorkflowRunSnapshot, WorkflowSpec, WorkflowStateResponse, WorkflowUpdate,
    derived_model_output_relation, resolve_siglip_concepts, topological_order,
    validate_workflow_spec,
};
use spiral_robotics_core::workflow::substitute_node_placeholders;
use spiral_robotics_studio_application::error::ApplicationError;
use spiral_robotics_studio_application::workflow::{admit_cancel, admit_run, admit_save};
use tokio::sync::broadcast;

use crate::backend::storage::manager::StorageManager;
use crate::backend::workflow::model::{
    MODEL_CLEANUP_FAILURE_PREFIX, ModelRunEvent, ModelRunRequest, ModelRunResult, run_model,
};

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct WorkflowManager {
    inner: Arc<WorkflowManagerInner>,
}

struct WorkflowManagerInner {
    storage: Arc<StorageManager>,
    document: Mutex<WorkflowDocument>,
    updates: broadcast::Sender<WorkflowUpdate>,
    active: Mutex<Option<ActiveRun>>,
}

struct ActiveRun {
    run_id: String,
    cancel: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl WorkflowManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        let (updates, _) = broadcast::channel(128);
        Self {
            inner: Arc::new(WorkflowManagerInner {
                storage,
                document: Mutex::new(WorkflowDocument::default()),
                updates,
                active: Mutex::new(None),
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
        let update = WorkflowUpdate {
            state: state_response(&document),
        };
        *lock(&self.inner.document) = document;
        let _ = self.inner.updates.send(update);
        Ok(())
    }

    pub(crate) fn state(&self) -> WorkflowStateResponse {
        let document = lock(&self.inner.document);
        state_response(&document)
    }

    pub(crate) fn is_active(&self) -> bool {
        lock(&self.inner.document).snapshot.phase.active()
    }

    pub(crate) fn capabilities(&self) -> WorkflowCapabilities {
        WorkflowCapabilities::default()
    }

    pub(crate) fn subscription(
        &self,
    ) -> (WorkflowStateResponse, broadcast::Receiver<WorkflowUpdate>) {
        let document = lock(&self.inner.document);
        let updates = self.inner.updates.subscribe();
        (state_response(&document), updates)
    }

    pub(crate) fn save(
        &self,
        request: SaveWorkflowRequest,
    ) -> Result<WorkflowStateResponse, ApplicationError> {
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
        if !validation.valid() {
            let issue = validation
                .errors
                .first()
                .map(|issue| format!("{} {}", issue.message, issue.recovery))
                .unwrap_or_else(|| "Workflow validation failed.".to_owned());
            return Err(ApplicationError::InvalidInput(issue));
        }

        let changed = workflow_content_changed(&current.workflow, &workflow);
        let removed_model_ids = removed_model_ids(&current.workflow, &workflow);
        let mut snapshot = current.snapshot.clone();
        snapshot.workflow_revision = workflow.revision;
        snapshot.updated_ms = now_millis();
        if changed {
            let had_result = snapshot.result.is_some();
            snapshot.phase = WorkflowRunPhase::Idle;
            snapshot.result_stale = had_result;
            snapshot.error = None;
            snapshot.nodes = saved_graph_node_snapshots(&current.snapshot, &workflow, had_result);
        }
        let candidate = WorkflowDocument { workflow, snapshot };
        self.inner
            .storage
            .save_workflow_document_and_drop_models(&candidate, &removed_model_ids)
            .map_err(ApplicationError::External)?;
        *current = candidate;
        let response = state_response(&current);
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: response.clone(),
        });
        Ok(response)
    }

    pub(crate) fn start(
        &self,
        request: RunWorkflowRequest,
    ) -> Result<WorkflowRunSnapshot, ApplicationError> {
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
            task: None,
        });
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: state_response(&document),
        });
        drop(document);
        drop(active);

        let manager = self.clone();
        let task_run_id = run_id.clone();
        let task = tokio::spawn(async move {
            manager.execute(run_id, cancel).await;
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
                run.cancel.store(true, Ordering::SeqCst);
                run.task.take()
            })
        };
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    pub(crate) fn cancel(&self) -> Result<WorkflowRunSnapshot, ApplicationError> {
        // Keep admission and Cancelling publication in one active -> document
        // critical section. Otherwise execution could publish Cancelled and
        // clear `active` between these steps, only for this request to regress
        // the persisted snapshot back to Cancelling.
        let active = lock(&self.inner.active);
        admit_cancel(active.is_some())?;
        let control = active.as_ref().expect("admission requires an active run");
        control.cancel.store(true, Ordering::SeqCst);
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(control.run_id.as_str()) {
            return Err(ApplicationError::Conflict(
                "The active Workflow run changed before cancellation.".to_owned(),
            ));
        }
        document.snapshot.phase = WorkflowRunPhase::Cancelling;
        document.snapshot.updated_ms = now_millis();
        for node in &mut document.snapshot.nodes {
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
        self.inner
            .storage
            .save_workflow_document(&document)
            .map_err(ApplicationError::External)?;
        let snapshot = document.snapshot.clone();
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: state_response(&document),
        });
        Ok(snapshot)
    }

    async fn execute(&self, run_id: String, cancel: Arc<AtomicBool>) {
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
        let mut relation_outputs = BTreeMap::new();
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
                    relation_outputs.insert(node.id.clone(), relation.clone());
                    self.complete_node(
                        &run_id,
                        &node.id,
                        node_started,
                        format!("Resolved {field_count} fields"),
                        None,
                    );
                }
                WorkflowNodeConfig::Query { sql, max_rows } => {
                    let substituted = match substitute_node_placeholders(sql, &relation_outputs) {
                        Ok(sql) => sql,
                        Err(error) => {
                            self.finish_failed(&run_id, Some(&node.id), error.to_string());
                            return;
                        }
                    };
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
                    media_column,
                    concept_columns,
                    output_relation,
                    ..
                } => {
                    let source = workflow
                        .edges
                        .iter()
                        .find(|edge| edge.target == node.id)
                        .map(|edge| edge.source.as_str());
                    let Some(input_relation) = source
                        .and_then(|source| relation_outputs.get(source))
                        .cloned()
                    else {
                        self.finish_failed(
                            &run_id,
                            Some(&node.id),
                            "Model has no resolved upstream Table relation.".to_owned(),
                        );
                        return;
                    };
                    let Some(output_relation) = output_relation.clone() else {
                        self.finish_failed(
                            &run_id,
                            Some(&node.id),
                            "Model output relation was not derived by the server. Apply the workflow again."
                                .to_owned(),
                        );
                        return;
                    };
                    let (previous, previous_quarantine) = self.model_previous(&run_id, &node.id);
                    let request = ModelRunRequest {
                        input_relation,
                        media_column: media_column.clone(),
                        output_relation: output_relation.clone(),
                        concept_columns: concept_columns.clone(),
                        previous,
                        previous_quarantine,
                    };
                    let repository = self.inner.storage.workflow_model_repository();
                    let event_manager = self.clone();
                    let event_run_id = run_id.clone();
                    let event_node_id = node.id.clone();
                    let worker_cancel = cancel.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        run_model(repository, request, worker_cancel, |event| {
                            event_manager.set_model_event(&event_run_id, &event_node_id, event);
                        })
                    })
                    .await;
                    let result = match result {
                        Ok(Ok(result)) => result,
                        Ok(Err(error)) => {
                            self.finish_failed(&run_id, Some(&node.id), error);
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
                    if self.cancelled(&run_id, &cancel) {
                        self.finish_cancelled(&run_id);
                        return;
                    }
                    relation_outputs.insert(node.id.clone(), output_relation);
                    self.complete_model_node(&run_id, &node.id, node_started, result);
                }
            }
        }
        self.finish_completed(&run_id, terminal_result);
    }

    fn model_previous(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> (
        Option<spiral_robotics_contracts::workflow::WorkflowModelRunMetadata>,
        WorkflowQuarantineSummary,
    ) {
        let document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id) {
            return (None, WorkflowQuarantineSummary::default());
        }
        document
            .snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .map_or_else(
                || (None, WorkflowQuarantineSummary::default()),
                |node| (node.model.clone(), node.quarantine.clone()),
            )
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
                state: state_response(&document),
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
                format!("Reused derived relation · {} rows", result.rows)
            } else if has_warnings {
                format!("Complete with warnings · {} rows", result.rows)
            } else {
                format!("Scored {} rows", result.rows)
            };
            node.elapsed_ms = now_millis().saturating_sub(started_ms);
            node.rows = Some(result.rows);
            node.error = None;
            node.progress = Some(spiral_robotics_contracts::workflow::WorkflowNodeProgress {
                completed: result.metadata.input_rows,
                total: result.metadata.input_rows,
                unit: "rows".to_owned(),
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
        let Some(control) = active.as_ref().filter(|active| active.run_id == run_id) else {
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
        let finished = now_millis();
        document.snapshot.phase = WorkflowRunPhase::Completed;
        document.snapshot.finished_ms = finished;
        document.snapshot.updated_ms = finished;
        document.snapshot.error = None;
        document.snapshot.result = result;
        document.snapshot.result_stale = false;
        self.persist_publish(&document);
        drop(document);
        *active = None;
    }

    fn finish_failed(&self, run_id: &str, node_id: Option<&str>, error: String) {
        let mut active = lock(&self.inner.active);
        let Some(control) = active.as_ref().filter(|active| active.run_id == run_id) else {
            return;
        };
        if control.cancel.load(Ordering::SeqCst) && !error.starts_with(MODEL_CLEANUP_FAILURE_PREFIX)
        {
            drop(active);
            self.finish_cancelled(run_id);
            return;
        }
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id) {
            return;
        }
        if let Some(node_id) = node_id {
            if let Some(node) = document
                .snapshot
                .nodes
                .iter_mut()
                .find(|node| node.node_id == node_id)
            {
                node.phase = WorkflowNodePhase::Failed;
                node.detail = "Failed".to_owned();
                node.error = Some(error.clone());
            }
        }
        let finished = now_millis();
        document.snapshot.phase = WorkflowRunPhase::Failed;
        document.snapshot.finished_ms = finished;
        document.snapshot.updated_ms = finished;
        document.snapshot.error = Some(error);
        self.persist_publish(&document);
        drop(document);
        *active = None;
    }

    fn finish_cancelled(&self, run_id: &str) {
        let mut active = lock(&self.inner.active);
        if active.as_ref().is_none_or(|active| active.run_id != run_id) {
            return;
        }
        let mut document = lock(&self.inner.document);
        if document.snapshot.run_id.as_deref() != Some(run_id) {
            return;
        }
        for node in &mut document.snapshot.nodes {
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
        let finished = now_millis();
        document.snapshot.phase = WorkflowRunPhase::Cancelled;
        document.snapshot.finished_ms = finished;
        document.snapshot.updated_ms = finished;
        document.snapshot.error = None;
        self.persist_publish(&document);
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
        if let Err(error) = self.inner.storage.save_workflow_document(document) {
            tracing::error!(%error, "persisting Workflow snapshot");
        }
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: state_response(document),
        });
    }
}

fn state_response(document: &WorkflowDocument) -> WorkflowStateResponse {
    WorkflowStateResponse {
        workflow: document.workflow.clone(),
        validation: validate_workflow_spec(&document.workflow),
        snapshot: document.snapshot.clone(),
    }
}

fn derive_model_outputs(workflow: &mut WorkflowSpec) {
    for node in &mut workflow.nodes {
        if let WorkflowNodeConfig::Model {
            concepts,
            concept_columns,
            output_relation,
            ..
        } = &mut node.config
        {
            *concept_columns = resolve_siglip_concepts(concepts);
            *output_relation = Some(derived_model_output_relation(&workflow.id, &node.id));
        }
    }
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

fn removed_model_ids(current: &WorkflowSpec, next: &WorkflowSpec) -> Vec<String> {
    let retained = next
        .nodes
        .iter()
        .filter(|node| matches!(node.config, WorkflowNodeConfig::Model { .. }))
        .map(|node| node.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    current
        .nodes
        .iter()
        .filter(|node| {
            matches!(node.config, WorkflowNodeConfig::Model { .. })
                && !retained.contains(node.id.as_str())
        })
        .map(|node| node.id.clone())
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use arrow_array::{ArrayRef, Int64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use spiral_robotics_contracts::api::SpiralQueryResponse;
    use spiral_robotics_contracts::workflow::{
        WorkflowEdgeSpec, WorkflowModelRunMetadata, WorkflowNodeConfig, WorkflowNodeSpec,
        WorkflowRunPhase,
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
    fn query_only_graph_edits_preserve_model_fingerprint_for_safe_reuse() {
        let mut workflow = siglip_video_workflow("media.manifest", "model");
        derive_model_outputs(&mut workflow);
        let mut previous = WorkflowRunSnapshot::idle_for(&workflow);
        let metadata = WorkflowModelRunMetadata {
            model_id: "siglip-base".to_owned(),
            artifact_revision: "revision".to_owned(),
            output_relation: derived_model_output_relation("default", "model"),
            concept_columns: Vec::new(),
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
    fn removed_model_cleanup_uses_node_identity_not_persisted_relation_text() {
        let mut current = siglip_video_workflow("media.manifest", "model");
        let WorkflowNodeConfig::Model {
            output_relation, ..
        } = &mut current
            .nodes
            .iter_mut()
            .find(|node| node.id == "model")
            .unwrap()
            .config
        else {
            unreachable!()
        };
        *output_relation = Some("training.episodes".to_owned());
        let next = WorkflowSpec::default();
        assert_eq!(removed_model_ids(&current, &next), vec!["model"]);
        assert_eq!(
            derived_model_output_relation(&current.id, "model"),
            "workflow.default_model"
        );
    }

    #[test]
    fn model_outputs_and_score_columns_are_server_derived_without_aliasing_ids() {
        let mut workflow = WorkflowSpec {
            id: "pick-place".to_owned(),
            nodes: vec![spiral_robotics_contracts::workflow::WorkflowNodeSpec {
                id: "score-node".to_owned(),
                name: "Score frames".to_owned(),
                config: WorkflowNodeConfig::Model {
                    model_id: "siglip-base".to_owned(),
                    configuration: BTreeMap::new(),
                    media_column: "image".to_owned(),
                    concepts: vec!["Red cube".to_owned(), "red-cube".to_owned()],
                    concept_columns: Vec::new(),
                    output_relation: Some("caller.chosen".to_owned()),
                },
            }],
            edges: Vec::new(),
            ..WorkflowSpec::default()
        };
        derive_model_outputs(&mut workflow);
        let WorkflowNodeConfig::Model {
            output_relation,
            concept_columns,
            ..
        } = &workflow.nodes[0].config
        else {
            unreachable!()
        };
        assert_eq!(
            output_relation.as_deref(),
            Some("workflow.pick_place_68af5e01b914d83e_score_node_fd576fe8b71daff4")
        );
        assert_eq!(concept_columns[0].score_column, "score_red_cube");
        assert_eq!(concept_columns[1].score_column, "score_red_cube_2");
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
                expected_revision: 0,
                workflow,
            })
            .expect("the valid SQL workflow must save");
        let request = RunWorkflowRequest {
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
            .start(request)
            .expect("the persisted workflow must run again");
        let cancelling = reloaded.cancel().expect("the active run must cancel");
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

    /// Live Stage-3 acceptance: a tiny deterministic MP4 is stored as a Spiral
    /// media relation, decoded in-engine, scored by the pinned SigLIP model,
    /// queried downstream, reused, reloaded, zero-row replaced, and cancelled.
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
        derive_model_outputs(&mut workflow);
        let output_relation = workflow
            .nodes
            .iter()
            .find_map(|node| match &node.config {
                WorkflowNodeConfig::Model {
                    output_relation: Some(relation),
                    ..
                } => Some(relation.clone()),
                _ => None,
            })
            .expect("the server derivation must produce a Model output relation");
        // Install cleanup before the first catalog mutation so a seeding or
        // Dataset-build failure cannot strand a test namespace.
        let mut cleanup = WorkflowTestNamespace::new(
            repository.clone(),
            vec![
                manifest_relation.clone(),
                empty_relation.clone(),
                format!("{namespace}.workflow_runs"),
                output_relation.clone(),
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
                expected_revision: 0,
                workflow,
            })
            .expect("the SigLIP video workflow must save");

        let request = RunWorkflowRequest {
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
            for column in ["score_black_square", "score_white_square"] {
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

        let empty_saved = reloaded
            .save(SaveWorkflowRequest {
                expected_revision: saved.workflow.revision,
                workflow: siglip_video_workflow(&empty_relation, &model_node_id),
            })
            .expect("the empty manifest workflow must save");
        reloaded
            .start(RunWorkflowRequest {
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
        assert_eq!(empty_metadata.device, "not loaded (empty input)");

        let changed_saved = reloaded
            .save(SaveWorkflowRequest {
                expected_revision: empty_saved.workflow.revision,
                workflow: siglip_video_workflow(&manifest_relation, &model_node_id),
            })
            .expect("restoring the media workflow must save");
        reloaded
            .start(RunWorkflowRequest {
                revision: changed_saved.workflow.revision,
            })
            .expect("the cancellable model run must start");
        let cancelling = reloaded.cancel().expect("the active Model run must cancel");
        assert_eq!(cancelling.phase, WorkflowRunPhase::Cancelling);
        assert_eq!(
            wait_for_model_terminal(&reloaded).await.phase,
            WorkflowRunPhase::Cancelled
        );

        // Simulate a hostile legacy document whose caller-owned output text
        // points at the input manifest. Removal must recompute the one owned
        // Workflow relation from workflow/node identity and leave the manifest
        // untouched. A subsequent ordinary save/read proves autocommit was
        // restored after the atomic cleanup transaction.
        {
            let mut document = lock(&reloaded.inner.document);
            let WorkflowNodeConfig::Model {
                output_relation: stored_output,
                ..
            } = &mut document
                .workflow
                .nodes
                .iter_mut()
                .find(|node| node.id == model_node_id)
                .expect("the test workflow retains its Model")
                .config
            else {
                unreachable!()
            };
            *stored_output = Some(manifest_relation.clone());
        }
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
                expected_revision: without_model.revision,
                workflow: without_model,
            })
            .expect("removing the Model must atomically save and clean its output");
        let manifest_check = manifest_relation.clone();
        let output_check = output_relation.clone();
        repository
            .run_exclusive_blocking(move |connection| {
                if !table_exists(connection, &manifest_check)? {
                    return Err("hostile stored output text dropped the input manifest".to_owned());
                }
                if table_exists(connection, &output_check)? {
                    return Err("removed Model output relation still exists".to_owned());
                }
                Ok(())
            })
            .expect("Model removal must preserve unrelated tables and drop only its output");
        let mut renamed = removed.workflow;
        renamed.name = "Post-cleanup write".to_owned();
        let renamed = reloaded
            .save(SaveWorkflowRequest {
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
                    id: model_node_id.to_owned(),
                    name: "Score video frames".to_owned(),
                    config: WorkflowNodeConfig::Model {
                        model_id: "siglip-base".to_owned(),
                        configuration: BTreeMap::new(),
                        media_column: "media_relation".to_owned(),
                        concepts: vec!["black square".to_owned(), "white square".to_owned()],
                        concept_columns: Vec::new(),
                        output_relation: None,
                    },
                },
                WorkflowNodeSpec {
                    id: "query".to_owned(),
                    name: "Inspect scores".to_owned(),
                    config: WorkflowNodeConfig::Query {
                        sql: format!(
                            "SELECT * FROM {{{{node:{model_node_id}}}}} ORDER BY frame_index"
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
                    target: model_node_id.to_owned(),
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
