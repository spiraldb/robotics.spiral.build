//! Shell-owned Workflow editor state and backend commands.
//!
//! Canvas geometry and inspector selection are browser interaction state. The
//! saved graph, validation, execution phases, and result payload are backend
//! truth and reconcile through HTTP plus the one shared SSE stream.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use dioxus::prelude::*;
use dioxus_flow::{Edge, EdgeKind, Node, Side};
use dioxus_workbench::PanelId;
use gloo_net::http::Method;
use spiral_robotics_api::workflow::{
    CancelWorkflowRequest, RunWorkflowRequest, SIGLIP_BASE_MODEL_ID, SaveWorkflowRequest,
    WorkflowAgentActivateConversationRequest, WorkflowAgentArtifactKind,
    WorkflowAgentArtifactReceipt, WorkflowAgentCancelRequest,
    WorkflowAgentCreateConversationRequest, WorkflowAgentStateResponse, WorkflowAgentTurnAccepted,
    WorkflowAgentTurnRequest, WorkflowCapabilities, WorkflowEdgeSpec, WorkflowFeatureApplicability,
    WorkflowFeatureDefinition, WorkflowModelRunMetadata, WorkflowNodeConfig, WorkflowNodeKind,
    WorkflowNodePhase, WorkflowNodeProgress, WorkflowNodeSnapshot, WorkflowQuarantineSummary,
    WorkflowRelation, WorkflowRelationsResponse, WorkflowRunPhase, WorkflowRunSnapshot,
    WorkflowSpec, WorkflowStateResponse, WorkflowValidationReport, derived_feature_cache_relation,
};

use crate::client::http as api;
use crate::shell::notifications::MessageSource;
use crate::shell::state::{Studio, studio};

