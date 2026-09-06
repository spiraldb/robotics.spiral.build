//! Shell-owned Workflow editor state and backend commands.
//!
//! Canvas geometry and inspector selection are browser interaction state. The
//! saved graph, validation, execution phases, and result payload are backend
//! truth, refreshed exclusively by the project-scoped queries.

use crate::client::project::ProjectClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use dioxus::prelude::*;
use dioxus_flow::{Edge, EdgeKind, Node, Side};
use dioxus_workbench::PanelId;
use spiral_robotics_common::relations::{CatalogRelation, RelationCatalog};
use spiral_robotics_common::workflow::{
    WorkflowAgentArtifactReceipt, WorkflowAgentCommand as AgentRequest, WorkflowAgentSnapshot,
    WorkflowCommand as WorkflowRequest, WorkflowEdgeSpec, WorkflowFeatureApplicability,
    WorkflowFeatureDefinition, WorkflowModelRunMetadata, WorkflowNodeConfig, WorkflowNodePhase,
    WorkflowNodeProgress, WorkflowNodeSnapshot, WorkflowQuarantineSummary, WorkflowRunPhase,
    WorkflowRunSnapshot, WorkflowSnapshot, WorkflowSpec, WorkflowValidationReport,
};

use crate::shell::notifications::MessageSource;
use crate::shell::state::Studio;

pub(crate) const ASSISTANT_PANEL_ID: &str = "workflow-assistant";
pub(crate) const NODE_PANEL_ID: &str = "workflow-node";
pub(crate) const CANVAS_PANEL_ID: &str = "workflow-canvas";
pub(crate) const RESULTS_PANEL_ID: &str = "workflow-results";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum WorkflowNodeKind {
    Table,
    Model,
    Query,
    Result,
}

