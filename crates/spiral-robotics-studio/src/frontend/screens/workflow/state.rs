//! Shell-owned Workflow editor state and backend commands.
//!
//! Canvas geometry and inspector selection are browser interaction state. The
//! saved graph, validation, execution phases, and result payload are backend
//! truth and reconcile through HTTP plus the one shared SSE stream.

use std::collections::{BTreeMap, HashMap};

use dioxus::prelude::*;
use dioxus_flow::{Edge, EdgeKind, Node, Side};
use gloo_net::http::Method;
use spiral_robotics_contracts::workflow::{
    RunWorkflowRequest, SIGLIP_BASE_MODEL_ID, SaveWorkflowRequest, WorkflowCapabilities,
    WorkflowConceptColumn, WorkflowEdgeSpec, WorkflowModelRunMetadata, WorkflowNodeConfig,
    WorkflowNodeKind, WorkflowNodePhase, WorkflowNodeProgress, WorkflowNodeSnapshot,
    WorkflowQuarantineSummary, WorkflowRelation, WorkflowRelationsResponse, WorkflowRunPhase,
    WorkflowRunSnapshot, WorkflowSpec, WorkflowStateResponse, WorkflowValidationReport,
    resolve_siglip_concepts, validate_workflow_spec, workflow_has_model,
};
use spiral_robotics_panels::PanelId;

use crate::frontend::api::http as api;
use crate::frontend::app::notifications::MessageSource;
use crate::frontend::app::state::{Studio, studio};

pub(crate) const ASSISTANT_PANEL_ID: &str = "workflow-assistant";
pub(crate) const NODE_PANEL_ID: &str = "workflow-node";
pub(crate) const CANVAS_PANEL_ID: &str = "workflow-canvas";
pub(crate) const RESULTS_PANEL_ID: &str = "workflow-results";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WorkflowNodeData {
    pub kind: WorkflowNodeKind,
    pub fields: Vec<String>,
    pub relation: Option<String>,
    pub output_relation: Option<String>,
    pub sql: Option<String>,
    pub description: String,
    pub phase: WorkflowNodePhase,
    pub detail: String,
    pub error: Option<String>,
    pub model_id: Option<String>,
    pub media_column: Option<String>,
    pub concepts: Vec<String>,
    pub concept_columns: Vec<WorkflowConceptColumn>,
    pub progress: Option<WorkflowNodeProgress>,
    pub quarantine: WorkflowQuarantineSummary,
    pub model: Option<WorkflowModelRunMetadata>,
    pub warnings: Vec<String>,
}