pub(crate) const ASSISTANT_PANEL_ID: &str = "workflow-assistant";
pub(crate) const NODE_PANEL_ID: &str = "workflow-node";
pub(crate) const CANVAS_PANEL_ID: &str = "workflow-canvas";
pub(crate) const RESULTS_PANEL_ID: &str = "workflow-results";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WorkflowNodeData {
    pub(crate) kind: WorkflowNodeKind,
    pub(crate) fields: Vec<String>,
    pub(crate) relation: Option<String>,
    pub(crate) feature_cache_relation: Option<String>,
    pub(crate) sql: Option<String>,
    pub(crate) description: String,
    pub(crate) phase: WorkflowNodePhase,
    pub(crate) detail: String,
    pub(crate) error: Option<String>,
    pub(crate) model_id: Option<String>,
    pub(crate) features: Vec<WorkflowFeatureDefinition>,
    pub(crate) progress: Option<WorkflowNodeProgress>,
    pub(crate) quarantine: WorkflowQuarantineSummary,
    pub(crate) model: Option<WorkflowModelRunMetadata>,
    pub(crate) warnings: Vec<String>,
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
    pub(crate) selected_node: Option<String>,
    pub(crate) active_panel: Option<String>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowAgentCommand {
    Sending,
    Cancelling,
    CreatingConversation,
    ActivatingConversation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AcceptedAgentDraft {
    pub(crate) scope_id: String,
    pub(crate) conversation_id: String,
    pub(crate) text: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct AgentDraftKey {
    pub(crate) scope_id: String,
    pub(crate) conversation_id: String,
}

impl AgentDraftKey {
    pub(crate) fn new(scope_id: &str, conversation_id: &str) -> Self {
        Self {
            scope_id: scope_id.to_owned(),
            conversation_id: conversation_id.to_owned(),
        }
    }
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
    agent: Signal<WorkflowAgentStateResponse>,
    agent_loaded: Signal<bool>,
    agent_command: Signal<Option<WorkflowAgentCommand>>,
    agent_error: Signal<Option<String>>,
    agent_accepted_draft: Signal<Option<AcceptedAgentDraft>>,
    agent_drafts: Signal<BTreeMap<AgentDraftKey, String>>,
}

pub(crate) fn use_workflow_controller() -> WorkflowController {
    let initial = initial_state();
    let initial_spec = initial.workflow.clone();
    let initial_nodes = graph_nodes(&initial_spec, &initial.snapshot, &[]);
    let initial_edges = graph_edges(&initial_spec.edges, &initial.snapshot);
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
        agent: use_signal(WorkflowAgentStateResponse::default),
        agent_loaded: use_signal(|| false),
        agent_command: use_signal(|| None),
        agent_error: use_signal(|| None),
        agent_accepted_draft: use_signal(|| None),
        agent_drafts: use_signal(BTreeMap::new),
    };
    let studio = studio();
    use_future(move || async move {
        controller.load(studio).await;
    });
    use_effect(move || {
        if let Some(state) = studio.workflow_state.read().clone() {
            let scope_changed = controller.reconcile_state(state);
            if scope_changed {
                let expected_scope_id = controller.server.peek().scope_id.clone();
                dioxus::dioxus_core::spawn_forever(async move {
                    controller.load_relations(studio, expected_scope_id).await;
                });
            }
        }
    });
    use_effect(move || {
        if let Some(state) = studio.workflow_agent_state.read().clone() {
            controller.reconcile_agent(state);
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
        self.draft.read().has_model()
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

    pub(crate) fn validation(&self) -> WorkflowValidationReport {
        self.draft.read().validate()
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

    pub(crate) fn agent_state(&self) -> WorkflowAgentStateResponse {
        self.agent.read().clone()
    }

    pub(crate) fn agent_loaded(&self) -> bool {
        *self.agent_loaded.read()
    }

    pub(crate) fn agent_command(&self) -> Option<WorkflowAgentCommand> {
        *self.agent_command.read()
    }

    pub(crate) fn agent_error(&self) -> Option<String> {
        self.agent_error.read().clone()
    }

    pub(crate) const fn agent_accepted_draft(self) -> Signal<Option<AcceptedAgentDraft>> {
        self.agent_accepted_draft
    }

    /// Shell-hoisted unsent text survives Workflow route and Settings outlet
    /// unmounts while remaining isolated by project scope and durable
    /// conversation identity.
    pub(crate) const fn agent_drafts(self) -> Signal<BTreeMap<AgentDraftKey, String>> {
        self.agent_drafts
    }

    pub(crate) fn agent_conversation_disabled_reason(
        &self,
        connected: bool,
        catalog_available: bool,
    ) -> Option<String> {
        let agent = self.agent_state();
        let snapshot = self.snapshot();
        agent_conversation_change_reason(
            self.agent_loaded(),
            connected,
            catalog_available,
            self.agent_command(),
            &agent,
            &snapshot,
        )
    }

    pub(crate) fn new_agent_conversation_disabled_reason(
        &self,
        connected: bool,
        catalog_available: bool,
    ) -> Option<String> {
        if let Some(reason) = self.agent_conversation_disabled_reason(connected, catalog_available)
        {
            return Some(reason);
        }
        let agent = self.agent_state();
        if agent.conversations.len() >= agent.conversation_limit {
            return Some(format!(
                "The {0}-conversation limit is reached. Previous conversations are kept; continue one of them instead.",
                agent.conversation_limit
            ));
        }
        None
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
        if self.agent_state().session.phase.active() || self.agent_command().is_some() {
            return Some("The Workflow Assistant is using this graph.".to_owned());
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
        let draft = self.draft.read();
        if draft.nodes.is_empty() && draft.edges.is_empty() {
            return Some("Ask the Assistant to build this workflow before running.".to_owned());
        }
        drop(draft);
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
        if self.agent_state().session.phase.active() || self.agent_command().is_some() {
            return Some(
                "Wait for the active Assistant turn before applying graph changes.".to_owned(),
            );
        }
        let draft = self.draft.read();
        if draft.nodes.is_empty() && draft.edges.is_empty() {
            return None;
        }
        drop(draft);
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

    pub(crate) fn receipt_panel(&self, receipt: &WorkflowAgentArtifactReceipt) -> Option<PanelId> {
        let dirty = self.dirty();
        let server = self.server.read();
        receipt_panel_id(receipt, &server, dirty).map(PanelId::from)
    }

    pub(crate) fn open_agent_receipt(mut self, receipt: WorkflowAgentArtifactReceipt) {
        let Some(panel) = self.receipt_panel(&receipt) else {
            return;
        };
        let next = reduce_workflow_ui(
            &self.ui.peek(),
            WorkflowUiAction::ActivatePanel(panel.to_string()),
        );
        self.ui.set(next);
    }

    pub(crate) fn reload_saved(mut self) {
        self.draft.set(self.server.peek().workflow.clone());
        self.load_error.set(None);
        self.sync_graph();
    }

    pub(crate) fn clear_workflow_draft(mut self) {
        if topology_mutation_blocked(self.snapshot().phase, self.command())
            || self.agent_state().session.phase.active()
            || self.agent_command().is_some()
        {
            return;
        }
        let empty = empty_workflow_draft(&self.draft.peek());
        self.draft.set(empty);
        self.clear_selection();
        self.sync_graph();
    }

    pub(crate) fn set_table_relation(mut self, node_id: &str, relation: String) {
        let mut draft = self.draft.write();
        let changed = edit_node(&mut draft, node_id, |config| {
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
            normalize_draft_model_sources(&mut draft);
        }
        drop(draft);
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

    pub(crate) fn set_model_feature(
        mut self,
        node_id: &str,
        index: usize,
        edit: impl FnOnce(&mut WorkflowFeatureDefinition),
    ) {
        let mut draft = self.draft.write();
        let changed = edit_node(&mut draft, node_id, |config| {
            let WorkflowNodeConfig::Model { features, .. } = config else {
                return false;
            };
            let Some(feature) = features.get_mut(index) else {
                return false;
            };
            edit(feature);
            true
        });
        let features = draft
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .and_then(|node| match &node.config {
                WorkflowNodeConfig::Model { features, .. } => Some(features.clone()),
                _ => None,
            })
            .unwrap_or_default();
        if changed {
            refresh_generated_feature_query(&mut draft, node_id, &features);
        }
        drop(draft);
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn add_model_feature(mut self, node_id: &str) {
        let mut draft = self.draft.write();
        let changed = edit_node(&mut draft, node_id, |config| {
            let WorkflowNodeConfig::Model { features, .. } = config else {
                return false;
            };
            let Some(first) = features.first() else {
                return false;
            };
            let applicability = WorkflowFeatureApplicability {
                source_relation: first.applicability.source_relation.clone(),
                media_field: first.applicability.media_field.clone(),
                stream_ids: Vec::new(),
            };
            features.push(WorkflowFeatureDefinition {
                name: String::new(),
                description: String::new(),
                concept: String::new(),
                applicability,
            });
            true
        });
        if changed {
            let features = model_features(&draft, node_id);
            refresh_generated_feature_query(&mut draft, node_id, &features);
        }
        drop(draft);
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn remove_model_feature(mut self, node_id: &str, index: usize) {
        let mut draft = self.draft.write();
        let changed = edit_node(&mut draft, node_id, |config| {
            let WorkflowNodeConfig::Model { features, .. } = config else {
                return false;
            };
            if features.len() <= 1 || index >= features.len() {
                return false;
            }
            features.remove(index);
            true
        });
        if changed {
            let features = model_features(&draft, node_id);
            refresh_generated_feature_query(&mut draft, node_id, &features);
        }
        drop(draft);
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn add_siglip_model(mut self) {
        if self.has_model() || topology_mutation_blocked(self.snapshot().phase, self.command()) {
            return;
        }
        let Some(node_id) = insert_siglip_model(&mut self.draft.write()) else {
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
        let requested_scope_id = self.server.peek().scope_id.clone();
        let request = SaveWorkflowRequest {
            scope_id: requested_scope_id.clone(),
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
            self.finish_command(&requested_scope_id);
            match response {
                Ok(response) => self.accept_applied_state(response, &requested_scope_id),
                Err(error) => {
                    studio.report_from(MessageSource::Workflow, error);
                    if let Ok(state) = api::get::<WorkflowStateResponse>("/api/workflow").await {
                        self.reconcile_state_command(state, &requested_scope_id);
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
        let requested_scope_id = self.server.peek().scope_id.clone();
        let request = RunWorkflowRequest {
            scope_id: requested_scope_id.clone(),
            revision: self.server.peek().workflow.revision,
        };
        dioxus::dioxus_core::spawn_forever(async move {
            let response = api::send_json::<_, WorkflowRunSnapshot>(
                Method::POST,
                "/api/workflow/run",
                Some(&request),
            )
            .await;
            self.finish_command(&requested_scope_id);
            match response {
                Ok(snapshot) => self.reconcile_snapshot(snapshot, &requested_scope_id),
                Err(error) => studio.report_from(MessageSource::Workflow, error),
            }
        });
    }

    pub(crate) fn cancel(mut self, studio: Studio) {
        if !self.snapshot().phase.active() || self.command().is_some() {
            return;
        }
        self.command.set(Some(WorkflowCommand::Cancelling));
        let requested_scope_id = self.server.peek().scope_id.clone();
        let request = CancelWorkflowRequest {
            scope_id: requested_scope_id.clone(),
        };
        dioxus::dioxus_core::spawn_forever(async move {
            let response = api::send_json::<_, WorkflowRunSnapshot>(
                Method::POST,
                "/api/workflow/cancel",
                Some(&request),
            )
            .await;
            self.finish_command(&requested_scope_id);
            match response {
                Ok(snapshot) => self.reconcile_snapshot(snapshot, &requested_scope_id),
                Err(error) => studio.report_from(MessageSource::Workflow, error),
            }
        });
    }

    pub(crate) fn create_agent_conversation(mut self, studio: Studio) {
        let catalog_available = studio.snapshot.read().readiness.catalog_available;
        if self
            .new_agent_conversation_disabled_reason((studio.live)(), catalog_available)
            .is_some()
        {
            return;
        }
        let current = self.agent.peek().clone();
        self.agent_command
            .set(Some(WorkflowAgentCommand::CreatingConversation));
        self.agent_error.set(None);
        let request = WorkflowAgentCreateConversationRequest {
            scope_id: current.scope_id.clone(),
            expected_revision: current.revision,
        };
        dioxus::dioxus_core::spawn_forever(async move {
            let response = api::send_json::<_, WorkflowAgentStateResponse>(
                Method::POST,
                "/api/workflow/agent/conversations",
                Some(&request),
            )
            .await;
            self.agent_command.set(None);
            match response {
                Ok(state) => {
                    self.reconcile_agent_command(state, &request.scope_id);
                }
                Err(error) => {
                    self.agent_error.set(Some(error.clone()));
                    studio.report_from(MessageSource::Workflow, error);
                }
            }
        });
    }

    pub(crate) fn activate_agent_conversation(mut self, conversation_id: String, studio: Studio) {
        let current = self.agent.peek().clone();
        let catalog_available = studio.snapshot.read().readiness.catalog_available;
        if conversation_id == current.active_conversation_id
            || self
                .agent_conversation_disabled_reason((studio.live)(), catalog_available)
                .is_some()
        {
            return;
        }
        self.agent_command
            .set(Some(WorkflowAgentCommand::ActivatingConversation));
        self.agent_error.set(None);
        let request = WorkflowAgentActivateConversationRequest {
            scope_id: current.scope_id.clone(),
            conversation_id,
            expected_revision: current.revision,
        };
        dioxus::dioxus_core::spawn_forever(async move {
            let response = api::send_json::<_, WorkflowAgentStateResponse>(
                Method::PUT,
                "/api/workflow/agent/conversations/active",
                Some(&request),
            )
            .await;
            self.agent_command.set(None);
            match response {
                Ok(state) => {
                    self.reconcile_agent_command(state, &request.scope_id);
                }
                Err(error) => {
                    self.agent_error.set(Some(error.clone()));
                    studio.report_from(MessageSource::Workflow, error);
                }
            }
        });
    }

    pub(crate) fn send_agent(mut self, text: String, studio: Studio) {
        let text = text.trim().to_owned();
        let state = self.agent.peek().clone();
        if text.is_empty()
            || !state.readiness.configured
            || state.session.phase.active()
            || self.agent_command.peek().is_some()
            || self.server.peek().snapshot.phase.active()
        {
            return;
        }
        self.agent_command.set(Some(WorkflowAgentCommand::Sending));
        self.agent_error.set(None);
        let request = WorkflowAgentTurnRequest {
            scope_id: state.scope_id.clone(),
            conversation_id: state.active_conversation_id.clone(),
            text: text.clone(),
            expected_revision: state.revision,
        };
        dioxus::dioxus_core::spawn_forever(async move {
            let response = api::send_json::<_, WorkflowAgentTurnAccepted>(
                Method::POST,
                "/api/workflow/agent/turn",
                Some(&request),
            )
            .await;
            self.agent_command.set(None);
            match response {
                Ok(accepted) => {
                    if self.reconcile_agent_command(accepted.state, &request.scope_id) {
                        self.agent_accepted_draft.set(Some(AcceptedAgentDraft {
                            scope_id: request.scope_id,
                            conversation_id: request.conversation_id,
                            text,
                        }));
                    }
                }
                Err(error) => {
                    self.agent_error.set(Some(error.clone()));
                    studio.report_from(MessageSource::Workflow, error);
                    if let Ok(state) =
                        api::get::<WorkflowAgentStateResponse>("/api/workflow/agent").await
                    {
                        self.reconcile_agent_command(state, &request.scope_id);
                    }
                }
            }
        });
    }

    pub(crate) fn cancel_agent(mut self, studio: Studio) {
        let current = self.agent.peek().clone();
        if !current.session.phase.active() || self.agent_command.peek().is_some() {
            return;
        }
        self.agent_command
            .set(Some(WorkflowAgentCommand::Cancelling));
        dioxus::dioxus_core::spawn_forever(async move {
            let Some(turn_id) = current.session.active_turn_id.clone() else {
                self.agent_command.set(None);
                return;
            };
            let request = WorkflowAgentCancelRequest {
                scope_id: current.scope_id.clone(),
                conversation_id: current.active_conversation_id.clone(),
                turn_id,
            };
            let response = api::send_json::<_, WorkflowAgentStateResponse>(
                Method::POST,
                "/api/workflow/agent/cancel",
                Some(&request),
            )
            .await;
            self.agent_command.set(None);
            match response {
                Ok(state) => {
                    self.reconcile_agent_command(state, &request.scope_id);
                }
                Err(error) => {
                    self.agent_error.set(Some(error.clone()));
                    studio.report_from(MessageSource::Workflow, error);
                }
            }
        });
    }

    pub(crate) fn retry_agent(self, studio: Studio) {
        let text = self
            .agent
            .peek()
            .session
            .messages
            .iter()
            .rev()
            .find(|message| message.role == spiral_robotics_api::workflow::WorkflowAgentRole::User)
            .map(|message| message.text.clone());
        if let Some(text) = text {
            self.send_agent(text, studio);
        }
    }

    async fn load(mut self, studio: Studio) {
        let workflow_bootstrap_scope = self.server.peek().scope_id.clone();
        let state_loaded = match api::get::<WorkflowStateResponse>("/api/workflow").await {
            Ok(state) => self.reconcile_state_bootstrap(state, &workflow_bootstrap_scope),
            Err(error) => {
                if !*self.loaded.peek() {
                    self.load_error.set(Some(error.clone()));
                }
                studio.report_from(MessageSource::Workflow, error);
                false
            }
        };
        let relations_scope = self.server.peek().scope_id.clone();
        self.load_relations(studio, relations_scope).await;
        match api::get::<WorkflowCapabilities>("/api/workflow/capabilities").await {
            Ok(capabilities) => self.capabilities.set(capabilities),
            Err(error) => studio.report_from(MessageSource::Workflow, error),
        }
        let agent_bootstrap_scope = self.agent.peek().scope_id.clone();
        match api::get::<WorkflowAgentStateResponse>("/api/workflow/agent").await {
            Ok(state) => self.reconcile_agent_bootstrap(state, &agent_bootstrap_scope),
            Err(error) => {
                if !*self.agent_loaded.peek() {
                    self.agent_error.set(Some(error.clone()));
                }
                studio.report_from(MessageSource::Workflow, error);
            }
        }
        if state_loaded {
            self.loaded.set(true);
        }
    }

    async fn load_relations(mut self, studio: Studio, expected_scope_id: String) {
        match api::get::<WorkflowRelationsResponse>("/api/workflow/relations").await {
            Ok(relations) => {
                if relations_are_current(
                    &self.server.peek().scope_id,
                    &expected_scope_id,
                    &relations,
                ) {
                    self.relations.set(relations);
                    self.sync_graph();
                }
            }
            Err(error) => studio.report_from(MessageSource::Workflow, error),
        }
    }

    fn accept_applied_state(mut self, state: WorkflowStateResponse, requested_scope_id: &str) {
        let current = self.server.peek().clone();
        if current.scope_id != requested_scope_id
            || state.scope_id != requested_scope_id
            || state.workflow.id != current.workflow.id
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

    fn reconcile_snapshot(mut self, snapshot: WorkflowRunSnapshot, requested_scope_id: &str) {
        if self.server.peek().scope_id != requested_scope_id
            || snapshot.workflow_id != self.server.peek().workflow.id
            || snapshot.workflow_revision != self.server.peek().workflow.revision
            || !snapshot_is_monotonic(&self.server.peek().snapshot, &snapshot)
        {
            return;
        }
        self.server.write().snapshot = snapshot;
        self.sync_graph();
    }

    fn reconcile_state(mut self, state: WorkflowStateResponse) -> bool {
        if !server_state_is_monotonic(&self.server.peek(), &state) {
            return false;
        }
        // This path runs inside the shell's Workflow SSE effect. Keep draft
        // comparison untracked so the effect subscribes only to the incoming
        // backend signal instead of retriggering itself after writing server
        // state or graph projection.
        let scope_changed = state.scope_id != self.server.peek().scope_id;
        let keep_local_draft = !scope_changed && *self.draft.peek() != self.server.peek().workflow;
        if scope_changed {
            self.relations.set(WorkflowRelationsResponse::default());
            self.ui.set(WorkflowUiState::default());
            self.command.set(None);
        }
        self.server.set(state.clone());
        if !keep_local_draft {
            self.draft.set(state.workflow);
        }
        self.loaded.set(true);
        self.load_error.set(None);
        self.sync_graph();
        scope_changed
    }

    fn reconcile_state_command(
        self,
        state: WorkflowStateResponse,
        requested_scope_id: &str,
    ) -> bool {
        if self.server.peek().scope_id != requested_scope_id || state.scope_id != requested_scope_id
        {
            return false;
        }
        self.reconcile_state(state);
        true
    }

    fn finish_command(mut self, requested_scope_id: &str) {
        if self.server.peek().scope_id == requested_scope_id {
            self.command.set(None);
        }
    }

    fn reconcile_state_bootstrap(
        self,
        state: WorkflowStateResponse,
        requested_scope_id: &str,
    ) -> bool {
        let current = self.server.peek();
        if current.scope_id != requested_scope_id && state.scope_id != current.scope_id {
            return false;
        }
        drop(current);
        self.reconcile_state(state);
        true
    }

    fn reconcile_agent(mut self, state: WorkflowAgentStateResponse) {
        let current = self.agent.peek();
        if !agent_state_is_monotonic(&current, &state) {
            return;
        }
        drop(current);
        self.agent.set(state);
        self.agent_loaded.set(true);
        self.agent_error.set(None);
    }

    fn reconcile_agent_command(
        self,
        state: WorkflowAgentStateResponse,
        requested_scope_id: &str,
    ) -> bool {
        if !agent_command_response_is_current(&self.agent.peek(), &state, requested_scope_id) {
            // A command issued before a project reload cannot replace the
            // authoritative state from the newer server scope when its HTTP
            // response arrives late.
            return false;
        }
        self.reconcile_agent(state);
        true
    }

    fn reconcile_agent_bootstrap(
        self,
        state: WorkflowAgentStateResponse,
        requested_scope_id: &str,
    ) {
        let current = self.agent.peek();
        if !agent_bootstrap_is_current(&current, &state, requested_scope_id) {
            // A slower GET from the prior project/import scope must not replace
            // a newer authoritative scope already delivered over SSE.
            return;
        }
        drop(current);
        self.reconcile_agent(state);
    }

    fn sync_graph(mut self) {
        let spec = self.draft.peek().clone();
        let snapshot = self.server.peek().snapshot.clone();
        let relations = self.relations.peek().relations.clone();
        let mut next = graph_nodes(&spec, &snapshot, &relations);
        let next_edges = graph_edges(&spec.edges, &snapshot);
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

fn receipt_panel_id(
    receipt: &WorkflowAgentArtifactReceipt,
    state: &WorkflowStateResponse,
    dirty: bool,
) -> Option<&'static str> {
    match receipt.kind {
        WorkflowAgentArtifactKind::Graph
            if !dirty && receipt.revision == Some(state.workflow.revision) =>
        {
            Some(CANVAS_PANEL_ID)
        }
        WorkflowAgentArtifactKind::Run
            if receipt.run_id.is_some()
                && receipt.run_id.as_deref() == state.snapshot.run_id.as_deref() =>
        {
            Some(CANVAS_PANEL_ID)
        }
        WorkflowAgentArtifactKind::Results
            if receipt.run_id.is_some()
                && receipt.run_id.as_deref() == state.snapshot.run_id.as_deref()
                && state.snapshot.phase == WorkflowRunPhase::Completed
                && !state.snapshot.result_stale
                && state.snapshot.result.is_some() =>
        {
            Some(RESULTS_PANEL_ID)
        }
        _ => None,
    }
}

fn agent_state_is_monotonic(
    current: &WorkflowAgentStateResponse,
    incoming: &WorkflowAgentStateResponse,
) -> bool {
    incoming.session.workflow_id == current.session.workflow_id
        && (incoming.scope_id != current.scope_id || incoming.revision >= current.revision)
}

fn agent_command_response_is_current(
    current: &WorkflowAgentStateResponse,
    incoming: &WorkflowAgentStateResponse,
    requested_scope_id: &str,
) -> bool {
    current.scope_id == requested_scope_id && incoming.scope_id == requested_scope_id
}

fn agent_conversation_change_reason(
    loaded: bool,
    connected: bool,
    catalog_available: bool,
    command: Option<WorkflowAgentCommand>,
    agent: &WorkflowAgentStateResponse,
    snapshot: &WorkflowRunSnapshot,
) -> Option<String> {
    if !loaded {
        return Some("Conversation history is still loading.".to_owned());
    }
    if !connected {
        return Some("Reconnect to Studio before changing conversations.".to_owned());
    }
    if !catalog_available {
        return Some("Configure or reconnect Spiral before changing conversations.".to_owned());
    }
    if !agent.history_available {
        return Some(agent.readiness.detail.clone());
    }
    if command.is_some() {
        return Some("Wait for the current conversation action to finish.".to_owned());
    }
    if agent.session.phase.active() {
        return Some("Stop the active Assistant turn before changing conversations.".to_owned());
    }
    if snapshot.phase.active()
        && snapshot.run_id.as_deref().is_some_and(|run_id| {
            agent
                .conversations
                .iter()
                .any(|conversation| conversation.owned_run_id.as_deref() == Some(run_id))
        })
    {
        return Some(
            "Stop the Workflow run started by the Assistant before changing conversations."
                .to_owned(),
        );
    }
    None
}

fn agent_bootstrap_is_current(
    current: &WorkflowAgentStateResponse,
    incoming: &WorkflowAgentStateResponse,
    requested_scope_id: &str,
) -> bool {
    current.scope_id == requested_scope_id || incoming.scope_id == current.scope_id
}

fn initial_state() -> WorkflowStateResponse {
    let workflow = WorkflowSpec::default();
    WorkflowStateResponse {
        scope_id: String::new(),
        validation: workflow.validate(),
        snapshot: WorkflowRunSnapshot::idle_for(&workflow),
        workflow,
    }
}

fn server_state_is_monotonic(
    current: &WorkflowStateResponse,
    incoming: &WorkflowStateResponse,
) -> bool {
    if incoming.snapshot.workflow_id != incoming.workflow.id
        || incoming.snapshot.workflow_revision != incoming.workflow.revision
    {
        return false;
    }
    if incoming.scope_id != current.scope_id {
        return true;
    }
    if incoming.workflow.id != current.workflow.id
        || incoming.workflow.revision < current.workflow.revision
    {
        return false;
    }
    incoming.workflow.revision > current.workflow.revision
        || snapshot_is_monotonic(&current.snapshot, &incoming.snapshot)
}

fn relations_are_current(
    current_scope_id: &str,
    requested_scope_id: &str,
    incoming: &WorkflowRelationsResponse,
) -> bool {
    current_scope_id == requested_scope_id && incoming.scope_id == requested_scope_id
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

fn insert_siglip_model(workflow: &mut WorkflowSpec) -> Option<String> {
    let result_edge_index = workflow.edges.iter().position(|edge| {
        let source = workflow.nodes.iter().find(|node| node.id == edge.source);
        let target = workflow.nodes.iter().find(|node| node.id == edge.target);
        source.is_some_and(|node| node.config.kind() == WorkflowNodeKind::Query)
            && target.is_some_and(|node| node.config.kind() == WorkflowNodeKind::Result)
    })?;
    let result_edge = workflow.edges[result_edge_index].clone();
    let candidate_id = result_edge.source.clone();
    let source_id = workflow
        .edges
        .iter()
        .find(|edge| edge.target == candidate_id)
        .and_then(|edge| {
            workflow
                .nodes
                .iter()
                .find(|node| node.id == edge.source)
                .filter(|node| node.config.kind() == WorkflowNodeKind::Table)
                .map(|_| edge.source.clone())
        })?;
    let source_relation = workflow
        .nodes
        .iter()
        .find(|node| node.id == source_id)
        .and_then(|node| match &node.config {
            WorkflowNodeConfig::Table { relation } => Some(relation.clone()),
            _ => None,
        })?;
    workflow.edges.remove(result_edge_index);
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
    let mut query_suffix = 1_u32;
    let final_query_id = loop {
        let candidate = if query_suffix == 1 {
            "feature_query".to_owned()
        } else {
            format!("feature_query_{query_suffix}")
        };
        if !workflow.nodes.iter().any(|node| node.id == candidate) {
            break candidate;
        }
        query_suffix += 1;
    };
    let insert_at = workflow
        .nodes
        .iter()
        .position(|node| node.id == result_edge.target)
        .unwrap_or(workflow.nodes.len());
    workflow.nodes.insert(
        insert_at,
        spiral_robotics_api::workflow::WorkflowNodeSpec {
            id: node_id.clone(),
            name: "Backfill visual features".to_owned(),
            config: WorkflowNodeConfig::Model {
                model_id: SIGLIP_BASE_MODEL_ID.to_owned(),
                features: vec![WorkflowFeatureDefinition {
                    name: String::new(),
                    description: String::new(),
                    concept: String::new(),
                    applicability: WorkflowFeatureApplicability {
                        source_relation,
                        media_field: "image".to_owned(),
                        stream_ids: Vec::new(),
                    },
                }],
                feature_cache_relation: None,
            },
        },
    );
    workflow.nodes.insert(
        insert_at + 1,
        spiral_robotics_api::workflow::WorkflowNodeSpec {
            id: final_query_id.clone(),
            name: "Join candidates and features".to_owned(),
            config: WorkflowNodeConfig::Query {
                sql: feature_join_sql(&candidate_id, &node_id, &[]),
                max_rows: 100,
            },
        },
    );
    workflow.edges.extend([
        WorkflowEdgeSpec {
            source: candidate_id.clone(),
            target: node_id.clone(),
        },
        WorkflowEdgeSpec {
            source: candidate_id.clone(),
            target: final_query_id.clone(),
        },
        WorkflowEdgeSpec {
            source: node_id.clone(),
            target: final_query_id.clone(),
        },
        WorkflowEdgeSpec {
            source: final_query_id,
            target: result_edge.target,
        },
    ]);
    Some(node_id)
}

fn remove_siglip_model(workflow: &mut WorkflowSpec, node_id: &str) -> Option<String> {
    let model_index = workflow
        .nodes
        .iter()
        .position(|node| node.id == node_id && node.config.kind() == WorkflowNodeKind::Model)?;
    let candidate_id = workflow
        .edges
        .iter()
        .find(|edge| edge.target == node_id)
        .map(|edge| edge.source.clone())?;
    let final_query_id = workflow
        .edges
        .iter()
        .find(|edge| edge.source == node_id)
        .map(|edge| edge.target.clone())?;
    let result_id = workflow
        .edges
        .iter()
        .find(|edge| edge.source == final_query_id)
        .map(|edge| edge.target.clone())?;
    let query_is_upstream = workflow
        .nodes
        .iter()
        .any(|node| node.id == candidate_id && node.config.kind() == WorkflowNodeKind::Query);
    let query_is_downstream = workflow
        .nodes
        .iter()
        .any(|node| node.id == final_query_id && node.config.kind() == WorkflowNodeKind::Query);
    if !query_is_upstream || !query_is_downstream {
        return None;
    }

    workflow.nodes.remove(model_index);
    workflow.nodes.retain(|node| node.id != final_query_id);
    workflow.edges.retain(|edge| {
        edge.source != node_id
            && edge.target != node_id
            && edge.source != final_query_id
            && edge.target != final_query_id
    });
    workflow.edges.push(WorkflowEdgeSpec {
        source: candidate_id.clone(),
        target: result_id,
    });
    Some(candidate_id)
}

fn model_features(workflow: &WorkflowSpec, model_id: &str) -> Vec<WorkflowFeatureDefinition> {
    workflow
        .nodes
        .iter()
        .find(|node| node.id == model_id)
        .and_then(|node| match &node.config {
            WorkflowNodeConfig::Model { features, .. } => Some(features.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn refresh_generated_feature_query(
    workflow: &mut WorkflowSpec,
    model_id: &str,
    features: &[WorkflowFeatureDefinition],
) {
    let Some(candidate_id) = workflow
        .edges
        .iter()
        .find(|edge| edge.target == model_id)
        .map(|edge| edge.source.clone())
    else {
        return;
    };
    let Some(final_query_id) = workflow
        .edges
        .iter()
        .find(|edge| edge.source == model_id)
        .map(|edge| edge.target.clone())
    else {
        return;
    };
    if let Some(query) = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == final_query_id)
        && let WorkflowNodeConfig::Query { sql, .. } = &mut query.config
    {
        *sql = feature_join_sql(&candidate_id, model_id, features);
    }
}

/// Keep deterministic server-owned Model fields visible and valid while the
/// user edits its one authoritative source Table. The backend repeats this
/// normalization at admission; the browser never chooses a cache relation.
fn normalize_draft_model_sources(workflow: &mut WorkflowSpec) {
    let incoming = workflow.edges.iter().fold(
        BTreeMap::<String, Vec<String>>::new(),
        |mut incoming, edge| {
            incoming
                .entry(edge.target.clone())
                .or_default()
                .push(edge.source.clone());
            incoming
        },
    );
    let nodes = workflow
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.config.clone()))
        .collect::<BTreeMap<_, _>>();
    for node in &mut workflow.nodes {
        let WorkflowNodeConfig::Model {
            features,
            feature_cache_relation,
            ..
        } = &mut node.config
        else {
            continue;
        };
        let mut sources = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut pending = incoming.get(&node.id).cloned().unwrap_or_default();
        while let Some(upstream_id) = pending.pop() {
            if !visited.insert(upstream_id.clone()) {
                continue;
            }
            match nodes.get(&upstream_id) {
                Some(WorkflowNodeConfig::Table { relation }) if !relation.is_empty() => {
                    sources.insert(relation.clone());
                }
                Some(_) => pending.extend(incoming.get(&upstream_id).cloned().unwrap_or_default()),
                None => {}
            }
        }
        let source = (sources.len() == 1)
            .then(|| sources.into_iter().next())
            .flatten();
        *feature_cache_relation = source.as_deref().map(derived_feature_cache_relation);
        for feature in features {
            feature.applicability.source_relation = source.clone().unwrap_or_default();
        }
    }
}

fn feature_join_sql(
    candidate_id: &str,
    model_id: &str,
    features: &[WorkflowFeatureDefinition],
) -> String {
    let feature_projection = features
        .iter()
        .filter(|feature| !feature.name.is_empty())
        .map(|feature| format!(", features.{}", feature.name))
        .collect::<String>();
    format!(
        "SELECT candidates.*{feature_projection} FROM {{{{node:{candidate_id}}}}} candidates JOIN {{{{node:{model_id}}}}} features ON candidates.episode_id = features.episode_id AND candidates.stream_id = features.stream_id AND candidates.segment_index = features.segment_index AND candidates.frame_index = features.frame_index AND candidates.episode_time_ns = features.episode_time_ns"
    )
}

fn topology_mutation_blocked(phase: WorkflowRunPhase, command: Option<WorkflowCommand>) -> bool {
    phase.active() || command.is_some()
}

fn revision_conflict(draft: &WorkflowSpec, server: &WorkflowSpec) -> bool {
    draft != server && draft.revision != server.revision
}

fn graph_nodes(
    workflow: &WorkflowSpec,
    snapshot: &WorkflowRunSnapshot,
    relations: &[WorkflowRelation],
) -> Vec<Node<WorkflowNodeData>> {
    // Keep the fallback coordinates aligned with the canvas' layered-layout
    // geometry. The flow handle may not be ready for the first mount-time
    // layout pass, and widely spaced fallback ranks force Fit against the
    // readability-preserving minimum zoom at the 1024 px desktop floor.
    const FIRST_RANK_X: f64 = 24.0;
    // Smooth-step edges reserve a 20-unit stub at both endpoints. The
    // fallback must therefore keep more than 40 units between cards even
    // when the first mount-time auto-layout is not ready yet.
    const RANK_STEP_X: f64 = 224.0; // 10.5 rem card + 56 flow-unit rank gap.
    let relation_index = relations
        .iter()
        .map(|relation| (relation.name.as_str(), relation))
        .collect::<BTreeMap<_, _>>();
    let phase_index = snapshot
        .nodes
        .iter()
        .map(|node| (node.node_id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let ranks = fallback_node_ranks(workflow);
    let mut rank_indices = BTreeMap::<usize, usize>::new();
    workflow
        .nodes
        .iter()
        .map(|node| {
            let rank = ranks.get(node.id.as_str()).copied().unwrap_or_default();
            let rank_index = rank_indices.entry(rank).or_default();
            let position = (
                FIRST_RANK_X + RANK_STEP_X * rank as f64,
                30.0 + *rank_index as f64 * 178.0,
            );
            *rank_index += 1;
            let type_name = match node.config.kind() {
                WorkflowNodeKind::Table => "table",
                WorkflowNodeKind::Model => "model",
                WorkflowNodeKind::Query => "query",
                WorkflowNodeKind::Result => "result",
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

fn fallback_node_ranks(workflow: &WorkflowSpec) -> BTreeMap<&str, usize> {
    let mut ranks = workflow
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), 0_usize))
        .collect::<BTreeMap<_, _>>();
    let order = workflow.topological_order().unwrap_or_else(|_| {
        // Invalid drafts are not runnable, but still need deterministic finite
        // fallback geometry while validation explains the graph error.
        workflow.nodes.iter().map(|node| node.id.clone()).collect()
    });
    for source in order {
        let source_rank = ranks.get(source.as_str()).copied().unwrap_or_default();
        for edge in workflow.edges.iter().filter(|edge| edge.source == source) {
            if let Some(target_rank) = ranks.get_mut(edge.target.as_str()) {
                *target_rank = (*target_rank).max(source_rank.saturating_add(1));
            }
        }
    }
    ranks
}

fn node_data(
    node: &spiral_robotics_api::workflow::WorkflowNodeSpec,
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
                feature_cache_relation: None,
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
                features: Vec::new(),
                progress,
                quarantine,
                model: model_snapshot,
                warnings,
            }
        }
        WorkflowNodeConfig::Model {
            model_id,
            features,
            feature_cache_relation,
            ..
        } => WorkflowNodeData {
            kind: WorkflowNodeKind::Model,
            fields: features
                .iter()
                .map(|feature| feature.name.trim())
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
            relation: None,
            feature_cache_relation: feature_cache_relation.clone(),
            sql: None,
            description:
                "Pinned SigLIP backfills registered features for a SQL-defined candidate cohort."
                    .to_owned(),
            phase,
            detail,
            error,
            model_id: Some(model_id.clone()),
            features: features.clone(),
            progress,
            quarantine,
            model: model_snapshot,
            warnings,
        },
        WorkflowNodeConfig::Query { sql, .. } => WorkflowNodeData {
            kind: WorkflowNodeKind::Query,
            fields: Vec::new(),
            relation: None,
            feature_cache_relation: None,
            sql: Some(sql.clone()),
            description: "Bounded read-only Spiral SQL with server-owned relation placeholders."
                .to_owned(),
            phase,
            detail,
            error,
            model_id: None,
            features: Vec::new(),
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
            feature_cache_relation: None,
            sql: None,
            description: "Terminal media-aware query result.".to_owned(),
            phase,
            detail,
            error,
            model_id: None,
            features: Vec::new(),
            progress,
            quarantine,
            model: model_snapshot,
            warnings,
        },
    }
}

fn graph_edges(edges: &[WorkflowEdgeSpec], snapshot: &WorkflowRunSnapshot) -> Vec<Edge> {
    let active_targets = snapshot
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.phase,
                WorkflowNodePhase::Validating
                    | WorkflowNodePhase::Loading
                    | WorkflowNodePhase::Running
                    | WorkflowNodePhase::Cancelling
            )
        })
        .map(|node| node.node_id.as_str())
        .collect::<BTreeSet<_>>();
    edges
        .iter()
        .map(|edge| {
            let active = active_targets.contains(edge.target.as_str());
            let edge = Edge::new(edge.source.clone(), edge.target.clone())
                .kind(EdgeKind::SmoothStep)
                .animated(active);
            if active {
                edge.class("workflow-edge-running")
            } else {
                edge
            }
        })
        .collect()
}

fn empty_workflow_draft(current: &WorkflowSpec) -> WorkflowSpec {
    WorkflowSpec {
        id: current.id.clone(),
        name: "Untitled workflow".to_owned(),
        revision: current.revision,
        nodes: Vec::new(),
        edges: Vec::new(),
    }
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
        assert!(!state.workflow.has_model());
    }

    #[test]
    fn graph_projection_preserves_saved_topology() {
        let workflow = WorkflowSpec::default();
        let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        let nodes = graph_nodes(&workflow, &snapshot, &[]);
        let edges = graph_edges(&workflow.edges, &snapshot);
        assert_eq!(nodes.len(), 3);
        assert_eq!(edges.len(), 2);
        assert_eq!(nodes[0].id, "table");
        assert_eq!(nodes[2].id, "result");
        assert_eq!(nodes[1].position.x - nodes[0].position.x, 224.0);
        assert_eq!(nodes[2].position.x - nodes[1].position.x, 224.0);
    }

    #[test]
    fn starting_fresh_preserves_server_identity_but_removes_all_topology() {
        let workflow = WorkflowSpec {
            id: "inspection".to_owned(),
            revision: 17,
            ..WorkflowSpec::default()
        };

        let empty = empty_workflow_draft(&workflow);

        assert_eq!(empty.id, "inspection");
        assert_eq!(empty.revision, 17);
        assert_eq!(empty.name, "Untitled workflow");
        assert!(empty.nodes.is_empty());
        assert!(empty.edges.is_empty());
        assert!(!empty.validate().valid());
        assert!(graph_nodes(&empty, &WorkflowRunSnapshot::idle_for(&empty), &[]).is_empty());
        assert!(graph_edges(&empty.edges, &WorkflowRunSnapshot::idle_for(&empty)).is_empty());
    }

    #[test]
    fn only_edges_feeding_the_active_node_animate() {
        let workflow = WorkflowSpec::default();
        let mut snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        snapshot.nodes[1].phase = WorkflowNodePhase::Running;

        let edges = graph_edges(&workflow.edges, &snapshot);

        assert!(edges[0].animated);
        assert_eq!(edges[0].class.as_deref(), Some("workflow-edge-running"));
        assert!(!edges[1].animated);
        assert!(edges[1].class.is_none());
    }

    #[test]
    fn inserting_siglip_makes_candidate_backfill_and_final_join_visible() {
        let mut workflow = WorkflowSpec::default();
        let WorkflowNodeConfig::Table { relation } = &mut workflow.nodes[0].config else {
            unreachable!()
        };
        *relation = "pick_place.camera_frames".to_owned();
        let node_id = insert_siglip_model(&mut workflow).unwrap();
        assert_eq!(node_id, "siglip");
        assert_eq!(workflow.nodes.len(), 5);
        assert_eq!(workflow.edges.len(), 5);
        assert_eq!(workflow.edges[0].source, "table");
        assert_eq!(workflow.edges[0].target, "query");
        assert!(
            workflow
                .edges
                .iter()
                .any(|edge| edge.source == "query" && edge.target == "siglip")
        );
        assert!(
            workflow
                .edges
                .iter()
                .any(|edge| edge.source == "query" && edge.target == "feature_query")
        );
        assert!(
            workflow
                .edges
                .iter()
                .any(|edge| edge.source == "siglip" && edge.target == "feature_query")
        );
        assert!(
            workflow
                .edges
                .iter()
                .any(|edge| edge.source == "feature_query" && edge.target == "result")
        );
        let WorkflowNodeConfig::Model { features, .. } = &workflow
            .nodes
            .iter()
            .find(|node| node.id == "siglip")
            .unwrap()
            .config
        else {
            unreachable!()
        };
        assert_eq!(features[0].applicability.media_field, "image");
        assert_eq!(
            features[0].applicability.source_relation,
            "pick_place.camera_frames"
        );
        let WorkflowNodeConfig::Query { sql, .. } = &workflow
            .nodes
            .iter()
            .find(|node| node.id == "feature_query")
            .unwrap()
            .config
        else {
            unreachable!()
        };
        assert!(sql.contains("{{node:query}}"));
        assert!(sql.contains("{{node:siglip}}"));
    }

    #[test]
    fn model_fallback_uses_five_topology_ranks_with_two_table_roots() {
        let mut workflow = WorkflowSpec::default();
        let WorkflowNodeConfig::Table { relation } = &mut workflow.nodes[0].config else {
            unreachable!()
        };
        *relation = "pick_place.camera_frames".to_owned();
        insert_siglip_model(&mut workflow).unwrap();
        let mut joint_state = workflow.nodes[0].clone();
        joint_state.id = "joint_state".to_owned();
        joint_state.name = "Joint state".to_owned();
        joint_state.config = WorkflowNodeConfig::Table {
            relation: "pick_place.joint_state".to_owned(),
        };
        workflow.nodes.push(joint_state);
        workflow.edges.push(WorkflowEdgeSpec {
            source: "joint_state".to_owned(),
            target: "feature_query".to_owned(),
        });

        let ranks = fallback_node_ranks(&workflow);
        assert_eq!(ranks["table"], 0);
        assert_eq!(ranks["joint_state"], 0);
        assert_eq!(ranks["query"], 1);
        assert_eq!(ranks["siglip"], 2);
        assert_eq!(ranks["feature_query"], 3);
        assert_eq!(ranks["result"], 4);

        let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        let nodes = graph_nodes(&workflow, &snapshot, &[]);
        let positions = nodes
            .iter()
            .map(|node| (node.id.as_str(), node.position.x))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(positions["query"] - positions["table"], 224.0);
        assert_eq!(positions["siglip"] - positions["query"], 224.0);
        assert_eq!(positions["feature_query"] - positions["siglip"], 224.0);
        assert_eq!(positions["result"] - positions["feature_query"], 224.0);
    }

    #[test]
    fn rejected_model_insertion_never_mutates_a_chained_query_graph() {
        let mut workflow = WorkflowSpec::default();
        workflow
            .nodes
            .push(spiral_robotics_api::workflow::WorkflowNodeSpec {
                id: "second_query".to_owned(),
                name: "Second query".to_owned(),
                config: WorkflowNodeConfig::Query {
                    sql: "SELECT * FROM {{node:query}}".to_owned(),
                    max_rows: 100,
                },
            });
        workflow
            .edges
            .retain(|edge| !(edge.source == "query" && edge.target == "result"));
        workflow.edges.extend([
            WorkflowEdgeSpec {
                source: "query".to_owned(),
                target: "second_query".to_owned(),
            },
            WorkflowEdgeSpec {
                source: "second_query".to_owned(),
                target: "result".to_owned(),
            },
        ]);
        let before = workflow.clone();

        assert!(insert_siglip_model(&mut workflow).is_none());
        assert_eq!(workflow, before);
    }

    #[test]
    fn configured_source_and_feature_make_the_manual_model_draft_applicable() {
        let mut workflow = WorkflowSpec::default();
        let model_id = insert_siglip_model(&mut workflow).unwrap();
        let blank_report = workflow.validate();
        assert_eq!(
            blank_report
                .errors
                .iter()
                .filter(|issue| issue.code == "incomplete_feature_definition")
                .count(),
            1
        );
        assert!(!blank_report.errors.iter().any(|issue| matches!(
            issue.code.as_str(),
            "invalid_feature_name" | "invalid_feature_description" | "invalid_feature_concept"
        )));
        let WorkflowNodeConfig::Table { relation } = &mut workflow.nodes[0].config else {
            unreachable!()
        };
        *relation = "pick_place.camera_frames".to_owned();
        let WorkflowNodeConfig::Model { features, .. } = &mut workflow
            .nodes
            .iter_mut()
            .find(|node| node.id == model_id)
            .unwrap()
            .config
        else {
            unreachable!()
        };
        features[0].name = "is_raining_feature".to_owned();
        features[0].description = "Probability that the frame shows rain".to_owned();
        features[0].concept = "raining".to_owned();
        let features = features.clone();
        refresh_generated_feature_query(&mut workflow, &model_id, &features);
        normalize_draft_model_sources(&mut workflow);

        assert!(workflow.validate().valid());
        let WorkflowNodeConfig::Model {
            features,
            feature_cache_relation,
            ..
        } = &workflow
            .nodes
            .iter()
            .find(|node| node.id == model_id)
            .unwrap()
            .config
        else {
            unreachable!()
        };
        assert_eq!(
            features[0].applicability.source_relation,
            "pick_place.camera_frames"
        );
        assert_eq!(
            feature_cache_relation.as_deref(),
            Some(derived_feature_cache_relation("pick_place.camera_frames").as_str())
        );
        let WorkflowNodeConfig::Query { sql, .. } = &workflow
            .nodes
            .iter()
            .find(|node| node.id == "feature_query")
            .unwrap()
            .config
        else {
            unreachable!()
        };
        assert!(sql.contains("features.is_raining_feature"));
    }

    #[test]
    fn removing_siglip_restores_the_candidate_query_as_terminal() {
        let mut workflow = WorkflowSpec::default();
        insert_siglip_model(&mut workflow).unwrap();

        let selected = remove_siglip_model(&mut workflow, "siglip").unwrap();

        assert_eq!(selected, "query");
        assert_eq!(workflow.nodes.len(), 3);
        assert_eq!(workflow.edges.len(), 2);
        assert!(
            workflow
                .edges
                .iter()
                .any(|edge| edge.source == "table" && edge.target == "query")
        );
        assert!(
            workflow
                .edges
                .iter()
                .any(|edge| edge.source == "query" && edge.target == "result")
        );
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
        let node = spiral_robotics_api::workflow::WorkflowNodeSpec {
            id: "siglip".to_owned(),
            name: "SigLIP scores".to_owned(),
            config: WorkflowNodeConfig::Model {
                model_id: SIGLIP_BASE_MODEL_ID.to_owned(),
                features: vec![WorkflowFeatureDefinition {
                    name: "is_red_cube_feature".to_owned(),
                    description: "Probability that a red cube is visible".to_owned(),
                    concept: "red cube".to_owned(),
                    applicability: WorkflowFeatureApplicability {
                        source_relation: "pick_place.camera_frames".to_owned(),
                        media_field: "image".to_owned(),
                        stream_ids: Vec::new(),
                    },
                }],
                feature_cache_relation: Some(
                    "workflow.features_pick_place_camera_frames".to_owned(),
                ),
            },
        };
        let snapshot = WorkflowNodeSnapshot {
            node_id: node.id.clone(),
            phase: WorkflowNodePhase::CompleteWithWarnings,
            detail: "Complete with warnings".to_owned(),
            progress: Some(WorkflowNodeProgress {
                completed: 8,
                total: 8,
                unit: "feature cells".to_owned(),
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
        assert_eq!(projected.fields, ["is_red_cube_feature"]);
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
    fn assistant_receipt_navigation_never_opens_stale_artifacts() {
        let mut state = WorkflowStateResponse::default();
        state.workflow.revision = 4;
        state.snapshot.workflow_revision = 4;
        let graph = WorkflowAgentArtifactReceipt {
            kind: WorkflowAgentArtifactKind::Graph,
            label: "View graph".to_owned(),
            revision: Some(4),
            run_id: None,
            row_count: None,
        };
        assert_eq!(
            receipt_panel_id(&graph, &state, false),
            Some(CANVAS_PANEL_ID)
        );
        assert_eq!(receipt_panel_id(&graph, &state, true), None);
        let mut old_graph = graph.clone();
        old_graph.revision = Some(3);
        assert_eq!(receipt_panel_id(&old_graph, &state, false), None);

        state.snapshot.run_id = Some("run-4".to_owned());
        state.snapshot.phase = WorkflowRunPhase::Completed;
        state.snapshot.result = Some(spiral_robotics_api::api::SpiralQueryResponse {
            columns: Vec::new(),
            rows: Vec::new(),
            truncated: false,
            elapsed_ms: 1,
        });
        let results = WorkflowAgentArtifactReceipt {
            kind: WorkflowAgentArtifactKind::Results,
            label: "View results".to_owned(),
            revision: None,
            run_id: Some("run-4".to_owned()),
            row_count: Some(0),
        };
        assert_eq!(
            receipt_panel_id(&results, &state, false),
            Some(RESULTS_PANEL_ID),
            "zero-row completed results remain authoritative"
        );
        state.snapshot.result_stale = true;
        assert_eq!(receipt_panel_id(&results, &state, false), None);
        state.snapshot.result_stale = false;
        let mut old_results = results;
        old_results.run_id = Some("run-3".to_owned());
        assert_eq!(receipt_panel_id(&old_results, &state, false), None);
    }

    #[test]
    fn slower_bootstrap_response_cannot_regress_newer_sse_state() {
        let mut current = WorkflowStateResponse {
            scope_id: "project-a-scope".to_owned(),
            ..WorkflowStateResponse::default()
        };
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

        let project_b = WorkflowStateResponse {
            scope_id: "project-b-scope".to_owned(),
            ..WorkflowStateResponse::default()
        };
        assert!(
            server_state_is_monotonic(&current, &project_b),
            "a new project scope owns an independent revision sequence"
        );

        let project_a_relations = WorkflowRelationsResponse {
            scope_id: "project-a-scope".to_owned(),
            ..Default::default()
        };
        let project_b_relations = WorkflowRelationsResponse {
            scope_id: "project-b-scope".to_owned(),
            ..Default::default()
        };
        assert!(!relations_are_current(
            "project-b-scope",
            "project-a-scope",
            &project_a_relations
        ));
        assert!(relations_are_current(
            "project-b-scope",
            "project-b-scope",
            &project_b_relations
        ));
    }

    #[test]
    fn slower_agent_bootstrap_cannot_regress_streamed_history() {
        let current = WorkflowAgentStateResponse {
            scope_id: "project-a-scope".to_owned(),
            revision: 8,
            ..WorkflowAgentStateResponse::default()
        };
        let mut stale = current.clone();
        stale.revision = 7;
        assert!(!agent_state_is_monotonic(&current, &stale));
        let mut next = current.clone();
        next.revision = 9;
        assert!(agent_state_is_monotonic(&current, &next));
        next.session.workflow_id = "other".to_owned();
        assert!(!agent_state_is_monotonic(&current, &next));

        let mut project_b = current.clone();
        project_b.scope_id = "project-b-scope".to_owned();
        project_b.revision = 1;
        assert!(
            agent_state_is_monotonic(&current, &project_b),
            "a new project scope owns an independent revision sequence"
        );
        assert!(!agent_bootstrap_is_current(
            &project_b,
            &current,
            "project-a-scope"
        ));
        assert!(agent_bootstrap_is_current(
            &project_b,
            &project_b,
            "project-a-scope"
        ));
        let mut command_response = current.clone();
        command_response.revision += 1;
        assert!(agent_command_response_is_current(
            &current,
            &command_response,
            "project-a-scope"
        ));
        assert!(!agent_command_response_is_current(
            &project_b,
            &command_response,
            "project-a-scope"
        ));
    }

    #[test]
    fn conversation_switching_is_gated_only_by_authoritative_active_work() {
        let mut agent = WorkflowAgentStateResponse {
            history_available: true,
            ..WorkflowAgentStateResponse::default()
        };
        agent.conversations.push(
            spiral_robotics_api::workflow::WorkflowAgentConversationSummary {
                conversation_id: "conversation-a".to_owned(),
                title: "Inspect relations".to_owned(),
                ..Default::default()
            },
        );
        let mut snapshot = WorkflowRunSnapshot::default();
        assert!(
            agent_conversation_change_reason(true, true, true, None, &agent, &snapshot).is_none()
        );

        assert_eq!(
            agent_conversation_change_reason(true, true, false, None, &agent, &snapshot).as_deref(),
            Some("Configure or reconnect Spiral before changing conversations.")
        );

        agent.session.phase = spiral_robotics_api::workflow::WorkflowAgentPhase::Responding;
        assert_eq!(
            agent_conversation_change_reason(true, true, true, None, &agent, &snapshot).as_deref(),
            Some("Stop the active Assistant turn before changing conversations.")
        );
        agent.session.phase = spiral_robotics_api::workflow::WorkflowAgentPhase::Idle;

        snapshot.phase = WorkflowRunPhase::Running;
        snapshot.run_id = Some("run-agent".to_owned());
        agent.conversations[0].owned_run_id = Some("run-agent".to_owned());
        assert_eq!(
            agent_conversation_change_reason(true, true, true, None, &agent, &snapshot).as_deref(),
            Some("Stop the Workflow run started by the Assistant before changing conversations.")
        );
        agent.conversations[0].owned_run_id = Some("other-run".to_owned());
        assert!(
            agent_conversation_change_reason(true, true, true, None, &agent, &snapshot).is_none(),
            "a manual or unrelated run does not falsely claim Assistant conversation ownership"
        );
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
