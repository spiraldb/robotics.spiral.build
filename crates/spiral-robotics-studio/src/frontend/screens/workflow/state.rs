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
    RunWorkflowRequest, SaveWorkflowRequest, WorkflowEdgeSpec, WorkflowNodeConfig,
    WorkflowNodeKind, WorkflowNodePhase, WorkflowNodeSnapshot, WorkflowRelation,
    WorkflowRelationsResponse, WorkflowRunPhase, WorkflowRunSnapshot, WorkflowSpec,
    WorkflowStateResponse, WorkflowValidationReport, validate_workflow_spec, workflow_has_model,
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
            WorkflowNodePhase::Failed | WorkflowNodePhase::Unsupported => {
                "workflow-node-state-failed"
            }
            WorkflowNodePhase::Running
            | WorkflowNodePhase::Validating
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
            WorkflowNodePhase::Running => "Running",
            WorkflowNodePhase::Complete => "Complete",
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
        if workflow_has_model(&self.draft.read()) {
            return Some(
                "Model execution arrives in Stage 3. Remove Model nodes to run SQL now.".to_owned(),
            );
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
        let previous = self
            .nodes
            .peek()
            .iter()
            .map(|node| (node.id.clone(), node.clone()))
            .collect::<HashMap<_, _>>();
        for node in &mut next {
            if let Some(existing) = previous.get(&node.id) {
                node.position = existing.position;
                node.size = existing.size;
                node.measured = existing.measured;
                node.selected = existing.selected;
            }
        }
        self.nodes.set(next);
        self.edges.set(graph_edges(&spec.edges));
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
            }
        }
        WorkflowNodeConfig::Model {
            output_relation, ..
        } => WorkflowNodeData {
            kind: WorkflowNodeKind::Model,
            fields: Vec::new(),
            relation: None,
            output_relation: output_relation.clone(),
            sql: None,
            description: "Reserved schema · execution arrives in Stage 3.".to_owned(),
            phase: WorkflowNodePhase::Unsupported,
            detail: "Unavailable in Stage 2".to_owned(),
            error: Some(
                "Remove Model nodes to run SQL now; Model execution arrives in Stage 3.".to_owned(),
            ),
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
