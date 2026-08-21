//! Persisted SQL-only Workflow execution over existing Spiral relations.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use spiral_robotics_contracts::api::SpiralQueryResponse;
use spiral_robotics_contracts::workflow::{
    RunWorkflowRequest, SaveWorkflowRequest, WorkflowCapabilities, WorkflowDocument,
    WorkflowNodeConfig, WorkflowNodePhase, WorkflowNodeSnapshot, WorkflowRunPhase,
    WorkflowRunSnapshot, WorkflowSpec, WorkflowStateResponse, WorkflowUpdate, topological_order,
    validate_workflow_spec, workflow_has_model,
};
use spiral_robotics_core::workflow::substitute_node_placeholders;
use spiral_robotics_studio_application::error::ApplicationError;
use spiral_robotics_studio_application::workflow::{admit_cancel, admit_run, admit_save};
use tokio::sync::broadcast;

use crate::backend::storage::manager::StorageManager;

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
        let mut document = tokio::task::spawn_blocking(move || storage.load_workflow_document())
            .await
            .map_err(|error| format!("Workflow history task stopped: {error}"))??;
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
                        | WorkflowNodePhase::Running
                        | WorkflowNodePhase::Cancelling
                ) {
                    node.phase = WorkflowNodePhase::Failed;
                    node.detail = "Interrupted by Studio restart".to_owned();
                    node.error = document.snapshot.error.clone();
                }
            }
            let storage = self.inner.storage.clone();
            let recovered = document.clone();
            tokio::task::spawn_blocking(move || storage.save_workflow_document(&recovered))
                .await
                .map_err(|error| format!("Workflow recovery task stopped: {error}"))??;
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
        let mut snapshot = current.snapshot.clone();
        snapshot.workflow_revision = workflow.revision;
        snapshot.updated_ms = now_millis();
        if changed {
            let had_result = snapshot.result.is_some();
            snapshot.phase = WorkflowRunPhase::Idle;
            snapshot.result_stale = had_result;
            snapshot.error = None;
            snapshot.nodes = workflow
                .nodes
                .iter()
                .map(|node| WorkflowNodeSnapshot {
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
                    elapsed_ms: 0,
                    rows: None,
                    error: None,
                })
                .collect();
        }
        let candidate = WorkflowDocument { workflow, snapshot };
        self.inner
            .storage
            .save_workflow_document(&candidate)
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
            workflow_has_model(&document.workflow),
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
        });
        let _ = self.inner.updates.send(WorkflowUpdate {
            state: state_response(&document),
        });
        drop(document);
        drop(active);

        let manager = self.clone();
        tokio::spawn(async move {
            manager.execute(run_id, cancel).await;
        });
        Ok(snapshot)
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
                WorkflowNodePhase::Validating | WorkflowNodePhase::Running
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
                WorkflowNodeConfig::Model { .. } => {
                    self.finish_failed(
                        &run_id,
                        Some(&node.id),
                        spiral_robotics_studio_application::workflow::MODEL_STAGE_THREE_RECOVERY
                            .to_owned(),
                    );
                    return;
                }
            }
        }
        self.finish_completed(&run_id, terminal_result);
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
        if control.cancel.load(Ordering::SeqCst) {
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
    let workflow_id = workflow.id.replace('-', "_");
    for node in &mut workflow.nodes {
        if let WorkflowNodeConfig::Model {
            output_relation, ..
        } = &mut node.config
        {
            *output_relation = Some(format!(
                "workflow.{workflow_id}_{}",
                node.id.replace('-', "_")
            ));
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
            .map(|node| WorkflowNodeSnapshot {
                node_id: node.id.clone(),
                phase: WorkflowNodePhase::Pending,
                detail: "Pending".to_owned(),
                elapsed_ms: 0,
                rows: None,
                error: None,
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
    use spiral_robotics_contracts::workflow::{WorkflowNodeConfig, WorkflowRunPhase};

    use crate::backend::storage::manager::{
        ensure_dataset_namespace, execute_update_sql, ingest_batch, table_exists,
    };
    use crate::backend::storage::repository::SpiralRepository;
    use crate::backend::storage::settings::{
        BackendStateStore, environment_spiral_endpoint, environment_spiral_spfs_endpoint,
    };

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
        let _cleanup = WorkflowTestNamespace {
            repository: repository.clone(),
            relations: vec![seed_relation.clone(), format!("{namespace}.workflow_runs")],
            namespace: namespace.clone(),
        };
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
    }

    impl Drop for WorkflowTestNamespace {
        fn drop(&mut self) {
            let relations = self.relations.clone();
            let namespace = self.namespace.clone();
            let _ = self.repository.run_exclusive_blocking(move |connection| {
                for relation in relations {
                    if table_exists(connection, &relation)? {
                        execute_update_sql(connection, &format!("DROP TABLE {relation}"))?;
                    }
                }
                if table_exists(connection, &namespace)? {
                    execute_update_sql(connection, &format!("DROP TABLE {namespace}"))?;
                }
                Ok(())
            });
        }
    }
}