impl From<&WorkflowNodeConfig> for WorkflowNodeKind {
    fn from(config: &WorkflowNodeConfig) -> Self {
        match config {
            WorkflowNodeConfig::Table { .. } => Self::Table,
            WorkflowNodeConfig::Model { .. } => Self::Model,
            WorkflowNodeConfig::Query { .. } => Self::Query,
            WorkflowNodeConfig::Result => Self::Result,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WorkflowNodeData {
    kind: WorkflowNodeKind,
    description: String,
    source: WorkflowNodeSource,
    execution: WorkflowNodeExecution,
    model: WorkflowNodeModel,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct WorkflowNodeSource {
    fields: Vec<String>,
    relation: Option<String>,
    feature_cache_relation: Option<String>,
    sql: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct WorkflowNodeExecution {
    phase: WorkflowNodePhase,
    detail: String,
    error: Option<String>,
    progress: Option<WorkflowNodeProgress>,
    quarantine: WorkflowQuarantineSummary,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct WorkflowNodeModel {
    features: Vec<WorkflowFeatureDefinition>,
    metadata: Option<WorkflowModelRunMetadata>,
}

impl WorkflowNodeData {
    pub(crate) const fn kind(&self) -> WorkflowNodeKind {
        self.kind
    }

    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    pub(crate) fn fields(&self) -> &[String] {
        &self.source.fields
    }

    pub(crate) fn relation(&self) -> Option<&str> {
        self.source.relation.as_deref()
    }

    pub(crate) fn feature_cache_relation(&self) -> Option<&str> {
        self.source.feature_cache_relation.as_deref()
    }

    pub(crate) fn sql(&self) -> Option<&str> {
        self.source.sql.as_deref()
    }

    pub(crate) fn detail(&self) -> &str {
        &self.execution.detail
    }

    #[cfg(test)]
    pub(crate) const fn phase(&self) -> WorkflowNodePhase {
        self.execution.phase
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.execution.error.as_deref()
    }

    pub(crate) fn warnings(&self) -> &[String] {
        &self.execution.warnings
    }

    pub(crate) const fn progress(&self) -> Option<&WorkflowNodeProgress> {
        self.execution.progress.as_ref()
    }

    pub(crate) const fn quarantine(&self) -> &WorkflowQuarantineSummary {
        &self.execution.quarantine
    }

    pub(crate) fn features(&self) -> &[WorkflowFeatureDefinition] {
        &self.model.features
    }

    pub(crate) const fn model_run(&self) -> Option<&WorkflowModelRunMetadata> {
        self.model.metadata.as_ref()
    }

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
        match self.execution.phase {
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
        match self.execution.phase {
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
    selected_node: Option<String>,
    active_panel: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowUiAction {
    SelectNode(String),
    ClearSelection,
    ActivatePanel(String),
}

impl WorkflowUiState {
    pub(crate) fn reduce(&self, action: WorkflowUiAction) -> Self {
        let mut next = self.clone();
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowPendingCommand {
    Applying,
    Starting,
    Cancelling,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentPendingCommand {
    Sending,
    Cancelling,
    CreatingConversation,
    ActivatingConversation,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct AgentDraftKey {
    scope_id: String,
    conversation_id: String,
}

pub(crate) struct ModelFeatureTarget<'a> {
    node_id: &'a str,
    index: usize,
}

impl<'a> ModelFeatureTarget<'a> {
    pub(crate) const fn new(node_id: &'a str, index: usize) -> Self {
        Self { node_id, index }
    }
}

impl AgentDraftKey {
    pub(crate) fn new(scope_id: &str, conversation_id: &str) -> Self {
        Self {
            scope_id: scope_id.to_owned(),
            conversation_id: conversation_id.to_owned(),
        }
    }

    pub(crate) fn clear_if_unchanged(
        &self,
        drafts: &mut BTreeMap<AgentDraftKey, String>,
        submitted: &str,
    ) -> bool {
        if drafts.get(self).map(String::as_str) != Some(submitted) {
            return false;
        }
        drafts.remove(self);
        true
    }
}

#[derive(Clone, Copy)]
pub(crate) struct WorkflowController {
    graph: WorkflowGraphSignals,
    document: WorkflowDocumentSignals,
    lifecycle: WorkflowLifecycleSignals,
    assistant: WorkflowAssistantSignals,
    relation_catalog: Resource<Result<RelationCatalog, String>>,
}

#[derive(Clone, Copy)]
struct WorkflowGraphSignals {
    nodes: Signal<Vec<Node<WorkflowNodeData>>>,
    edges: Signal<Vec<Edge>>,
    ui: Signal<WorkflowUiState>,
}

#[derive(Clone, Copy)]
struct WorkflowDocumentSignals {
    server: Signal<WorkflowSnapshot>,
    draft: Signal<WorkflowSpec>,
    saved: Signal<Option<WorkflowSaveReceipt>>,
}

/// A save receipt acknowledges a submitted draft, without installing a
/// command response into the fetched server state.
#[derive(Clone)]
struct WorkflowSaveReceipt {
    submitted: WorkflowSpec,
    revision: u64,
}
impl WorkflowSaveReceipt {
    fn acknowledges(&self, draft: &WorkflowSpec, fetched: &WorkflowSpec) -> bool {
        draft == &self.submitted && fetched.revision() >= self.revision
    }
}

#[derive(Clone, Copy)]
struct WorkflowLifecycleSignals {
    loaded: Signal<bool>,
    command: Signal<Option<WorkflowPendingCommand>>,
    load_error: Signal<Option<String>>,
}

#[derive(Clone, Copy)]
struct WorkflowAssistantSignals {
    agent: Signal<WorkflowAgentSnapshot>,
    agent_loaded: Signal<bool>,
    agent_command: Signal<Option<AgentPendingCommand>>,
    agent_error: Signal<Option<String>>,
    agent_drafts: Signal<BTreeMap<AgentDraftKey, String>>,
}

impl WorkflowController {
    pub(crate) fn use_controller(
        relation_catalog: Resource<Result<RelationCatalog, String>>,
    ) -> Self {
        let initial = WorkflowSnapshot::from_workflow("", WorkflowSpec::default());
        let initial_spec = initial.workflow().to_owned();
        let initial_nodes = initial_spec.graph_nodes(initial.snapshot(), &[]);
        let initial_edges = initial_spec.graph_edges(initial.snapshot());
        let controller = Self {
            graph: WorkflowGraphSignals {
                nodes: use_signal(move || initial_nodes),
                edges: use_signal(move || initial_edges),
                ui: use_signal(WorkflowUiState::default),
            },
            document: WorkflowDocumentSignals {
                server: use_signal(move || initial),
                draft: use_signal(move || initial_spec),
                saved: use_signal(|| None),
            },
            lifecycle: WorkflowLifecycleSignals {
                loaded: use_signal(|| false),
                command: use_signal(|| None),
                load_error: use_signal(|| None),
            },
            assistant: WorkflowAssistantSignals {
                agent: use_signal(WorkflowAgentSnapshot::default),
                agent_loaded: use_signal(|| false),
                agent_command: use_signal(|| None),
                agent_error: use_signal(|| None),
                agent_drafts: use_signal(BTreeMap::new),
            },
            relation_catalog,
        };
        use_effect(move || {
            let _catalog_result = relation_catalog.read_unchecked().to_owned();
            controller.sync_graph();
        });
        controller
    }

    pub(crate) const fn nodes(self) -> Signal<Vec<Node<WorkflowNodeData>>> {
        self.graph.nodes
    }

    pub(crate) const fn edges(self) -> Signal<Vec<Edge>> {
        self.graph.edges
    }

    pub(crate) fn spec(&self) -> WorkflowSpec {
        self.document.draft.read().to_owned()
    }

    pub(crate) fn snapshot(&self) -> WorkflowRunSnapshot {
        self.document.server.read().snapshot().to_owned()
    }

    pub(crate) fn accepted_state(&self) -> Option<WorkflowSnapshot> {
        self.lifecycle
            .loaded
            .read()
            .then(|| self.document.server.read().to_owned())
    }

    pub(crate) fn relations(&self) -> Vec<CatalogRelation> {
        self.scoped_relation_catalog()
            .map(|catalog| catalog.relations().to_owned())
            .unwrap_or_default()
    }

    pub(crate) fn relation_limitation(&self) -> Option<String> {
        if let Some(error) = self
            .relation_catalog
            .read_unchecked()
            .as_ref()
            .and_then(|result| result.as_ref().err())
        {
            return Some(format!(
                "The Studio relation catalog is unavailable: {error}"
            ));
        }
        self.scoped_relation_catalog()
            .and_then(|catalog| catalog.limitation().map(str::to_owned))
    }

    pub(crate) fn has_model(&self) -> bool {
        self.document.draft.read().has_model()
    }

    pub(crate) fn topology_signature(&self) -> String {
        let workflow = self.document.draft.read();
        let mut signature = workflow
            .nodes()
            .iter()
            .map(|node| node.id())
            .collect::<Vec<_>>()
            .join("|");
        for edge in workflow.edges() {
            signature.push('|');
            signature.push_str(edge.source());
            signature.push('>');
            signature.push_str(edge.target());
        }
        signature
    }

    pub(crate) fn validation(&self) -> WorkflowValidationReport {
        self.document.draft.read().validate()
    }

    pub(crate) fn loaded(&self) -> bool {
        *self.lifecycle.loaded.read()
    }

    pub(crate) fn load_error(&self) -> Option<String> {
        self.lifecycle.load_error.read().to_owned()
    }

    pub(crate) fn command(&self) -> Option<WorkflowPendingCommand> {
        *self.lifecycle.command.read()
    }

    pub(crate) fn agent_state(&self) -> WorkflowAgentSnapshot {
        self.assistant.agent.read().to_owned()
    }

    pub(crate) fn agent_loaded(&self) -> bool {
        *self.assistant.agent_loaded.read()
    }

    pub(crate) fn agent_command(&self) -> Option<AgentPendingCommand> {
        *self.assistant.agent_command.read()
    }

    pub(crate) fn agent_error(&self) -> Option<String> {
        self.assistant.agent_error.read().to_owned()
    }

    /// Shell-hoisted unsent text survives Workflow route and Settings outlet
    /// unmounts while remaining isolated by project scope and durable
    /// conversation identity.
    pub(crate) const fn agent_drafts(self) -> Signal<BTreeMap<AgentDraftKey, String>> {
        self.assistant.agent_drafts
    }

    pub(crate) fn agent_conversation_disabled_reason(
        &self,
        connected: bool,
        catalog_available: bool,
    ) -> Option<String> {
        let agent = self.agent_state();
        let snapshot = self.snapshot();
        AgentConversationChange {
            loaded: self.agent_loaded(),
            connected,
            catalog_available,
            command: self.agent_command(),
            agent: &agent,
            snapshot: &snapshot,
        }
        .reason()
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
        if agent.conversations().len() >= agent.conversation_limit() {
            return Some(format!(
                "The {0}-conversation limit is reached. Previous conversations are kept; continue one of them instead.",
                agent.conversation_limit()
            ));
        }
        None
    }

    pub(crate) fn dirty(&self) -> bool {
        *self.document.draft.read() != *self.document.server.read().workflow()
    }

    pub(crate) fn revision_conflict(&self) -> bool {
        self.document
            .draft
            .read()
            .revision_conflicts_with(self.document.server.read().workflow())
    }

    pub(crate) fn selected_node(&self) -> Option<String> {
        self.graph.ui.read().selected_node.clone()
    }

    pub(crate) fn selected_node_data(&self) -> Option<(String, String, WorkflowNodeData)> {
        let selected = self.selected_node()?;
        self.graph
            .nodes
            .read()
            .iter()
            .find(|node| node.id == selected)
            .map(|node| (node.id.clone(), node.label.clone(), node.data.clone()))
    }

    pub(crate) fn active_panel(&self) -> Option<PanelId> {
        self.graph.ui.read().active_panel.clone().map(PanelId::from)
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
        if self.agent_state().session().phase().active() || self.agent_command().is_some() {
            return Some("The Workflow Assistant is using this graph.".to_owned());
        }
        if self.snapshot().phase().active() {
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
        let draft = self.document.draft.read();
        if draft.nodes().is_empty() && draft.edges().is_empty() {
            return Some("Ask the Assistant to build this workflow before running.".to_owned());
        }
        drop(draft);
        self.validation()
            .errors()
            .first()
            .map(|issue| issue.message().to_owned())
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
        if self.command().is_some() || self.snapshot().phase().active() {
            return Some("Cancel the active run before applying changes.".to_owned());
        }
        if self.agent_state().session().phase().active() || self.agent_command().is_some() {
            return Some(
                "Wait for the active Assistant turn before applying graph changes.".to_owned(),
            );
        }
        let draft = self.document.draft.read();
        if draft.nodes().is_empty() && draft.edges().is_empty() {
            return None;
        }
        drop(draft);
        self.validation()
            .errors()
            .first()
            .map(|issue| issue.message().to_owned())
    }

    pub(crate) fn select_node(mut self, id: String) {
        if !self.graph.nodes.peek().iter().any(|node| node.id == id) {
            return;
        }
        let next = self
            .graph
            .ui
            .peek()
            .reduce(WorkflowUiAction::SelectNode(id.clone()));
        self.graph.ui.set(next);
        self.graph.nodes.with_mut(|nodes| {
            for node in nodes {
                node.selected = node.id == id;
            }
        });
    }

    pub(crate) fn clear_selection(mut self) {
        let next = self
            .graph
            .ui
            .peek()
            .reduce(WorkflowUiAction::ClearSelection);
        self.graph.ui.set(next);
        self.graph.nodes.with_mut(|nodes| {
            for node in nodes {
                node.selected = false;
            }
        });
    }

    pub(crate) fn activate_panel(mut self, id: PanelId) {
        let next = self
            .graph
            .ui
            .peek()
            .reduce(WorkflowUiAction::ActivatePanel(id.to_string()));
        self.graph.ui.set(next);
    }

    pub(crate) fn receipt_panel(&self, receipt: &WorkflowAgentArtifactReceipt) -> Option<PanelId> {
        let dirty = self.dirty();
        let server = self.document.server.read();
        receipt.panel_id(&server, dirty).map(PanelId::from)
    }

    pub(crate) fn open_agent_receipt(mut self, receipt: WorkflowAgentArtifactReceipt) {
        let Some(panel) = self.receipt_panel(&receipt) else {
            return;
        };
        let next = self
            .graph
            .ui
            .peek()
            .reduce(WorkflowUiAction::ActivatePanel(panel.to_string()));
        self.graph.ui.set(next);
    }

    pub(crate) fn reload_saved(mut self) {
        self.document
            .draft
            .set(self.document.server.peek().workflow().to_owned());
        self.lifecycle.load_error.set(None);
        self.sync_graph();
    }

    pub(crate) fn clear_workflow_draft(mut self) {
        if self
            .snapshot()
            .phase()
            .topology_mutation_blocked(self.command())
            || self.agent_state().session().phase().active()
            || self.agent_command().is_some()
        {
            return;
        }
        let empty = self.document.draft.peek().empty_draft();
        self.document.draft.set(empty);
        self.clear_selection();
        self.sync_graph();
    }

    pub(crate) fn set_table_relation(mut self, node_id: &str, relation: String) {
        let mut draft = self.document.draft.write();
        let changed = draft.edit_node(node_id, |config| {
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
            draft.normalize_model_sources();
        }
        drop(draft);
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn set_query_sql(mut self, node_id: &str, sql: String) {
        let changed = self.document.draft.write().edit_node(node_id, |config| {
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
        target: ModelFeatureTarget<'_>,
        edit: impl FnOnce(&mut WorkflowFeatureDefinition),
    ) {
        let mut draft = self.document.draft.write();
        let changed = draft.edit_node(target.node_id, |config| {
            let WorkflowNodeConfig::Model { features, .. } = config else {
                return false;
            };
            let Some(feature) = features.get_mut(target.index) else {
                return false;
            };
            edit(feature);
            true
        });
        let features = draft
            .nodes()
            .iter()
            .find(|node| node.id() == target.node_id)
            .and_then(|node| match &node.config() {
                WorkflowNodeConfig::Model { features, .. } => Some(features.clone()),
                _ => None,
            })
            .unwrap_or_default();
        if changed {
            draft.refresh_generated_feature_query(target.node_id, &features);
        }
        drop(draft);
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn add_model_feature(mut self, node_id: &str) {
        let mut draft = self.document.draft.write();
        let changed = draft.edit_node(node_id, |config| {
            let WorkflowNodeConfig::Model { features, .. } = config else {
                return false;
            };
            let Some(first) = features.first() else {
                return false;
            };
            let applicability = WorkflowFeatureApplicability::new(
                first.applicability().source_relation(),
                first.applicability().media_field(),
            );
            features
                .push(WorkflowFeatureDefinition::new("", "", "").with_applicability(applicability));
            true
        });
        if changed {
            let features = draft.model_features(node_id);
            draft.refresh_generated_feature_query(node_id, &features);
        }
        drop(draft);
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn remove_model_feature(mut self, node_id: &str, index: usize) {
        let mut draft = self.document.draft.write();
        let changed = draft.edit_node(node_id, |config| {
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
            let features = draft.model_features(node_id);
            draft.refresh_generated_feature_query(node_id, &features);
        }
        drop(draft);
        if changed {
            self.sync_graph();
        }
    }

    pub(crate) fn add_siglip_model(mut self) {
        if self.has_model()
            || self
                .snapshot()
                .phase()
                .topology_mutation_blocked(self.command())
        {
            return;
        }
        let Some(node_id) = self.document.draft.write().insert_siglip_model() else {
            return;
        };
        self.sync_graph();
        self.select_node(node_id);
    }

    pub(crate) fn remove_siglip_model(mut self, node_id: &str) {
        if self
            .snapshot()
            .phase()
            .topology_mutation_blocked(self.command())
        {
            return;
        }
        let Some(query_id) = self.document.draft.write().remove_siglip_model(node_id) else {
            return;
        };
        self.sync_graph();
        self.select_node(query_id);
    }

    pub(crate) fn apply(mut self, studio: Studio) {
        if self.apply_disabled_reason().is_some() {
            return;
        }
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Applying));
        let requested_scope_id = self.document.server.peek().scope_id().to_owned();
        let submitted = self.document.draft.peek().to_owned();
        let request = WorkflowRequest::apply(
            requested_scope_id.clone(),
            self.document.draft.peek().revision(),
            self.document.draft.peek().to_owned(),
        );
        crate::client::project::ProjectClient::current().spawn(async move {
            let response = ProjectClient::current()
                .run_workflow_command(request)
                .await
                .map_err(|error| error.to_string());
            self.finish_command(&requested_scope_id);
            match response {
                Ok(response) => {
                    self.document.saved.set(Some(WorkflowSaveReceipt {
                        submitted,
                        revision: response.workflow().revision(),
                    }));
                    QueryRefresh::current().invalidate(&[QueryDomain::Workflow]);
                }
                Err(error) => {
                    studio.report_from(MessageSource::Workflow, error);
                }
            }
        });
    }

    pub(crate) fn run(mut self, studio: Studio) {
        if let Some(reason) = self.run_disabled_reason() {
            studio.report_warning_from(MessageSource::Workflow, reason);
            return;
        }
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Starting));
        let requested_scope_id = self.document.server.peek().scope_id().to_owned();
        let request = WorkflowRequest::run(
            requested_scope_id.clone(),
            self.document.server.peek().workflow().revision(),
        );
        crate::client::project::ProjectClient::current().spawn(async move {
            let response = ProjectClient::current()
                .run_workflow_command(request)
                .await
                .map_err(|error| error.to_string());
            self.finish_command(&requested_scope_id);
            match response {
                Ok(_) => {}
                Err(error) => studio.report_from(MessageSource::Workflow, error),
            }
        });
    }

    pub(crate) fn cancel(mut self, studio: Studio) {
        if !self.snapshot().phase().active() || self.command().is_some() {
            return;
        }
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Cancelling));
        let requested_scope_id = self.document.server.peek().scope_id().to_owned();
        let request = WorkflowRequest::cancel(requested_scope_id.clone());
        crate::client::project::ProjectClient::current().spawn(async move {
            let response = ProjectClient::current()
                .run_workflow_command(request)
                .await
                .map_err(|error| error.to_string());
            self.finish_command(&requested_scope_id);
            match response {
                Ok(_) => {}
                Err(error) => studio.report_from(MessageSource::Workflow, error),
            }
        });
    }

    pub(crate) fn create_agent_conversation(mut self, studio: Studio) {
        let catalog_available = studio.snapshot().read().readiness().catalog_available();
        if self
            .new_agent_conversation_disabled_reason((studio.live())(), catalog_available)
            .is_some()
        {
            return;
        }
        let current = self.assistant.agent.peek().to_owned();
        self.assistant
            .agent_command
            .set(Some(AgentPendingCommand::CreatingConversation));
        self.assistant.agent_error.set(None);
        let request = AgentRequest::create_conversation(current.scope_id(), current.revision());
        crate::client::project::ProjectClient::current().spawn(async move {
            let response = ProjectClient::current()
                .run_workflow_agent_command(request.clone())
                .await
                .map_err(|error| error.to_string());
            self.assistant.agent_command.set(None);
            match response {
                Ok(_) => {}
                Err(error) => {
                    self.assistant.agent_error.set(Some(error.clone()));
                    studio.report_from(MessageSource::Workflow, error);
                }
            }
        });
    }

    pub(crate) fn activate_agent_conversation(mut self, conversation_id: String, studio: Studio) {
        let current = self.assistant.agent.peek().to_owned();
        let catalog_available = studio.snapshot().read().readiness().catalog_available();
        if conversation_id == current.active_conversation_id()
            || self
                .agent_conversation_disabled_reason((studio.live())(), catalog_available)
                .is_some()
        {
            return;
        }
        self.assistant
            .agent_command
            .set(Some(AgentPendingCommand::ActivatingConversation));
        self.assistant.agent_error.set(None);
        let request = AgentRequest::activate_conversation(
            current.scope_id(),
            conversation_id,
            current.revision(),
        );
        crate::client::project::ProjectClient::current().spawn(async move {
            let response = ProjectClient::current()
                .run_workflow_agent_command(request.clone())
                .await
                .map_err(|error| error.to_string());
            self.assistant.agent_command.set(None);
            match response {
                Ok(_) => {}
                Err(error) => {
                    self.assistant.agent_error.set(Some(error.clone()));
                    studio.report_from(MessageSource::Workflow, error);
                }
            }
        });
    }

    pub(crate) fn send_agent(mut self, text: String, studio: Studio) {
        let text = text.trim().to_owned();
        let state = self.assistant.agent.peek().to_owned();
        if text.is_empty()
            || !state.readiness().configured()
            || state.session().phase().active()
            || self.assistant.agent_command.peek().is_some()
            || self.document.server.peek().snapshot().phase().active()
        {
            return;
        }
        self.assistant
            .agent_command
            .set(Some(AgentPendingCommand::Sending));
        self.assistant.agent_error.set(None);
        let request = AgentRequest::Turn {
            scope_id: state.scope_id().to_owned(),
            conversation_id: state.active_conversation_id().to_owned(),
            text: text.clone(),
            expected_revision: state.revision(),
        };
        crate::client::project::ProjectClient::current().spawn(async move {
            let response = ProjectClient::current()
                .run_workflow_agent_command(request.clone())
                .await
                .map_err(|error| error.to_string());
            self.assistant.agent_command.set(None);
            match response {
                Ok(_) => {
                    if self.assistant.agent.peek().scope_id() == request.scope_id() {
                        let key = AgentDraftKey::new(
                            request.scope_id(),
                            request
                                .conversation_id()
                                .expect("turn commands carry a conversation"),
                        );
                        key.clear_if_unchanged(&mut self.assistant.agent_drafts.write(), &text);
                    }
                }
                Err(error) => {
                    self.assistant.agent_error.set(Some(error.clone()));
                    studio.report_from(MessageSource::Workflow, error);
                }
            }
        });
    }

    pub(crate) fn cancel_agent(mut self, studio: Studio) {
        let current = self.assistant.agent.peek().to_owned();
        if !current.session().phase().active() || self.assistant.agent_command.peek().is_some() {
            return;
        }
        self.assistant
            .agent_command
            .set(Some(AgentPendingCommand::Cancelling));
        crate::client::project::ProjectClient::current().spawn(async move {
            let Some(turn_id) = current.session().active_turn_id().to_owned() else {
                self.assistant.agent_command.set(None);
                return;
            };
            let request = AgentRequest::cancel(
                current.scope_id(),
                current.active_conversation_id(),
                turn_id,
            );
            let response = ProjectClient::current()
                .run_workflow_agent_command(request.clone())
                .await
                .map_err(|error| error.to_string());
            self.assistant.agent_command.set(None);
            match response {
                Ok(_) => {}
                Err(error) => {
                    self.assistant.agent_error.set(Some(error.clone()));
                    studio.report_from(MessageSource::Workflow, error);
                }
            }
        });
    }

    pub(crate) fn retry_agent(self, studio: Studio) {
        let text = self
            .assistant
            .agent
            .peek()
            .session()
            .messages()
            .iter()
            .rev()
            .find(|message| {
                message.role() == spiral_robotics_common::workflow::WorkflowAgentRole::User
            })
            .map(|message| message.text().to_owned());
        if let Some(text) = text {
            self.send_agent(text, studio);
        }
    }

    pub(crate) fn use_polling(mut self, studio: Studio) {
        let sessions = crate::shell::project::ProjectSessionSignals::current();
        let workflow_active = use_memo(move || {
            !self.loaded() || self.snapshot().phase().active() || self.command().is_some()
        });
        let workflow = QueryPolicy::new(QueryDomain::Workflow, 500)
            .with_activity(workflow_active.into())
            .use_when(
                move || sessions.catalog_available(),
                || ProjectClient::current().workflow_snapshot(),
            );
        use_effect(move || {
            let result = workflow.read();
            if !workflow.finished() {
                return;
            }
            match result.as_ref() {
                Some(Ok(state)) => {
                    if self.accept_fetched_state(state.clone()) {
                        studio.refresh_relation_catalog();
                    }
                }
                Some(Err(error)) => self.lifecycle.load_error.set(Some(error.to_string())),
                None => {}
            }
        });
        let assistant_active = use_memo(move || {
            !self.agent_loaded()
                || self.assistant.agent.read().session().phase().active()
                || self.agent_command().is_some()
        });
        let agent = QueryPolicy::new(QueryDomain::Assistant, 500)
            .with_activity(assistant_active.into())
            .use_when(
                move || sessions.catalog_available(),
                || ProjectClient::current().workflow_agent_snapshot(),
            );
        use_effect(move || {
            let result = agent.read();
            if !agent.finished() {
                return;
            }
            match result.as_ref() {
                Some(Ok(state)) => self.accept_fetched_agent(state.clone()),
                Some(Err(error)) => self.assistant.agent_error.set(Some(error.to_string())),
                None => {}
            }
        });
    }

    fn accept_fetched_state(mut self, state: WorkflowSnapshot) -> bool {
        if !self.document.server.peek().accepts(&state) {
            return false;
        }
        if *self.document.server.peek() == state && self.document.saved.peek().is_none() {
            self.lifecycle.load_error.set(None);
            self.lifecycle.loaded.set(true);
            return false;
        }
        let scope_changed = state.scope_id() != self.document.server.peek().scope_id();
        let saved = self.document.saved.peek().clone();
        let acknowledged = saved.as_ref().is_some_and(|receipt| {
            receipt.acknowledges(&self.document.draft.peek(), state.workflow())
        });
        let keep_local_draft = !scope_changed
            && !acknowledged
            && *self.document.draft.peek() != *self.document.server.peek().workflow();
        if scope_changed
            || saved
                .as_ref()
                .is_some_and(|receipt| state.workflow().revision() >= receipt.revision)
        {
            self.document.saved.set(None);
        }
        if scope_changed {
            self.graph.ui.set(WorkflowUiState::default());
            self.lifecycle.command.set(None);
        }
        self.document.server.set(state.clone());
        if !keep_local_draft {
            self.document.draft.set(state.workflow().to_owned());
        }
        self.lifecycle.loaded.set(true);
        self.lifecycle.load_error.set(None);
        self.sync_graph();
        scope_changed
    }

    fn finish_command(mut self, requested_scope_id: &str) {
        if self.document.server.peek().scope_id() == requested_scope_id {
            self.lifecycle.command.set(None);
        }
    }

    fn accept_fetched_agent(mut self, state: WorkflowAgentSnapshot) {
        let current = self.assistant.agent.peek();
        if !current.accepts_fetched(&state) {
            return;
        }
        let changed = *current != state;
        drop(current);
        if !changed {
            self.assistant.agent_loaded.set(true);
            self.assistant.agent_error.set(None);
            return;
        }
        self.assistant.agent.set(state);
        self.assistant.agent_loaded.set(true);
        self.assistant.agent_error.set(None);
    }

    fn sync_graph(mut self) {
        let spec = self.document.draft.peek().to_owned();
        let snapshot = self.document.server.peek().snapshot().to_owned();
        let relations = self.relations();
        let mut next = spec.graph_nodes(&snapshot, &relations);
        let next_edges = spec.graph_edges(&snapshot);
        let previous = self
            .graph
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
            .graph
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
        self.graph.nodes.set(next);
        self.graph.edges.set(next_edges);
    }

    fn scoped_relation_catalog(&self) -> Option<RelationCatalog> {
        let current_scope_id = self.document.server.peek().scope_id().to_owned();
        self.relation_catalog
            .read_unchecked()
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .cloned()
            .filter(|catalog| catalog.matches_scope(&current_scope_id, &current_scope_id))
    }
}

trait WorkflowArtifactNavigation {
    fn panel_id(&self, state: &WorkflowSnapshot, dirty: bool) -> Option<&'static str>;
}

impl WorkflowArtifactNavigation for WorkflowAgentArtifactReceipt {
    fn panel_id(&self, state: &WorkflowSnapshot, dirty: bool) -> Option<&'static str> {
        match self {
            WorkflowAgentArtifactReceipt::Graph { revision, .. }
                if !dirty && *revision == state.workflow().revision() =>
            {
                Some(CANVAS_PANEL_ID)
            }
            WorkflowAgentArtifactReceipt::Run { run_id, .. }
                if Some(run_id.as_str()) == state.snapshot().run_id() =>
            {
                Some(CANVAS_PANEL_ID)
            }
            WorkflowAgentArtifactReceipt::Results { run_id, .. }
                if Some(run_id.as_str()) == state.snapshot().run_id()
                    && state.snapshot().phase() == WorkflowRunPhase::Completed
                    && !state.snapshot().result_stale()
                    && state.snapshot().result().is_some() =>
            {
                Some(RESULTS_PANEL_ID)
            }
            _ => None,
        }
    }
}

trait WorkflowAgentReconciliation {
    fn accepts_fetched(&self, incoming: &WorkflowAgentSnapshot) -> bool;
}
impl WorkflowAgentReconciliation for WorkflowAgentSnapshot {
    fn accepts_fetched(&self, incoming: &WorkflowAgentSnapshot) -> bool {
        incoming.session().workflow_id() == self.session().workflow_id()
            && (incoming.scope_id() != self.scope_id() || incoming.revision() >= self.revision())
    }
}

struct AgentConversationChange<'a> {
    loaded: bool,
    connected: bool,
    catalog_available: bool,
    command: Option<AgentPendingCommand>,
    agent: &'a WorkflowAgentSnapshot,
    snapshot: &'a WorkflowRunSnapshot,
}

impl AgentConversationChange<'_> {
    fn reason(&self) -> Option<String> {
        if !self.loaded {
            return Some("Conversation history is still loading.".to_owned());
        }
        if !self.connected {
            return Some("Reconnect to Studio before changing conversations.".to_owned());
        }
        if !self.catalog_available {
            return Some("Configure or reconnect Spiral before changing conversations.".to_owned());
        }
        if !self.agent.history_available() {
            return Some(self.agent.readiness().detail().to_owned());
        }
        if self.command.is_some() {
            return Some("Wait for the current conversation action to finish.".to_owned());
        }
        if self.agent.session().phase().active() {
            return Some(
                "Stop the active Assistant turn before changing conversations.".to_owned(),
            );
        }
        if self.snapshot.phase().active()
            && self.snapshot.run_id().is_some_and(|run_id| {
                self.agent
                    .conversations()
                    .iter()
                    .any(|conversation| conversation.owned_run_id() == Some(run_id))
            })
        {
            return Some(
                "Stop the Workflow run started by the Assistant before changing conversations."
                    .to_owned(),
            );
        }
        None
    }
}

trait WorkflowStateReconciliation {
    fn accepts(&self, incoming: &WorkflowSnapshot) -> bool;
}

impl WorkflowStateReconciliation for WorkflowSnapshot {
    fn accepts(&self, incoming: &WorkflowSnapshot) -> bool {
        if incoming.snapshot().workflow_id() != incoming.workflow().id()
            || incoming.snapshot().workflow_revision() != incoming.workflow().revision()
        {
            return false;
        }
        if incoming.scope_id() != self.scope_id() {
            return true;
        }
        if incoming.workflow().id() != self.workflow().id()
            || incoming.workflow().revision() < self.workflow().revision()
        {
            return false;
        }
        incoming.workflow().revision() > self.workflow().revision()
            || self.snapshot().accepts(incoming.snapshot())
    }
}

trait RelationCatalogReconciliation {
    fn matches_scope(&self, current_scope_id: &str, requested_scope_id: &str) -> bool;
}

impl RelationCatalogReconciliation for RelationCatalog {
    fn matches_scope(&self, current_scope_id: &str, requested_scope_id: &str) -> bool {
        current_scope_id == requested_scope_id && self.scope_id() == requested_scope_id
    }
}

trait WorkflowSnapshotReconciliation {
    fn accepts(&self, incoming: &WorkflowRunSnapshot) -> bool;
}

impl WorkflowSnapshotReconciliation for WorkflowRunSnapshot {
    fn accepts(&self, incoming: &WorkflowRunSnapshot) -> bool {
        if incoming.workflow_id() != self.workflow_id()
            || incoming.workflow_revision() != self.workflow_revision()
        {
            return false;
        }

        if incoming.run_id() != self.run_id() {
            return match (&self.run_id(), &incoming.run_id()) {
                (None, Some(_)) => true,
                (Some(_), None) => false,
                (Some(_), Some(_)) => incoming.started_ms() > self.started_ms(),
                (None, None) => true,
            };
        }
        if incoming.updated_ms() != self.updated_ms() {
            return incoming.updated_ms() > self.updated_ms();
        }

        incoming.phase().monotonic_rank() >= self.phase().monotonic_rank()
    }
}

trait WorkflowRunPhaseReconciliation {
    fn monotonic_rank(self) -> u8;
}

impl WorkflowRunPhaseReconciliation for WorkflowRunPhase {
    fn monotonic_rank(self) -> u8 {
        match self {
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
}

trait WorkflowDraft {
    fn edit_node(
        &mut self,
        node_id: &str,
        edit: impl FnOnce(&mut WorkflowNodeConfig) -> bool,
    ) -> bool;
    fn insert_siglip_model(&mut self) -> Option<String>;
    fn remove_siglip_model(&mut self, node_id: &str) -> Option<String>;
    fn model_features(&self, model_id: &str) -> Vec<WorkflowFeatureDefinition>;
    fn refresh_generated_feature_query(
        &mut self,
        model_id: &str,
        features: &[WorkflowFeatureDefinition],
    );
    fn revision_conflicts_with(&self, server: &WorkflowSpec) -> bool;
    fn empty_draft(&self) -> WorkflowSpec;
}

impl WorkflowDraft for WorkflowSpec {
    fn edit_node(
        &mut self,
        node_id: &str,
        edit: impl FnOnce(&mut WorkflowNodeConfig) -> bool,
    ) -> bool {
        self.nodes_mut()
            .iter_mut()
            .find(|node| node.id() == node_id)
            .is_some_and(|node| edit(node.config_mut()))
    }

    fn insert_siglip_model(&mut self) -> Option<String> {
        let workflow = self;
        let result_edge_index = workflow.edges().iter().position(|edge| {
            let source = workflow
                .nodes()
                .iter()
                .find(|node| node.id() == edge.source());
            let target = workflow
                .nodes()
                .iter()
                .find(|node| node.id() == edge.target());
            source.is_some_and(|node| {
                WorkflowNodeKind::from(node.config()) == WorkflowNodeKind::Query
            }) && target.is_some_and(|node| {
                WorkflowNodeKind::from(node.config()) == WorkflowNodeKind::Result
            })
        })?;
        let result_edge = workflow.edges()[result_edge_index].clone();
        let candidate_id = result_edge.source().to_owned();
        let source_id = workflow
            .edges()
            .iter()
            .find(|edge| edge.target() == candidate_id)
            .and_then(|edge| {
                workflow
                    .nodes()
                    .iter()
                    .find(|node| node.id() == edge.source())
                    .filter(|node| WorkflowNodeKind::from(node.config()) == WorkflowNodeKind::Table)
                    .map(|_| edge.source().to_owned())
            })?;
        let source_relation = workflow
            .nodes()
            .iter()
            .find(|node| node.id() == source_id)
            .and_then(|node| match &node.config() {
                WorkflowNodeConfig::Table { relation } => Some(relation.clone()),
                _ => None,
            })?;
        workflow.edges_mut().remove(result_edge_index);
        let mut suffix = 1_u32;
        let node_id = loop {
            let candidate = if suffix == 1 {
                "siglip".to_owned()
            } else {
                format!("siglip_{suffix}")
            };
            if !workflow.nodes().iter().any(|node| node.id() == candidate) {
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
            if !workflow.nodes().iter().any(|node| node.id() == candidate) {
                break candidate;
            }
            query_suffix += 1;
        };
        let insert_at = workflow
            .nodes()
            .iter()
            .position(|node| node.id() == result_edge.target())
            .unwrap_or(workflow.nodes().len());
        workflow.nodes_mut().insert(
            insert_at,
            spiral_robotics_common::workflow::WorkflowNodeSpec::new(
                node_id.clone(),
                "Backfill visual features",
                WorkflowNodeConfig::Model {
                    features: vec![
                        WorkflowFeatureDefinition::new("", "", "").with_applicability(
                            WorkflowFeatureApplicability::new(source_relation, "image"),
                        ),
                    ],
                },
            ),
        );
        workflow.nodes_mut().insert(
            insert_at + 1,
            spiral_robotics_common::workflow::WorkflowNodeSpec::new(
                final_query_id.clone(),
                "Join candidates and features",
                WorkflowNodeConfig::Query {
                    sql: FeatureJoinSql::build(&candidate_id, &node_id, &[]),
                    max_rows: 100,
                },
            ),
        );
        workflow.edges_mut().extend([
            WorkflowEdgeSpec::new(candidate_id.clone(), node_id.clone()),
            WorkflowEdgeSpec::new(candidate_id.clone(), final_query_id.clone()),
            WorkflowEdgeSpec::new(node_id.clone(), final_query_id.clone()),
            WorkflowEdgeSpec::new(final_query_id, result_edge.target()),
        ]);
        Some(node_id)
    }

    fn remove_siglip_model(&mut self, node_id: &str) -> Option<String> {
        let workflow = self;
        let model_index = workflow.nodes().iter().position(|node| {
            node.id() == node_id && WorkflowNodeKind::from(node.config()) == WorkflowNodeKind::Model
        })?;
        let candidate_id = workflow
            .edges()
            .iter()
            .find(|edge| edge.target() == node_id)
            .map(|edge| edge.source().to_owned())?;
        let final_query_id = workflow
            .edges()
            .iter()
            .find(|edge| edge.source() == node_id)
            .map(|edge| edge.target().to_owned())?;
        let result_id = workflow
            .edges()
            .iter()
            .find(|edge| edge.source() == final_query_id)
            .map(|edge| edge.target().to_owned())?;
        let query_is_upstream = workflow.nodes().iter().any(|node| {
            node.id() == candidate_id
                && WorkflowNodeKind::from(node.config()) == WorkflowNodeKind::Query
        });
        let query_is_downstream = workflow.nodes().iter().any(|node| {
            node.id() == final_query_id
                && WorkflowNodeKind::from(node.config()) == WorkflowNodeKind::Query
        });
        if !query_is_upstream || !query_is_downstream {
            return None;
        }

        workflow.nodes_mut().remove(model_index);
        workflow
            .nodes_mut()
            .retain(|node| node.id() != final_query_id);
        workflow.edges_mut().retain(|edge| {
            edge.source() != node_id
                && edge.target() != node_id
                && edge.source() != final_query_id
                && edge.target() != final_query_id
        });
        workflow
            .edges_mut()
            .push(WorkflowEdgeSpec::new(candidate_id.clone(), result_id));
        Some(candidate_id)
    }

    fn model_features(&self, model_id: &str) -> Vec<WorkflowFeatureDefinition> {
        self.nodes()
            .iter()
            .find(|node| node.id() == model_id)
            .and_then(|node| match &node.config() {
                WorkflowNodeConfig::Model { features, .. } => Some(features.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn refresh_generated_feature_query(
        &mut self,
        model_id: &str,
        features: &[WorkflowFeatureDefinition],
    ) {
        let workflow = self;
        let Some(candidate_id) = workflow
            .edges()
            .iter()
            .find(|edge| edge.target() == model_id)
            .map(|edge| edge.source().to_owned())
        else {
            return;
        };
        let Some(final_query_id) = workflow
            .edges()
            .iter()
            .find(|edge| edge.source() == model_id)
            .map(|edge| edge.target().to_owned())
        else {
            return;
        };
        if let Some(query) = workflow
            .nodes_mut()
            .iter_mut()
            .find(|node| node.id() == final_query_id)
            && let WorkflowNodeConfig::Query { sql, .. } = query.config_mut()
        {
            *sql = FeatureJoinSql::build(&candidate_id, model_id, features);
        }
    }

    /// Keep deterministic server-owned Model fields visible and valid while the
    /// user edits its one authoritative source Table. The backend repeats this
    /// normalization at admission; the browser never chooses a cache relation.
    fn revision_conflicts_with(&self, server: &WorkflowSpec) -> bool {
        self != server && self.revision() != server.revision()
    }

    fn empty_draft(&self) -> WorkflowSpec {
        WorkflowSpec::new(self.id(), "Untitled workflow").with_revision(self.revision())
    }
}

struct FeatureJoinSql;

impl FeatureJoinSql {
    fn build(candidate_id: &str, model_id: &str, features: &[WorkflowFeatureDefinition]) -> String {
        let feature_projection = features
            .iter()
            .filter(|feature| !feature.name().is_empty())
            .map(|feature| format!(", features.{}", feature.name()))
            .collect::<String>();
        format!(
            "SELECT candidates.*{feature_projection} FROM {{{{node:{candidate_id}}}}} candidates JOIN {{{{node:{model_id}}}}} features ON candidates.episode_id = features.episode_id AND candidates.stream_id = features.stream_id AND candidates.segment_index = features.segment_index AND candidates.frame_index = features.frame_index AND candidates.episode_time_ns = features.episode_time_ns"
        )
    }
}

trait WorkflowRunPhaseEditing {
    fn topology_mutation_blocked(self, command: Option<WorkflowPendingCommand>) -> bool;
}

impl WorkflowRunPhaseEditing for WorkflowRunPhase {
    fn topology_mutation_blocked(self, command: Option<WorkflowPendingCommand>) -> bool {
        self.active() || command.is_some()
    }
}

trait WorkflowGraphProjection {
    fn graph_nodes(
        &self,
        snapshot: &WorkflowRunSnapshot,
        relations: &[CatalogRelation],
    ) -> Vec<Node<WorkflowNodeData>>;
    fn fallback_node_ranks(&self) -> BTreeMap<&str, usize>;
    fn graph_edges(&self, snapshot: &WorkflowRunSnapshot) -> Vec<Edge>;
}

impl WorkflowGraphProjection for WorkflowSpec {
    fn graph_nodes(
        &self,
        snapshot: &WorkflowRunSnapshot,
        relations: &[CatalogRelation],
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
            .map(|relation| (relation.name(), relation))
            .collect::<BTreeMap<_, _>>();
        let phase_index = snapshot
            .nodes()
            .iter()
            .map(|node| (node.node_id(), node))
            .collect::<BTreeMap<_, _>>();
        let ranks = self.fallback_node_ranks();
        let mut rank_indices = BTreeMap::<usize, usize>::new();
        self.nodes()
            .iter()
            .map(|node| {
                let rank = ranks.get(node.id()).copied().unwrap_or_default();
                let rank_index = rank_indices.entry(rank).or_default();
                let position = (
                    FIRST_RANK_X + RANK_STEP_X * rank as f64,
                    30.0 + *rank_index as f64 * 178.0,
                );
                *rank_index += 1;
                let type_name = match WorkflowNodeKind::from(node.config()) {
                    WorkflowNodeKind::Table => "table",
                    WorkflowNodeKind::Model => "model",
                    WorkflowNodeKind::Query => "query",
                    WorkflowNodeKind::Result => "result",
                };
                Node::with_data(
                    node.id().to_owned(),
                    node.name().to_owned(),
                    position,
                    NodeProjection {
                        run_snapshot: snapshot,
                        relations: &relation_index,
                    }
                    .project(node, phase_index.get(node.id()).copied()),
                )
                .node_type(type_name)
                .sides(Side::Left, Side::Right)
            })
            .collect()
    }

    fn fallback_node_ranks(&self) -> BTreeMap<&str, usize> {
        let mut ranks = self
            .nodes()
            .iter()
            .map(|node| (node.id(), 0_usize))
            .collect::<BTreeMap<_, _>>();
        let order = self.topological_order().unwrap_or_else(|_| {
            // Invalid drafts are not runnable, but still need deterministic finite
            // fallback geometry while validation explains the graph error.
            self.nodes()
                .iter()
                .map(|node| node.id().to_owned())
                .collect()
        });
        for source in order {
            let source_rank = ranks.get(source.as_str()).copied().unwrap_or_default();
            for edge in self.edges().iter().filter(|edge| edge.source() == source) {
                if let Some(target_rank) = ranks.get_mut(edge.target()) {
                    *target_rank = (*target_rank).max(source_rank.saturating_add(1));
                }
            }
        }
        ranks
    }

    fn graph_edges(&self, snapshot: &WorkflowRunSnapshot) -> Vec<Edge> {
        let active_targets = snapshot
            .nodes()
            .iter()
            .filter(|node| {
                matches!(
                    node.phase(),
                    WorkflowNodePhase::Validating
                        | WorkflowNodePhase::Loading
                        | WorkflowNodePhase::Running
                        | WorkflowNodePhase::Cancelling
                )
            })
            .map(|node| node.node_id())
            .collect::<BTreeSet<_>>();
        self.edges()
            .iter()
            .map(|edge| {
                let active = active_targets.contains(edge.target());
                let edge = Edge::new(edge.source().to_owned(), edge.target().to_owned())
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
}

struct NodeProjection<'a> {
    run_snapshot: &'a WorkflowRunSnapshot,
    relations: &'a BTreeMap<&'a str, &'a CatalogRelation>,
}

impl NodeProjection<'_> {
    fn project(
        &self,
        node: &spiral_robotics_common::workflow::WorkflowNodeSpec,
        node_snapshot: Option<&WorkflowNodeSnapshot>,
    ) -> WorkflowNodeData {
        let phase = node_snapshot.map_or(WorkflowNodePhase::Pending, |snapshot| snapshot.phase());
        let detail = node_snapshot
            .map(|snapshot| snapshot.detail().to_owned())
            .unwrap_or_else(|| "Not run".to_owned());
        let error = node_snapshot.and_then(|snapshot| snapshot.error().map(str::to_owned));
        let progress = node_snapshot.and_then(|snapshot| snapshot.progress().cloned());
        let quarantine = node_snapshot
            .map(|snapshot| snapshot.quarantine().to_owned())
            .unwrap_or_default();
        let model_snapshot = node_snapshot.and_then(|snapshot| snapshot.model().cloned());
        let model_output_relation = node_snapshot
            .and_then(WorkflowNodeSnapshot::output_relation)
            .map(str::to_owned);
        let warnings = node_snapshot
            .map(|snapshot| snapshot.warnings().to_owned())
            .unwrap_or_default();
        let execution = WorkflowNodeExecution {
            phase,
            detail,
            error,
            progress,
            quarantine,
            warnings,
        };
        match node.config() {
        WorkflowNodeConfig::Table { relation } => {
            let metadata = self.relations.get(relation.as_str());
            WorkflowNodeData {
                kind: WorkflowNodeKind::Table,
                description: metadata.map_or_else(
                    || "Choose an existing qualified Spiral relation.".to_owned(),
                    |metadata| {
                        format!(
                            "{} · {} fields",
                            metadata.relation_type(),
                            metadata.columns().len()
                        )
                    },
                ),
                source: WorkflowNodeSource {
                    fields: metadata
                        .map(|metadata| {
                            metadata
                                .columns()
                                .iter()
                                .map(|column| column.name().to_owned())
                                .collect()
                        })
                        .unwrap_or_default(),
                    relation: (!relation.is_empty()).then(|| relation.clone()),
                    ..WorkflowNodeSource::default()
                },
                execution,
                model: WorkflowNodeModel {
                    metadata: model_snapshot,
                    ..WorkflowNodeModel::default()
                },
            }
        }
        WorkflowNodeConfig::Model { features } => WorkflowNodeData {
            kind: WorkflowNodeKind::Model,
            description:
                "Pinned SigLIP backfills registered features for a SQL-defined candidate cohort."
                    .to_owned(),
            source: WorkflowNodeSource {
                fields: features
                    .iter()
                    .map(|feature| feature.name().trim())
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect(),
                feature_cache_relation: model_output_relation,
                ..WorkflowNodeSource::default()
            },
            execution,
            model: WorkflowNodeModel {
                features: features.clone(),
                metadata: model_snapshot,
            },
        },
        WorkflowNodeConfig::Query { sql, .. } => WorkflowNodeData {
            kind: WorkflowNodeKind::Query,
            description: "Bounded read-only Spiral SQL with server-owned relation placeholders."
                .to_owned(),
            source: WorkflowNodeSource {
                sql: Some(sql.clone()),
                ..WorkflowNodeSource::default()
            },
            execution,
            model: WorkflowNodeModel {
                metadata: model_snapshot,
                ..WorkflowNodeModel::default()
            },
        },
        WorkflowNodeConfig::Result => WorkflowNodeData {
            kind: WorkflowNodeKind::Result,
            description: "Terminal media-aware query result.".to_owned(),
            source: WorkflowNodeSource {
                fields: self.run_snapshot
                    .result()
                    .as_ref()
                    .map(|result| {
                        result
                            .columns()
                            .iter()
                            .map(|column| column.name().to_owned())
                            .collect()
                    })
                    .unwrap_or_default(),
                ..WorkflowNodeSource::default()
            },
            execution,
            model: WorkflowNodeModel {
                metadata: model_snapshot,
                ..WorkflowNodeModel::default()
            },
        },
    }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_selection_activates_the_node_inspector() {
        let selected =
            WorkflowUiState::default().reduce(WorkflowUiAction::SelectNode("query".to_owned()));
        assert_eq!(selected.selected_node.as_deref(), Some("query"));
        assert_eq!(selected.active_panel.as_deref(), Some(NODE_PANEL_ID));
    }

    #[test]
    fn default_editor_is_table_query_result_without_a_fake_model() {
        let state = WorkflowSnapshot::from_workflow("", WorkflowSpec::default());
        let kinds = state
            .workflow()
            .nodes()
            .iter()
            .map(|node| WorkflowNodeKind::from(node.config()))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                WorkflowNodeKind::Table,
                WorkflowNodeKind::Query,
                WorkflowNodeKind::Result
            ]
        );
        assert!(!state.workflow().has_model());
    }

    #[test]
    fn graph_projection_preserves_saved_topology() {
        let workflow = WorkflowSpec::default();
        let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        let nodes = workflow.graph_nodes(&snapshot, &[]);
        let edges = workflow.graph_edges(&snapshot);
        assert_eq!(nodes.len(), 3);
        assert_eq!(edges.len(), 2);
        assert_eq!(nodes[0].id, "table");
        assert_eq!(nodes[2].id, "result");
        assert_eq!(nodes[1].position.x - nodes[0].position.x, 224.0);
        assert_eq!(nodes[2].position.x - nodes[1].position.x, 224.0);
    }

    #[test]
    fn starting_fresh_preserves_server_identity_but_removes_all_topology() {
        let workflow = WorkflowSpec::new("inspection", "Existing workflow").with_revision(17);

        let empty = workflow.empty_draft();

        assert_eq!(empty.id(), "inspection");
        assert_eq!(empty.revision(), 17);
        assert_eq!(empty.name(), "Untitled workflow");
        assert!(empty.nodes().is_empty());
        assert!(empty.edges().is_empty());
        assert!(!empty.validate().valid());
        assert!(
            empty
                .graph_nodes(&WorkflowRunSnapshot::idle_for(&empty), &[])
                .is_empty()
        );
        assert!(
            empty
                .graph_edges(&WorkflowRunSnapshot::idle_for(&empty))
                .is_empty()
        );
    }

    #[test]
    fn only_edges_feeding_the_active_node_animate() {
        let workflow = WorkflowSpec::default();
        let mut snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        snapshot.nodes_mut()[1].set_phase(WorkflowNodePhase::Running);

        let edges = workflow.graph_edges(&snapshot);

        assert!(edges[0].animated);
        assert_eq!(edges[0].class.as_deref(), Some("workflow-edge-running"));
        assert!(!edges[1].animated);
        assert!(edges[1].class.is_none());
    }

    #[test]
    fn inserting_siglip_makes_candidate_backfill_and_final_join_visible() {
        let mut workflow = WorkflowSpec::default();
        let WorkflowNodeConfig::Table { relation } = workflow.nodes_mut()[0].config_mut() else {
            unreachable!()
        };
        *relation = "pick_place.camera_frames".to_owned();
        let node_id = workflow.insert_siglip_model().unwrap();
        assert_eq!(node_id, "siglip");
        assert_eq!(workflow.nodes().len(), 5);
        assert_eq!(workflow.edges().len(), 5);
        assert_eq!(workflow.edges()[0].source(), "table");
        assert_eq!(workflow.edges()[0].target(), "query");
        assert!(
            workflow
                .edges()
                .iter()
                .any(|edge| edge.source() == "query" && edge.target() == "siglip")
        );
        assert!(
            workflow
                .edges()
                .iter()
                .any(|edge| edge.source() == "query" && edge.target() == "feature_query")
        );
        assert!(
            workflow
                .edges()
                .iter()
                .any(|edge| edge.source() == "siglip" && edge.target() == "feature_query")
        );
        assert!(
            workflow
                .edges()
                .iter()
                .any(|edge| edge.source() == "feature_query" && edge.target() == "result")
        );
        let WorkflowNodeConfig::Model { features, .. } = &workflow
            .nodes()
            .iter()
            .find(|node| node.id() == "siglip")
            .unwrap()
            .config()
        else {
            unreachable!()
        };
        assert_eq!(features[0].applicability().media_field(), "image");
        assert_eq!(
            features[0].applicability().source_relation(),
            "pick_place.camera_frames"
        );
        let WorkflowNodeConfig::Query { sql, .. } = &workflow
            .nodes()
            .iter()
            .find(|node| node.id() == "feature_query")
            .unwrap()
            .config()
        else {
            unreachable!()
        };
        assert!(sql.contains("{{node:query}}"));
        assert!(sql.contains("{{node:siglip}}"));
    }

    #[test]
    fn model_fallback_uses_five_topology_ranks_with_two_table_roots() {
        let mut workflow = WorkflowSpec::default();
        let WorkflowNodeConfig::Table { relation } = workflow.nodes_mut()[0].config_mut() else {
            unreachable!()
        };
        *relation = "pick_place.camera_frames".to_owned();
        workflow.insert_siglip_model().unwrap();
        let mut joint_state = workflow.nodes()[0].clone();
        joint_state.set_id("joint_state");
        joint_state.set_name("Joint state");
        joint_state.set_config(WorkflowNodeConfig::Table {
            relation: "pick_place.joint_state".to_owned(),
        });
        workflow.nodes_mut().push(joint_state);
        workflow
            .edges_mut()
            .push(WorkflowEdgeSpec::new("joint_state", "feature_query"));

        let ranks = workflow.fallback_node_ranks();
        assert_eq!(ranks["table"], 0);
        assert_eq!(ranks["joint_state"], 0);
        assert_eq!(ranks["query"], 1);
        assert_eq!(ranks["siglip"], 2);
        assert_eq!(ranks["feature_query"], 3);
        assert_eq!(ranks["result"], 4);

        let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        let nodes = workflow.graph_nodes(&snapshot, &[]);
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
            .nodes_mut()
            .push(spiral_robotics_common::workflow::WorkflowNodeSpec::new(
                "second_query",
                "Second query",
                WorkflowNodeConfig::Query {
                    sql: "SELECT * FROM {{node:query}}".to_owned(),
                    max_rows: 100,
                },
            ));
        workflow
            .edges_mut()
            .retain(|edge| !(edge.source() == "query" && edge.target() == "result"));
        workflow.edges_mut().extend([
            WorkflowEdgeSpec::new("query", "second_query"),
            WorkflowEdgeSpec::new("second_query", "result"),
        ]);
        let before = workflow.clone();

        assert!(workflow.insert_siglip_model().is_none());
        assert_eq!(workflow, before);
    }

    #[test]
    fn configured_source_and_feature_make_the_manual_model_draft_applicable() {
        let mut workflow = WorkflowSpec::default();
        let model_id = workflow.insert_siglip_model().unwrap();
        let blank_report = workflow.validate();
        assert_eq!(
            blank_report
                .errors()
                .iter()
                .filter(|issue| issue.code() == "incomplete_feature_definition")
                .count(),
            1
        );
        assert!(!blank_report.errors().iter().any(|issue| matches!(
            issue.code(),
            "invalid_feature_name" | "invalid_feature_description" | "invalid_feature_concept"
        )));
        let WorkflowNodeConfig::Table { relation } = workflow.nodes_mut()[0].config_mut() else {
            unreachable!()
        };
        *relation = "pick_place.camera_frames".to_owned();
        let WorkflowNodeConfig::Model { features, .. } = &mut workflow
            .nodes_mut()
            .iter_mut()
            .find(|node| node.id() == model_id)
            .unwrap()
            .config_mut()
        else {
            unreachable!()
        };
        features[0].set_name("is_raining_feature");
        features[0].set_description("Probability that the frame shows rain");
        features[0].set_concept("raining");
        let features = features.clone();
        workflow.refresh_generated_feature_query(&model_id, &features);
        workflow.normalize_model_sources();

        assert!(workflow.validate().valid());
        let config = workflow
            .nodes()
            .iter()
            .find(|node| node.id() == model_id)
            .unwrap()
            .config();
        let WorkflowNodeConfig::Model { features } = config else {
            unreachable!()
        };
        assert_eq!(
            features[0].applicability().source_relation(),
            "pick_place.camera_frames"
        );
        assert_eq!(
            config.model_output_name(),
            Some(WorkflowFeatureApplicability::derived_cache_name(
                "pick_place.camera_frames"
            ))
        );
        let WorkflowNodeConfig::Query { sql, .. } = &workflow
            .nodes()
            .iter()
            .find(|node| node.id() == "feature_query")
            .unwrap()
            .config()
        else {
            unreachable!()
        };
        assert!(sql.contains("features.is_raining_feature"));
    }

    #[test]
    fn removing_siglip_restores_the_candidate_query_as_terminal() {
        let mut workflow = WorkflowSpec::default();
        workflow.insert_siglip_model().unwrap();

        let selected = workflow.remove_siglip_model("siglip").unwrap();

        assert_eq!(selected, "query");
        assert_eq!(workflow.nodes().len(), 3);
        assert_eq!(workflow.edges().len(), 2);
        assert!(
            workflow
                .edges()
                .iter()
                .any(|edge| edge.source() == "table" && edge.target() == "query")
        );
        assert!(
            workflow
                .edges()
                .iter()
                .any(|edge| edge.source() == "query" && edge.target() == "result")
        );
        let WorkflowNodeConfig::Query { sql, .. } = workflow.nodes()[1].config() else {
            unreachable!()
        };
        assert!(sql.contains("{{node:table}}"));
        assert!(!sql.contains("{{node:siglip}}"));
    }

    #[test]
    fn topology_mutation_is_blocked_during_start_command_before_sse_arrives() {
        assert!(
            WorkflowRunPhase::Idle
                .topology_mutation_blocked(Some(WorkflowPendingCommand::Starting))
        );
        assert!(!WorkflowRunPhase::Idle.topology_mutation_blocked(None));
    }

    #[test]
    fn model_projection_preserves_authoritative_progress_and_warning_evidence() {
        let node = spiral_robotics_common::workflow::WorkflowNodeSpec::new(
            "siglip",
            "SigLIP scores",
            WorkflowNodeConfig::Model {
                features: vec![
                    WorkflowFeatureDefinition::new(
                        "is_red_cube_feature",
                        "Probability that a red cube is visible",
                        "red cube",
                    )
                    .with_applicability(WorkflowFeatureApplicability::new(
                        "pick_place.camera_frames",
                        "image",
                    )),
                ],
            },
        );
        let snapshot = WorkflowNodeSnapshot::new(
            node.id(),
            WorkflowNodePhase::CompleteWithWarnings,
            "Complete with warnings",
        )
        .with_progress(WorkflowNodeProgress::new(8, 8, "feature cells"))
        .with_quarantine(WorkflowQuarantineSummary::new(
            1,
            vec!["row 3: image is null".to_owned()],
        ))
        .with_artifacts(None, vec!["1 input row was quarantined.".to_owned()]);
        let mut run = WorkflowRunSnapshot::default();
        run.nodes_mut().push(snapshot.clone());
        let relations = BTreeMap::new();
        let projected = NodeProjection {
            run_snapshot: &run,
            relations: &relations,
        }
        .project(&node, Some(&snapshot));
        assert_eq!(projected.phase(), WorkflowNodePhase::CompleteWithWarnings);
        assert_eq!(projected.progress().unwrap().completed(), 8);
        assert_eq!(projected.quarantine().count(), 1);
        assert_eq!(projected.fields(), ["is_red_cube_feature"]);
    }

    #[test]
    fn dirty_draft_keeps_its_base_revision_for_conflict_detection() {
        let mut draft = WorkflowSpec::default();
        let mut server = draft.clone();
        draft.set_name("Local edit");
        assert!(!draft.revision_conflicts_with(&server));
        server.set_revision(1);
        server.set_name("Remote edit");
        assert!(draft.revision_conflicts_with(&server));
        assert_eq!(
            draft.revision(),
            0,
            "Apply must send the draft base revision"
        );
    }

    #[test]
    fn assistant_receipt_navigation_never_opens_stale_artifacts() {
        let mut state = WorkflowSnapshot::default();
        state.workflow_mut().set_revision(4);
        state.snapshot_mut().set_workflow_revision(4);
        let graph = WorkflowAgentArtifactReceipt::graph("View graph", 4);
        assert_eq!(graph.panel_id(&state, false), Some(CANVAS_PANEL_ID));
        assert_eq!(graph.panel_id(&state, true), None);
        let old_graph = WorkflowAgentArtifactReceipt::graph("View graph", 3);
        assert_eq!(old_graph.panel_id(&state, false), None);

        state.snapshot_mut().set_run_id(Some("run-4".to_owned()));
        state.snapshot_mut().set_phase(WorkflowRunPhase::Completed);
        state.snapshot_mut().set_result(Some(
            spiral_robotics_common::relations::SpiralQuerySnapshot::new(
                Vec::new(),
                Vec::new(),
                false,
            )
            .with_elapsed_ms(1),
        ));
        let results = WorkflowAgentArtifactReceipt::results("View results", "run-4", Some(0));
        assert_eq!(
            results.panel_id(&state, false),
            Some(RESULTS_PANEL_ID),
            "zero-row completed results remain authoritative"
        );
        state.snapshot_mut().set_result_stale(true);
        assert_eq!(results.panel_id(&state, false), None);
        state.snapshot_mut().set_result_stale(false);
        let old_results = WorkflowAgentArtifactReceipt::results("View results", "run-3", Some(0));
        assert_eq!(old_results.panel_id(&state, false), None);
    }

    #[test]
    fn fetched_workflow_revisions_do_not_regress() {
        let mut current = WorkflowSnapshot::default();
        current.set_scope_id("project-a-scope");
        current.workflow_mut().set_revision(2);
        current.snapshot_mut().set_workflow_revision(2);
        let mut stale_bootstrap = current.clone();
        stale_bootstrap.workflow_mut().set_revision(1);
        stale_bootstrap.snapshot_mut().set_workflow_revision(1);
        assert!(!current.accepts(&stale_bootstrap));

        let mut newer = current.clone();
        newer.workflow_mut().set_revision(3);
        newer.snapshot_mut().set_workflow_revision(3);
        assert!(current.accepts(&newer));

        let mut project_b = WorkflowSnapshot::default();
        project_b.set_scope_id("project-b-scope");
        assert!(
            current.accepts(&project_b),
            "a new project scope owns an independent revision sequence"
        );

        let project_a_relations =
            RelationCatalog::new(Vec::new(), false).with_scope("project-a-scope");
        let project_b_relations =
            RelationCatalog::new(Vec::new(), false).with_scope("project-b-scope");
        assert!(!project_a_relations.matches_scope("project-b-scope", "project-a-scope"));
        assert!(project_b_relations.matches_scope("project-b-scope", "project-b-scope"));
    }

    #[test]
    fn fetched_agent_history_rejects_revision_regression() {
        let mut current = WorkflowAgentSnapshot::default();
        current.set_scope_id("project-a-scope");
        current.set_revision(8);
        let mut stale = current.clone();
        stale.set_revision(7);
        assert!(!current.accepts_fetched(&stale));
        let mut next = current.clone();
        next.set_revision(9);
        assert!(current.accepts_fetched(&next));
        next.session_mut().set_workflow_id("other");
        assert!(!current.accepts_fetched(&next));

        let mut project_b = current.clone();
        project_b.set_scope_id("project-b-scope");
        project_b.set_revision(1);
        assert!(
            current.accepts_fetched(&project_b),
            "a new project scope owns an independent revision sequence"
        );
    }

    #[test]
    fn conversation_switching_is_gated_only_by_authoritative_active_work() {
        let mut agent = WorkflowAgentSnapshot::default();
        agent.set_history_available(true);
        agent.conversations_mut().push(
            spiral_robotics_common::workflow::WorkflowAgentConversationSummary::new(
                "conversation-a",
                "Inspect relations",
                0,
            ),
        );
        let mut snapshot = WorkflowRunSnapshot::default();
        assert!(
            AgentConversationChange {
                loaded: true,
                connected: true,
                catalog_available: true,
                command: None,
                agent: &agent,
                snapshot: &snapshot,
            }
            .reason()
            .is_none()
        );

        assert_eq!(
            AgentConversationChange {
                loaded: true,
                connected: true,
                catalog_available: false,
                command: None,
                agent: &agent,
                snapshot: &snapshot,
            }
            .reason()
            .as_deref(),
            Some("Configure or reconnect Spiral before changing conversations.")
        );

        agent
            .session_mut()
            .set_phase(spiral_robotics_common::workflow::WorkflowAgentPhase::Responding);
        assert_eq!(
            AgentConversationChange {
                loaded: true,
                connected: true,
                catalog_available: true,
                command: None,
                agent: &agent,
                snapshot: &snapshot,
            }
            .reason()
            .as_deref(),
            Some("Stop the active Assistant turn before changing conversations.")
        );
        agent
            .session_mut()
            .set_phase(spiral_robotics_common::workflow::WorkflowAgentPhase::Idle);

        snapshot.set_phase(WorkflowRunPhase::Running);
        snapshot.set_run_id(Some("run-agent".to_owned()));
        agent.conversations_mut()[0].set_owned_run_id(Some("run-agent".to_owned()));
        assert_eq!(
            AgentConversationChange {
                loaded: true,
                connected: true,
                catalog_available: true,
                command: None,
                agent: &agent,
                snapshot: &snapshot,
            }
            .reason()
            .as_deref(),
            Some("Stop the Workflow run started by the Assistant before changing conversations.")
        );
        agent.conversations_mut()[0].set_owned_run_id(Some("other-run".to_owned()));
        assert!(
            AgentConversationChange {
                loaded: true,
                connected: true,
                catalog_available: true,
                command: None,
                agent: &agent,
                snapshot: &snapshot,
            }
            .reason()
            .is_none(),
            "a manual or unrelated run does not falsely claim Assistant conversation ownership"
        );
    }

    #[test]
    fn late_run_response_cannot_regress_a_completed_sse_snapshot() {
        let mut current = WorkflowSnapshot::default();
        current
            .snapshot_mut()
            .set_run_id(Some("workflow-run-1".to_owned()));
        current
            .snapshot_mut()
            .set_phase(WorkflowRunPhase::Completed);
        current.snapshot_mut().set_started_ms(10);
        current.snapshot_mut().set_updated_ms(12);
        current.snapshot_mut().set_finished_ms(12);

        let mut late_start = current.clone();
        late_start
            .snapshot_mut()
            .set_phase(WorkflowRunPhase::Validating);
        late_start.snapshot_mut().set_finished_ms(0);
        late_start.snapshot_mut().set_updated_ms(10);
        assert!(!current.accepts(&late_start));

        // Millisecond clock ties still cannot move a terminal run backwards.
        late_start
            .snapshot_mut()
            .set_updated_ms(current.snapshot().updated_ms());
        assert!(!current.accepts(&late_start));
    }

    #[test]
    fn late_cancel_response_cannot_regress_a_cancelled_sse_snapshot() {
        let mut current = WorkflowSnapshot::default();
        current
            .snapshot_mut()
            .set_run_id(Some("workflow-run-1".to_owned()));
        current
            .snapshot_mut()
            .set_phase(WorkflowRunPhase::Cancelled);
        current.snapshot_mut().set_started_ms(10);
        current.snapshot_mut().set_updated_ms(13);
        current.snapshot_mut().set_finished_ms(13);

        let mut late_cancel = current.clone();
        late_cancel
            .snapshot_mut()
            .set_phase(WorkflowRunPhase::Cancelling);
        late_cancel.snapshot_mut().set_finished_ms(0);
        late_cancel.snapshot_mut().set_updated_ms(12);
        assert!(!current.snapshot().accepts(late_cancel.snapshot()));

        late_cancel
            .snapshot_mut()
            .set_updated_ms(current.snapshot().updated_ms());
        assert!(!current.snapshot().accepts(late_cancel.snapshot()));
    }
    #[test]
    fn a_save_receipt_waits_for_the_fetched_revision_and_preserves_later_edits() {
        let submitted = WorkflowSpec::new("workflow", "submitted").with_revision(2);
        let receipt = WorkflowSaveReceipt {
            submitted: submitted.clone(),
            revision: 3,
        };
        assert!(!receipt.acknowledges(&submitted, &submitted));
        let fetched = submitted.clone().with_revision(3);
        assert!(receipt.acknowledges(&submitted, &fetched));
        let edited = WorkflowSpec::new("workflow", "edited during save").with_revision(2);
        assert!(!receipt.acknowledges(&edited, &fetched));
    }
}