impl WorkflowNodeData {
    pub(crate) const fn kind_label(&self) -> &'static str {
        match self.kind {
            WorkflowNodeKind::Table => "Table",
            WorkflowNodeKind::Model => "Model",
            WorkflowNodeKind::Query => "Query",
            WorkflowNodeKind::Result => "Result",
        }
    }

    pub(crate) const fn kind_class(&self) -> &'static str {
        match self.kind {
            WorkflowNodeKind::Table => "workflow-node-kind-table",
            WorkflowNodeKind::Model => "workflow-node-kind-model",
            WorkflowNodeKind::Query => "workflow-node-kind-query",
            WorkflowNodeKind::Result => "workflow-node-kind-result",
        }
    }

    pub(crate) const fn phase_class(&self) -> &'static str {
        match self.phase {
            WorkflowNodePhase::Complete => "workflow-node-state-complete",
            WorkflowNodePhase::CompleteWithWarnings => "workflow-node-state-warning",
            WorkflowNodePhase::Failed | WorkflowNodePhase::Unsupported => {
                "workflow-node-state-failed"
            }
            WorkflowNodePhase::Running
            | WorkflowNodePhase::Validating
            | WorkflowNodePhase::Loading
            | WorkflowNodePhase::Cancelling => "workflow-node-state-running",
            WorkflowNodePhase::Stale => "workflow-node-state-stale",
            WorkflowNodePhase::Cancelled => "workflow-node-state-cancelled",
            WorkflowNodePhase::Pending => "workflow-node-state-idle",
        }
    }

    pub(crate) const fn phase_label(&self) -> &'static str {
        match self.phase {
            WorkflowNodePhase::Pending => "Pending",
            WorkflowNodePhase::Validating => "Validating",
            WorkflowNodePhase::Loading => "Loading",
            WorkflowNodePhase::Running => "Running",
            WorkflowNodePhase::Complete => "Complete",
            WorkflowNodePhase::CompleteWithWarnings => "Complete with warnings",
            WorkflowNodePhase::Failed => "Failed",
            WorkflowNodePhase::Stale => "Stale",
            WorkflowNodePhase::Cancelling => "Cancelling",
            WorkflowNodePhase::Cancelled => "Cancelled",
            WorkflowNodePhase::Unsupported => "Unavailable",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WorkflowUiState {
    pub selected_node: Option<String>,
    pub active_panel: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowUiAction {
    SelectNode(String),
    ClearSelection,
    ActivatePanel(String),
}

pub(crate) fn reduce_workflow_ui(
    state: &WorkflowUiState,
    action: WorkflowUiAction,
) -> WorkflowUiState {
    let mut next = state.clone();
    match action {
        WorkflowUiAction::SelectNode(id) => {
            next.selected_node = Some(id);
            next.active_panel = Some(NODE_PANEL_ID.to_owned());
        }
        WorkflowUiAction::ClearSelection => next.selected_node = None,
        WorkflowUiAction::ActivatePanel(id) => next.active_panel = Some(id),
    }
    next
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowCommand {
    Applying,
    Starting,
    Cancelling,
}

#[derive(Clone, Copy)]
pub(crate) struct WorkflowController {
    nodes: Signal<Vec<Node<WorkflowNodeData>>>,
    edges: Signal<Vec<Edge>>,
    ui: Signal<WorkflowUiState>,
    server: Signal<WorkflowStateResponse>,
    draft: Signal<WorkflowSpec>,
    relations: Signal<WorkflowRelationsResponse>,
    capabilities: Signal<WorkflowCapabilities>,
    loaded: Signal<bool>,
    command: Signal<Option<WorkflowCommand>>,
    load_error: Signal<Option<String>>,
}

pub(crate) fn use_workflow_controller() -> WorkflowController {
    let initial = initial_state();
    let initial_spec = initial.workflow.clone();
    let initial_nodes = graph_nodes(&initial_spec, &initial.snapshot, &[]);
    let initial_edges = graph_edges(&initial_spec.edges);
    let controller = WorkflowController {
        nodes: use_signal(move || initial_nodes),
        edges: use_signal(move || initial_edges),
        ui: use_signal(WorkflowUiState::default),
        server: use_signal(move || initial),
        draft: use_signal(move || initial_spec),
        relations: use_signal(WorkflowRelationsResponse::default),
        capabilities: use_signal(WorkflowCapabilities::default),
        loaded: use_signal(|| false),
        command: use_signal(|| None),
        load_error: use_signal(|| None),
    };
    let studio = studio();
    use_future(move || async move {
        controller.load(studio).await;
    });
    use_effect(move || {
        if let Some(state) = studio.workflow_state.read().clone() {
            controller.reconcile_state(state);
        }
    });
    use_context_provider(|| controller)
}

pub(crate) fn workflow_controller() -> WorkflowController {
    use_context::<WorkflowController>()
}

impl WorkflowController {
    pub(crate) const fn nodes(self) -> Signal<Vec<Node<WorkflowNodeData>>> {
        self.nodes
    }

    pub(crate) const fn edges(self) -> Signal<Vec<Edge>> {
        self.edges
    }

    pub(crate) fn spec(&self) -> WorkflowSpec {
        self.draft.read().clone()
    }

    pub(crate) fn snapshot(&self) -> WorkflowRunSnapshot {
        self.server.read().snapshot.clone()
    }

    pub(crate) fn relations(&self) -> Vec<WorkflowRelation> {
        self.relations.read().relations.clone()
    }

    pub(crate) fn relation_limitation(&self) -> Option<String> {
        self.relations.read().limitation.clone()
    }

    pub(crate) fn capabilities(&self) -> WorkflowCapabilities {
        self.capabilities.read().clone()
    }

    pub(crate) fn has_model(&self) -> bool {
        workflow_has_model(&self.draft.read())
    }

    pub(crate) fn topology_signature(&self) -> String {
        let workflow = self.draft.read();
        let mut signature = workflow
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>()
            .join("|");
        for edge in &workflow.edges {
            signature.push('|');
            signature.push_str(&edge.source);
            signature.push('>');
            signature.push_str(&edge.target);
        }
        signature
    }

    pub(crate) fn model_media_fields(&self, node_id: &str) -> Vec<String> {
        let draft = self.draft.read();
        let upstream_id = draft
            .edges
            .iter()
            .find(|edge| edge.target == node_id)
            .map(|edge| edge.source.as_str());
        let relation = upstream_id.and_then(|upstream_id| {
            draft.nodes.iter().find_map(|node| {
                (node.id == upstream_id)
                    .then_some(&node.config)
                    .and_then(|config| match config {
                        WorkflowNodeConfig::Table { relation } => Some(relation.as_str()),
                        _ => None,
                    })
            })
        });
        let relations = self.relations.read();
        relations
            .relations
            .iter()
            .find(|candidate| Some(candidate.name.as_str()) == relation)
            .map(relation_model_media_fields)
            .unwrap_or_default()
    }

    pub(crate) fn validation(&self) -> WorkflowValidationReport {
        validate_workflow_spec(&self.draft.read())
    }

    pub(crate) fn loaded(&self) -> bool {
        *self.loaded.read()
    }

    pub(crate) fn load_error(&self) -> Option<String> {
        self.load_error.read().clone()
    }

    pub(crate) fn command(&self) -> Option<WorkflowCommand> {
        *self.command.read()
    }

    pub(crate) fn dirty(&self) -> bool {
        *self.draft.read() != self.server.read().workflow
    }

    pub(crate) fn revision_conflict(&self) -> bool {
        revision_conflict(&self.draft.read(), &self.server.read().workflow)
    }

    pub(crate) fn selected_node(&self) -> Option<String> {
        self.ui.read().selected_node.clone()
    }

    pub(crate) fn selected_node_data(&self) -> Option<(String, String, WorkflowNodeData)> {
        let selected = self.selected_node()?;
        self.nodes
            .read()
            .iter()
            .find(|node| node.id == selected)
            .map(|node| (node.id.clone(), node.label.clone(), node.data.clone()))
    }

    pub(crate) fn active_panel(&self) -> Option<PanelId> {
        self.ui.read().active_panel.clone().map(PanelId::from)
    }

    pub(crate) fn run_disabled_reason(&self) -> Option<String> {
        if let Some(error) = self.load_error() {
            return Some(format!("Could not load the saved Workflow: {error}"));
        }
        if !self.loaded() {
            return Some("Loading the saved Workflow…".to_owned());
        }
        if self.command().is_some() {
            return Some("A Workflow command is in progress.".to_owned());
        }
        if self.snapshot().phase.active() {
            return Some("This Workflow is already running.".to_owned());
        }
        if self.revision_conflict() {
            return Some(
                "The saved Workflow changed in another session. Reload the saved graph before editing again."
                    .to_owned(),
            );
        }
        if self.dirty() {
            return Some("Apply graph changes before running.".to_owned());
        }
        self.validation()
            .errors
            .first()
            .map(|issue| issue.message.clone())
    }

    pub(crate) fn apply_disabled_reason(&self) -> Option<String> {
        if let Some(error) = self.load_error() {
            return Some(format!("Could not load the saved Workflow: {error}"));
        }
        if !self.dirty() {
            return Some("No unapplied graph changes.".to_owned());
        }
        if self.revision_conflict() {
            return Some(
                "The saved Workflow changed in another session. Reload the saved graph before editing again."
                    .to_owned(),
            );
        }
        if self.command().is_some() || self.snapshot().phase.active() {
            return Some("Cancel the active run before applying changes.".to_owned());
        }
        self.validation()
            .errors
            .first()
            .map(|issue| issue.message.clone())
    }

    pub(crate) fn select_node(mut self, id: String) {
        if !self.nodes.peek().iter().any(|node| node.id == id) {
            return;
        }
        let next = reduce_workflow_ui(&self.ui.peek(), WorkflowUiAction::SelectNode(id.clone()));
        self.ui.set(next);
        self.nodes.with_mut(|nodes| {
            for node in nodes {
                node.selected = node.id == id;
            }
        });
    }

    pub(crate) fn clear_selection(mut self) {
        let next = reduce_workflow_ui(&self.ui.peek(), WorkflowUiAction::ClearSelection);
        self.ui.set(next);
        self.nodes.with_mut(|nodes| {
            for node in nodes {
                node.selected = false;
            }
        });
    }

    pub(crate) fn activate_panel(mut self, id: PanelId) {
        let next = reduce_workflow_ui(
            &self.ui.peek(),
            WorkflowUiAction::ActivatePanel(id.to_string()),
        );
        self.ui.set(next);
    }

    pub(crate) fn reload_saved(mut self) {
        self.draft.set(self.server.peek().workflow.clone());
        self.load_error.set(None);
        self.sync_graph();
    }

    pub(crate) fn set_table_relation(mut self, node_id: &str, relation: String) {
        let changed = edit_node(&mut self.draft.write(), node_id, |config| {
            let WorkflowNodeConfig::Table { relation: current } = config else {
                return false;
            };
            if current == &relation {
                return false;
            }
            *current = relation;
            true
        });
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn set_query_sql(mut self, node_id: &str, sql: String) {
        let changed = edit_node(&mut self.draft.write(), node_id, |config| {
            let WorkflowNodeConfig::Query { sql: current, .. } = config else {
                return false;
            };
            if current == &sql {
                return false;
            }
            *current = sql;
            true
        });
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn set_model_media_column(mut self, node_id: &str, media_column: String) {
        let changed = edit_node(&mut self.draft.write(), node_id, |config| {
            let WorkflowNodeConfig::Model {
                media_column: current,
                ..
            } = config
            else {
                return false;
            };
            if current == &media_column {
                return false;
            }
            *current = media_column;
            true
        });
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn set_model_concepts(mut self, node_id: &str, value: String) {
        let concepts = value.lines().map(str::to_owned).collect::<Vec<_>>();
        let changed = edit_node(&mut self.draft.write(), node_id, |config| {
            let WorkflowNodeConfig::Model {
                concepts: current,
                concept_columns,
                ..
            } = config
            else {
                return false;
            };
            if current == &concepts {
                return false;
            }
            *current = concepts;
            concept_columns.clear();
            true
        });
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn add_siglip_model(mut self) {
        if self.has_model() || topology_mutation_blocked(self.snapshot().phase, self.command()) {
            return;
        }
        let relations = self.relations.peek().relations.clone();
        let Some(node_id) = insert_siglip_model(&mut self.draft.write(), &relations) else {
            return;
        };
        self.sync_graph();
        self.select_node(node_id);
    }

    pub(crate) fn remove_siglip_model(mut self, node_id: &str) {
        if topology_mutation_blocked(self.snapshot().phase, self.command()) {
            return;
        }
        let Some(query_id) = remove_siglip_model(&mut self.draft.write(), node_id) else {
            return;
        };
        self.sync_graph();
        self.select_node(query_id);
    }

    pub(crate) fn apply(mut self, studio: Studio) {
        if self.apply_disabled_reason().is_some() {
            return;
        }
        self.command.set(Some(WorkflowCommand::Applying));
        let request = SaveWorkflowRequest {
            expected_revision: self.draft.peek().revision,
            workflow: self.draft.peek().clone(),
        };
        dioxus::dioxus_core::spawn_forever(async move {
            let response = api::send_json::<_, WorkflowStateResponse>(
                Method::PUT,
                "/api/workflow",
                Some(&request),
            )
            .await;
            self.command.set(None);
            match response {
                Ok(response) => self.accept_applied_state(response),
                Err(error) => {
                    studio.report_from(MessageSource::Workflow, error);
                    if let Ok(state) = api::get::<WorkflowStateResponse>("/api/workflow").await {
                        self.reconcile_state(state);
                    }
                }
            }
        });
    }

    pub(crate) fn run(mut self, studio: Studio) {
        if let Some(reason) = self.run_disabled_reason() {
            studio.report_warning_from(MessageSource::Workflow, reason);
            return;
        }
        self.command.set(Some(WorkflowCommand::Starting));
        let request = RunWorkflowRequest {
            revision: self.server.peek().workflow.revision,
        };
        dioxus::dioxus_core::spawn_forever(async move {
            let response = api::send_json::<_, WorkflowRunSnapshot>(
                Method::POST,
                "/api/workflow/run",
                Some(&request),
            )
            .await;
            self.command.set(None);
            match response {
                Ok(snapshot) => self.reconcile_snapshot(snapshot),
                Err(error) => studio.report_from(MessageSource::Workflow, error),
            }
        });
    }

    pub(crate) fn cancel(mut self, studio: Studio) {
        if !self.snapshot().phase.active() || self.command().is_some() {
            return;
        }
        self.command.set(Some(WorkflowCommand::Cancelling));
        dioxus::dioxus_core::spawn_forever(async move {
            let response = api::send_json::<(), WorkflowRunSnapshot>(
                Method::POST,
                "/api/workflow/cancel",
                None,
            )
            .await;
            self.command.set(None);
            match response {
                Ok(snapshot) => self.reconcile_snapshot(snapshot),
                Err(error) => studio.report_from(MessageSource::Workflow, error),
            }
        });
    }

    async fn load(mut self, studio: Studio) {
        let state_loaded = match api::get::<WorkflowStateResponse>("/api/workflow").await {
            Ok(state) => {
                self.reconcile_state(state);
                true
            }
            Err(error) => {
                if !*self.loaded.peek() {
                    self.load_error.set(Some(error.clone()));
                }
                studio.report_from(MessageSource::Workflow, error);
                false
            }
        };
        match api::get::<WorkflowRelationsResponse>("/api/workflow/relations").await {
            Ok(relations) => {
                self.relations.set(relations);
                self.sync_graph();
            }
            Err(error) => {
                studio.report_from(MessageSource::Workflow, error);
            }
        }
        match api::get::<WorkflowCapabilities>("/api/workflow/capabilities").await {
            Ok(capabilities) => self.capabilities.set(capabilities),
            Err(error) => studio.report_from(MessageSource::Workflow, error),
        }
        if state_loaded {
            self.loaded.set(true);
        }
    }

    fn accept_applied_state(mut self, state: WorkflowStateResponse) {
        let current = self.server.peek().clone();
        if state.workflow.id != current.workflow.id
            || state.workflow.revision < current.workflow.revision
        {
            return;
        }
        if state.workflow.revision == current.workflow.revision {
            // The save event may win the race against the PUT response. In
            // that case acknowledge the graph without replacing a newer run
            // snapshot that arrived on SSE in the meantime.
            if state.workflow != current.workflow {
                return;
            }
            self.draft.set(current.workflow.clone());
            if snapshot_is_monotonic(&current.snapshot, &state.snapshot) {
                self.server.set(state);
            }
        } else {
            self.draft.set(state.workflow.clone());
            self.server.set(state);
        }
        self.load_error.set(None);
        self.sync_graph();
    }

    fn reconcile_snapshot(mut self, snapshot: WorkflowRunSnapshot) {
        if snapshot.workflow_id != self.server.peek().workflow.id
            || snapshot.workflow_revision != self.server.peek().workflow.revision
            || !snapshot_is_monotonic(&self.server.peek().snapshot, &snapshot)
        {
            return;
        }
        self.server.write().snapshot = snapshot;
        self.sync_graph();
    }

    fn reconcile_state(mut self, state: WorkflowStateResponse) {
        if !server_state_is_monotonic(&self.server.peek(), &state) {
            return;
        }
        // This path runs inside the shell's Workflow SSE effect. Keep draft
        // comparison untracked so the effect subscribes only to the incoming
        // backend signal instead of retriggering itself after writing server
        // state or graph projection.
        let keep_local_draft = *self.draft.peek() != self.server.peek().workflow;
        self.server.set(state.clone());
        if !keep_local_draft {
            self.draft.set(state.workflow);
        }
        self.loaded.set(true);
        self.load_error.set(None);
        self.sync_graph();
    }

    fn sync_graph(mut self) {
        let spec = self.draft.peek().clone();
        let snapshot = self.server.peek().snapshot.clone();
        let relations = self.relations.peek().relations.clone();
        let mut next = graph_nodes(&spec, &snapshot, &relations);
        let next_edges = graph_edges(&spec.edges);
        let previous = self
            .nodes
            .peek()
            .iter()
            .map(|node| (node.id.clone(), node.clone()))
            .collect::<HashMap<_, _>>();
        let previous_ids = previous.keys().collect::<std::collections::BTreeSet<_>>();
        let next_ids = next
            .iter()
            .map(|node| &node.id)
            .collect::<std::collections::BTreeSet<_>>();
        let previous_edges = self
            .edges
            .peek()
            .iter()
            .map(|edge| (edge.source.clone(), edge.target.clone()))
            .collect::<Vec<_>>();
        let projected_edges = next_edges
            .iter()
            .map(|edge| (edge.source.clone(), edge.target.clone()))
            .collect::<Vec<_>>();
        let topology_changed = previous_ids != next_ids || previous_edges != projected_edges;
        for node in &mut next {
            if let Some(existing) = previous.get(&node.id) {
                if !topology_changed {
                    node.position = existing.position;
                }
                node.size = existing.size;
                node.measured = existing.measured;
                node.selected = existing.selected;
            }
        }
        self.nodes.set(next);
        self.edges.set(next_edges);
    }
}

fn initial_state() -> WorkflowStateResponse {
    let workflow = WorkflowSpec::default();
    WorkflowStateResponse {
        validation: validate_workflow_spec(&workflow),
        snapshot: WorkflowRunSnapshot::idle_for(&workflow),
        workflow,
    }
}

fn server_state_is_monotonic(
    current: &WorkflowStateResponse,
    incoming: &WorkflowStateResponse,
) -> bool {
    if incoming.workflow.id != current.workflow.id
        || incoming.snapshot.workflow_id != incoming.workflow.id
        || incoming.snapshot.workflow_revision != incoming.workflow.revision
        || incoming.workflow.revision < current.workflow.revision
    {
        return false;
    }
    incoming.workflow.revision > current.workflow.revision
        || snapshot_is_monotonic(&current.snapshot, &incoming.snapshot)
}

fn snapshot_is_monotonic(current: &WorkflowRunSnapshot, incoming: &WorkflowRunSnapshot) -> bool {
    if incoming.workflow_id != current.workflow_id
        || incoming.workflow_revision != current.workflow_revision
    {
        return false;
    }

    if incoming.run_id != current.run_id {
        return match (&current.run_id, &incoming.run_id) {
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (Some(_), Some(_)) => incoming.started_ms > current.started_ms,
            (None, None) => true,
        };
    }
    if incoming.updated_ms != current.updated_ms {
        return incoming.updated_ms > current.updated_ms;
    }

    run_phase_order(incoming.phase) >= run_phase_order(current.phase)
}

const fn run_phase_order(phase: WorkflowRunPhase) -> u8 {
    match phase {
        WorkflowRunPhase::Idle => 0,
        WorkflowRunPhase::Validating => 1,
        WorkflowRunPhase::Running => 2,
        WorkflowRunPhase::Cancelling => 3,
        WorkflowRunPhase::Completed
        | WorkflowRunPhase::Failed
        | WorkflowRunPhase::Cancelled
        | WorkflowRunPhase::Interrupted => 4,
    }
}

fn edit_node(
    workflow: &mut WorkflowSpec,
    node_id: &str,
    edit: impl FnOnce(&mut WorkflowNodeConfig) -> bool,
) -> bool {
    workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == node_id)
        .is_some_and(|node| edit(&mut node.config))
}

fn insert_siglip_model(
    workflow: &mut WorkflowSpec,
    relations: &[WorkflowRelation],
) -> Option<String> {
    let edge_index = workflow.edges.iter().position(|edge| {
        let source = workflow.nodes.iter().find(|node| node.id == edge.source);
        let target = workflow.nodes.iter().find(|node| node.id == edge.target);
        source.is_some_and(|node| node.config.kind() == WorkflowNodeKind::Table)
            && target.is_some_and(|node| node.config.kind() == WorkflowNodeKind::Query)
    })?;
    let replaced = workflow.edges.remove(edge_index);
    let mut suffix = 1_u32;
    let node_id = loop {
        let candidate = if suffix == 1 {
            "siglip".to_owned()
        } else {
            format!("siglip_{suffix}")
        };
        if !workflow.nodes.iter().any(|node| node.id == candidate) {
            break candidate;
        }
        suffix += 1;
    };
    let source_relation = workflow.nodes.iter().find_map(|node| {
        (node.id == replaced.source)
            .then_some(&node.config)
            .and_then(|config| match config {
                WorkflowNodeConfig::Table { relation } => Some(relation.as_str()),
                _ => None,
            })
    });
    let media_column = relations
        .iter()
        .find(|relation| Some(relation.name.as_str()) == source_relation)
        .and_then(|relation| relation_model_media_fields(relation).into_iter().next())
        .unwrap_or_default();
    let query_id = replaced.target.clone();
    if let Some(query) = workflow.nodes.iter_mut().find(|node| node.id == query_id)
        && let WorkflowNodeConfig::Query { sql, .. } = &mut query.config
    {
        *sql = sql.replace(
            &format!("{{{{node:{}}}}}", replaced.source),
            &format!("{{{{node:{node_id}}}}}"),
        );
    }
    let insert_at = workflow
        .nodes
        .iter()
        .position(|node| node.id == query_id)
        .unwrap_or(workflow.nodes.len());
    workflow.nodes.insert(
        insert_at,
        spiral_robotics_contracts::workflow::WorkflowNodeSpec {
            id: node_id.clone(),
            name: "SigLIP scores".to_owned(),
            config: WorkflowNodeConfig::Model {
                model_id: SIGLIP_BASE_MODEL_ID.to_owned(),
                configuration: BTreeMap::new(),
                media_column,
                concepts: Vec::new(),
                concept_columns: Vec::new(),
                output_relation: None,
            },
        },
    );
    workflow.edges.insert(
        edge_index,
        WorkflowEdgeSpec {
            source: replaced.source,
            target: node_id.clone(),
        },
    );
    workflow.edges.insert(
        edge_index + 1,
        WorkflowEdgeSpec {
            source: node_id.clone(),
            target: query_id,
        },
    );
    Some(node_id)
}

fn remove_siglip_model(workflow: &mut WorkflowSpec, node_id: &str) -> Option<String> {
    let model_index = workflow
        .nodes
        .iter()
        .position(|node| node.id == node_id && node.config.kind() == WorkflowNodeKind::Model)?;
    let incoming_index = workflow
        .edges
        .iter()
        .position(|edge| edge.target == node_id)?;
    let outgoing_index = workflow
        .edges
        .iter()
        .position(|edge| edge.source == node_id)?;
    let incoming = workflow.edges[incoming_index].clone();
    let outgoing = workflow.edges[outgoing_index].clone();
    let table_is_upstream = workflow
        .nodes
        .iter()
        .any(|node| node.id == incoming.source && node.config.kind() == WorkflowNodeKind::Table);
    let query_is_downstream = workflow
        .nodes
        .iter()
        .any(|node| node.id == outgoing.target && node.config.kind() == WorkflowNodeKind::Query);
    if !table_is_upstream || !query_is_downstream {
        return None;
    }

    workflow.nodes.remove(model_index);
    workflow
        .edges
        .retain(|edge| edge.source != node_id && edge.target != node_id);
    workflow.edges.insert(
        incoming_index.min(outgoing_index).min(workflow.edges.len()),
        WorkflowEdgeSpec {
            source: incoming.source.clone(),
            target: outgoing.target.clone(),
        },
    );
    if let Some(query) = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == outgoing.target)
        && let WorkflowNodeConfig::Query { sql, .. } = &mut query.config
    {
        *sql = sql.replace(
            &format!("{{{{node:{node_id}}}}}"),
            &format!("{{{{node:{}}}}}", incoming.source),
        );
    }
    Some(outgoing.target)
}

fn topology_mutation_blocked(phase: WorkflowRunPhase, command: Option<WorkflowCommand>) -> bool {
    phase.active() || command.is_some()
}

fn relation_model_media_fields(relation: &WorkflowRelation) -> Vec<String> {
    let mut fields = relation
        .columns
        .iter()
        .filter(|column| column.semantic_type.as_deref() == Some("spql.image.Image"))
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    let names = relation
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if [
        "episode_id",
        "stream_id",
        "segment_index",
        "timeline_offset_ns",
        "fps",
        "frames",
        "media_relation",
    ]
    .iter()
    .all(|field| names.contains(field))
    {
        fields.push("media_relation".to_owned());
    }
    fields
}

fn revision_conflict(draft: &WorkflowSpec, server: &WorkflowSpec) -> bool {
    draft != server && draft.revision != server.revision
}

fn graph_nodes(
    workflow: &WorkflowSpec,
    snapshot: &WorkflowRunSnapshot,
    relations: &[WorkflowRelation],
) -> Vec<Node<WorkflowNodeData>> {
    let relation_index = relations
        .iter()
        .map(|relation| (relation.name.as_str(), relation))
        .collect::<BTreeMap<_, _>>();
    let phase_index = snapshot
        .nodes
        .iter()
        .map(|node| (node.node_id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let mut table_index = 0_usize;
    let mut model_index = 0_usize;
    let mut query_index = 0_usize;
    let mut result_index = 0_usize;
    let has_model = workflow_has_model(workflow);
    workflow
        .nodes
        .iter()
        .map(|node| {
            let (position, type_name) = match node.config.kind() {
                WorkflowNodeKind::Table => {
                    let position = (24.0, 30.0 + table_index as f64 * 178.0);
                    table_index += 1;
                    (position, "table")
                }
                WorkflowNodeKind::Model => {
                    let position = (320.0, 70.0 + model_index as f64 * 178.0);
                    model_index += 1;
                    (position, "model")
                }
                WorkflowNodeKind::Query => {
                    let position = (
                        if has_model { 640.0 } else { 400.0 },
                        70.0 + query_index as f64 * 178.0,
                    );
                    query_index += 1;
                    (position, "query")
                }
                WorkflowNodeKind::Result => {
                    let position = (
                        if has_model { 980.0 } else { 760.0 },
                        70.0 + result_index as f64 * 178.0,
                    );
                    result_index += 1;
                    (position, "result")
                }
            };
            Node::with_data(
                node.id.clone(),
                node.name.clone(),
                position,
                node_data(
                    node,
                    phase_index.get(node.id.as_str()).copied(),
                    snapshot,
                    &relation_index,
                ),
            )
            .node_type(type_name)
            .sides(Side::Left, Side::Right)
        })
        .collect()
}

fn node_data(
    node: &spiral_robotics_contracts::workflow::WorkflowNodeSpec,
    node_snapshot: Option<&WorkflowNodeSnapshot>,
    run_snapshot: &WorkflowRunSnapshot,
    relations: &BTreeMap<&str, &WorkflowRelation>,
) -> WorkflowNodeData {
    let phase = node_snapshot.map_or(WorkflowNodePhase::Pending, |snapshot| snapshot.phase);
    let detail = node_snapshot
        .map(|snapshot| snapshot.detail.clone())
        .unwrap_or_else(|| "Not run".to_owned());
    let error = node_snapshot.and_then(|snapshot| snapshot.error.clone());
    let progress = node_snapshot.and_then(|snapshot| snapshot.progress.clone());
    let quarantine = node_snapshot
        .map(|snapshot| snapshot.quarantine.clone())
        .unwrap_or_default();
    let model_snapshot = node_snapshot.and_then(|snapshot| snapshot.model.clone());
    let warnings = node_snapshot
        .map(|snapshot| snapshot.warnings.clone())
        .unwrap_or_default();
    match &node.config {
        WorkflowNodeConfig::Table { relation } => {
            let metadata = relations.get(relation.as_str());
            WorkflowNodeData {
                kind: WorkflowNodeKind::Table,
                fields: metadata
                    .map(|metadata| {
                        metadata
                            .columns
                            .iter()
                            .map(|column| column.name.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
                relation: (!relation.is_empty()).then(|| relation.clone()),
                output_relation: None,
                sql: None,
                description: metadata.map_or_else(
                    || "Choose an existing qualified Spiral relation.".to_owned(),
                    |metadata| {
                        format!(
                            "{} · {} fields",
                            metadata.relation_type,
                            metadata.columns.len()
                        )
                    },
                ),
                phase,
                detail,
                error,
                model_id: None,
                media_column: None,
                concepts: Vec::new(),
                concept_columns: Vec::new(),
                progress,
                quarantine,
                model: model_snapshot,
                warnings,
            }
        }
        WorkflowNodeConfig::Model {
            model_id,
            media_column,
            concepts,
            concept_columns,
            output_relation,
            ..
        } => WorkflowNodeData {
            kind: WorkflowNodeKind::Model,
            fields: if concept_columns.is_empty() {
                resolve_siglip_concepts(concepts)
                    .into_iter()
                    .map(|mapping| mapping.score_column)
                    .collect()
            } else {
                concept_columns
                    .iter()
                    .map(|mapping| mapping.score_column.clone())
                    .collect()
            },
            relation: None,
            output_relation: output_relation.clone(),
            sql: None,
            description: "Pinned SigLIP zero-shot scoring over an upstream Spiral image field."
                .to_owned(),
            phase,
            detail,
            error,
            model_id: Some(model_id.clone()),
            media_column: Some(media_column.clone()),
            concepts: concepts.clone(),
            concept_columns: if concept_columns.is_empty() {
                resolve_siglip_concepts(concepts)
            } else {
                concept_columns.clone()
            },
            progress,
            quarantine,
            model: model_snapshot,
            warnings,
        },
        WorkflowNodeConfig::Query { sql, .. } => WorkflowNodeData {
            kind: WorkflowNodeKind::Query,
            fields: Vec::new(),
            relation: None,
            output_relation: None,
            sql: Some(sql.clone()),
            description: "Bounded read-only Spiral SQL with server-owned relation placeholders."
                .to_owned(),
            phase,
            detail,
            error,
            model_id: None,
            media_column: None,
            concepts: Vec::new(),
            concept_columns: Vec::new(),
            progress,
            quarantine,
            model: model_snapshot,
            warnings,
        },
        WorkflowNodeConfig::Result => WorkflowNodeData {
            kind: WorkflowNodeKind::Result,
            fields: run_snapshot
                .result
                .as_ref()
                .map(|result| {
                    result
                        .columns
                        .iter()
                        .map(|column| column.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            relation: None,
            output_relation: None,
            sql: None,
            description: "Terminal media-aware query result.".to_owned(),
            phase,
            detail,
            error,
            model_id: None,
            media_column: None,
            concepts: Vec::new(),
            concept_columns: Vec::new(),
            progress,
            quarantine,
            model: model_snapshot,
            warnings,
        },
    }
}

fn graph_edges(edges: &[WorkflowEdgeSpec]) -> Vec<Edge> {
    edges
        .iter()
        .map(|edge| Edge::new(edge.source.clone(), edge.target.clone()).kind(EdgeKind::SmoothStep))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_selection_activates_the_node_inspector() {
        let selected = reduce_workflow_ui(
            &WorkflowUiState::default(),
            WorkflowUiAction::SelectNode("query".to_owned()),
        );
        assert_eq!(selected.selected_node.as_deref(), Some("query"));
        assert_eq!(selected.active_panel.as_deref(), Some(NODE_PANEL_ID));
    }

    #[test]
    fn default_editor_is_table_query_result_without_a_fake_model() {
        let state = initial_state();
        let kinds = state
            .workflow
            .nodes
            .iter()
            .map(|node| node.config.kind())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                WorkflowNodeKind::Table,
                WorkflowNodeKind::Query,
                WorkflowNodeKind::Result
            ]
        );
        assert!(!workflow_has_model(&state.workflow));
    }

    #[test]
    fn graph_projection_preserves_saved_topology() {
        let workflow = WorkflowSpec::default();
        let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        let nodes = graph_nodes(&workflow, &snapshot, &[]);
        let edges = graph_edges(&workflow.edges);
        assert_eq!(nodes.len(), 3);
        assert_eq!(edges.len(), 2);
        assert_eq!(nodes[0].id, "table");
        assert_eq!(nodes[2].id, "result");
    }

    #[test]
    fn inserting_siglip_rewrites_lineage_and_selects_the_authoritative_image_field() {
        let mut workflow = WorkflowSpec::default();
        let WorkflowNodeConfig::Table { relation } = &mut workflow.nodes[0].config else {
            unreachable!()
        };
        *relation = "pick_place.camera_frames".to_owned();
        let relations = vec![WorkflowRelation {
            name: relation.clone(),
            relation_type: "TABLE".to_owned(),
            columns: vec![spiral_robotics_contracts::api::QueryColumn {
                name: "image".to_owned(),
                data_type: "Struct".to_owned(),
                nullable: false,
                semantic_type: Some("spql.image.Image".to_owned()),
            }],
            row_count: None,
        }];
        let node_id = insert_siglip_model(&mut workflow, &relations).unwrap();
        assert_eq!(node_id, "siglip");
        assert_eq!(workflow.nodes.len(), 4);
        assert_eq!(workflow.edges.len(), 3);
        assert_eq!(workflow.edges[0].source, "table");
        assert_eq!(workflow.edges[0].target, "siglip");
        assert_eq!(workflow.edges[1].source, "siglip");
        assert_eq!(workflow.edges[1].target, "query");
        let WorkflowNodeConfig::Model { media_column, .. } = &workflow.nodes[1].config else {
            unreachable!()
        };
        assert_eq!(media_column, "image");
        let WorkflowNodeConfig::Query { sql, .. } = &workflow.nodes[2].config else {
            unreachable!()
        };
        assert!(sql.contains("{{node:siglip}}"));
        assert!(!sql.contains("{{node:table}}"));
    }

    #[test]
    fn removing_siglip_restores_table_query_lineage_and_placeholder() {
        let mut workflow = WorkflowSpec::default();
        insert_siglip_model(&mut workflow, &[]).unwrap();

        let selected = remove_siglip_model(&mut workflow, "siglip").unwrap();

        assert_eq!(selected, "query");
        assert_eq!(workflow.nodes.len(), 3);
        assert_eq!(workflow.edges.len(), 2);
        assert_eq!(workflow.edges[0].source, "table");
        assert_eq!(workflow.edges[0].target, "query");
        assert_eq!(workflow.edges[1].source, "query");
        assert_eq!(workflow.edges[1].target, "result");
        let WorkflowNodeConfig::Query { sql, .. } = &workflow.nodes[1].config else {
            unreachable!()
        };
        assert!(sql.contains("{{node:table}}"));
        assert!(!sql.contains("{{node:siglip}}"));
    }

    #[test]
    fn topology_mutation_is_blocked_during_start_command_before_sse_arrives() {
        assert!(topology_mutation_blocked(
            WorkflowRunPhase::Idle,
            Some(WorkflowCommand::Starting)
        ));
        assert!(!topology_mutation_blocked(WorkflowRunPhase::Idle, None));
    }

    #[test]
    fn model_projection_preserves_authoritative_progress_and_warning_evidence() {
        let node = spiral_robotics_contracts::workflow::WorkflowNodeSpec {
            id: "siglip".to_owned(),
            name: "SigLIP scores".to_owned(),
            config: WorkflowNodeConfig::Model {
                model_id: SIGLIP_BASE_MODEL_ID.to_owned(),
                configuration: BTreeMap::new(),
                media_column: "image".to_owned(),
                concepts: vec!["red cube".to_owned()],
                concept_columns: vec![WorkflowConceptColumn {
                    concept: "red cube".to_owned(),
                    score_column: "score_red_cube".to_owned(),
                }],
                output_relation: Some("workflow.siglip".to_owned()),
            },
        };
        let snapshot = WorkflowNodeSnapshot {
            node_id: node.id.clone(),
            phase: WorkflowNodePhase::CompleteWithWarnings,
            detail: "Complete with warnings".to_owned(),
            progress: Some(WorkflowNodeProgress {
                completed: 8,
                total: 8,
                unit: "rows".to_owned(),
            }),
            quarantine: WorkflowQuarantineSummary {
                count: 1,
                examples: vec!["row 3: image is null".to_owned()],
            },
            warnings: vec!["1 input row was quarantined.".to_owned()],
            ..WorkflowNodeSnapshot::default()
        };
        let run = WorkflowRunSnapshot {
            nodes: vec![snapshot.clone()],
            ..WorkflowRunSnapshot::default()
        };
        let projected = node_data(&node, Some(&snapshot), &run, &BTreeMap::new());
        assert_eq!(projected.phase, WorkflowNodePhase::CompleteWithWarnings);
        assert_eq!(projected.progress.as_ref().unwrap().completed, 8);
        assert_eq!(projected.quarantine.count, 1);
        assert_eq!(projected.fields, ["score_red_cube"]);
    }

    #[test]
    fn dirty_draft_keeps_its_base_revision_for_conflict_detection() {
        let mut draft = WorkflowSpec::default();
        let mut server = draft.clone();
        draft.name = "Local edit".to_owned();
        assert!(!revision_conflict(&draft, &server));
        server.revision = 1;
        server.name = "Remote edit".to_owned();
        assert!(revision_conflict(&draft, &server));
        assert_eq!(draft.revision, 0, "Apply must send the draft base revision");
    }

    #[test]
    fn slower_bootstrap_response_cannot_regress_newer_sse_state() {
        let mut current = WorkflowStateResponse::default();
        current.workflow.revision = 2;
        current.snapshot.workflow_revision = 2;
        let mut stale_bootstrap = current.clone();
        stale_bootstrap.workflow.revision = 1;
        stale_bootstrap.snapshot.workflow_revision = 1;
        assert!(!server_state_is_monotonic(&current, &stale_bootstrap));

        let mut newer = current.clone();
        newer.workflow.revision = 3;
        newer.snapshot.workflow_revision = 3;
        assert!(server_state_is_monotonic(&current, &newer));
    }

    #[test]
    fn late_run_response_cannot_regress_a_completed_sse_snapshot() {
        let mut current = WorkflowStateResponse::default();
        current.snapshot.run_id = Some("workflow-run-1".to_owned());
        current.snapshot.phase = WorkflowRunPhase::Completed;
        current.snapshot.started_ms = 10;
        current.snapshot.updated_ms = 12;
        current.snapshot.finished_ms = 12;

        let mut late_start = current.clone();
        late_start.snapshot.phase = WorkflowRunPhase::Validating;
        late_start.snapshot.finished_ms = 0;
        late_start.snapshot.updated_ms = 10;
        assert!(!server_state_is_monotonic(&current, &late_start));

        // Millisecond clock ties still cannot move a terminal run backwards.
        late_start.snapshot.updated_ms = current.snapshot.updated_ms;
        assert!(!server_state_is_monotonic(&current, &late_start));
    }

    #[test]
    fn late_cancel_response_cannot_regress_a_cancelled_sse_snapshot() {
        let mut current = WorkflowStateResponse::default();
        current.snapshot.run_id = Some("workflow-run-1".to_owned());
        current.snapshot.phase = WorkflowRunPhase::Cancelled;
        current.snapshot.started_ms = 10;
        current.snapshot.updated_ms = 13;
        current.snapshot.finished_ms = 13;

        let mut late_cancel = current.clone();
        late_cancel.snapshot.phase = WorkflowRunPhase::Cancelling;
        late_cancel.snapshot.finished_ms = 0;
        late_cancel.snapshot.updated_ms = 12;
        assert!(!snapshot_is_monotonic(
            &current.snapshot,
            &late_cancel.snapshot
        ));

        late_cancel.snapshot.updated_ms = current.snapshot.updated_ms;
        assert!(!snapshot_is_monotonic(
            &current.snapshot,
            &late_cancel.snapshot
        ));
    }
}
