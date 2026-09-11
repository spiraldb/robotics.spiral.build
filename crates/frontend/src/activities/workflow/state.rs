//! Browser-owned workflow document, authoring state, and run observations.
use crate::activities::workflow::WorkflowClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use dioxus::prelude::*;
use dioxus_flow::{Edge, EdgeKind, Node, Side};
use dioxus_workbench::PanelId;
use spiral_robotics_common::relations::{CatalogRelation, RelationCatalog};
use spiral_robotics_common::workflow::{
    WorkflowAgentArtifactReceipt, WorkflowEdgeSpec, WorkflowModelFeature, WorkflowNodeConfig,
    WorkflowNodePhase, WorkflowNodeProgress, WorkflowNodeSnapshot, WorkflowQuarantineSummary,
    WorkflowRunPhase, WorkflowRunSnapshot, WorkflowSpec, WorkflowSummary, WorkflowValidationReport,
};

use super::agent::{WorkflowAgentState, WorkflowAssistant};
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use spiral_robotics_common::workflow::document::{WorkflowDocument, WorkflowRun};

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
    features: Vec<WorkflowModelFeature>,
    image_column: String,
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

    pub(crate) fn sql(&self) -> Option<&str> {
        self.source.sql.as_deref()
    }

    pub(crate) fn detail(&self) -> &str {
        &self.execution.detail
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

    pub(crate) fn features(&self) -> &[WorkflowModelFeature] {
        &self.model.features
    }

    pub(crate) fn image_column(&self) -> &str {
        &self.model.image_column
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
    Saving,
    Selecting,
    Starting,
    Cancelling,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentPendingCommand {
    Sending,
    Cancelling,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct AgentDraftKey(String);

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
    pub(crate) fn new(workflow_id: &str) -> Self {
        Self(workflow_id.to_owned())
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

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct WorkflowController {
    graph: WorkflowGraphSignals,
    document: WorkflowDefinitionSignals,
    lifecycle: WorkflowLifecycleSignals,
    assistant: WorkflowAssistant,
    agent_drafts: Signal<BTreeMap<AgentDraftKey, String>>,
    relation_catalog: ReadSignal<Option<Result<RelationCatalog, String>>>,
}

#[derive(Clone, Copy, PartialEq)]
struct WorkflowGraphSignals {
    nodes: Signal<Vec<Node<WorkflowNodeData>>>,
    edges: Signal<Vec<Edge>>,
    ui: Signal<WorkflowUiState>,
}

#[derive(Clone, Copy, PartialEq)]
struct WorkflowDefinitionSignals {
    baseline: Signal<WorkflowDocument>,
    draft: Signal<WorkflowSpec>,
    runs: Signal<Vec<WorkflowRun>>,
    conflict: Signal<bool>,
}
#[derive(Clone, Copy, PartialEq)]
struct WorkflowLifecycleSignals {
    loaded: Signal<bool>,
    command: Signal<Option<WorkflowPendingCommand>>,
    load_error: Signal<Option<String>>,
    workflows: Signal<Vec<WorkflowSummary>>,
}

impl WorkflowController {
    pub(crate) fn use_controller(
        relation_catalog: ReadSignal<Option<Result<RelationCatalog, String>>>,
    ) -> Self {
        let initial = use_hook(Self::fresh_document);
        let initial_spec = initial.workflow().clone();
        let snapshot = WorkflowRunSnapshot::idle_for(&initial_spec);
        let initial_nodes = initial_spec.graph_nodes(&snapshot, &[]);
        let initial_edges = initial_spec.graph_edges(&snapshot);
        let controller = use_hook(|| Self {
            graph: WorkflowGraphSignals {
                nodes: Signal::new(initial_nodes),
                edges: Signal::new(initial_edges),
                ui: Signal::new(WorkflowUiState::default()),
            },
            assistant: WorkflowAssistant::new(initial_spec.id()),
            agent_drafts: Signal::new(BTreeMap::new()),
            document: WorkflowDefinitionSignals {
                baseline: Signal::new(initial),
                draft: Signal::new(initial_spec),
                runs: Signal::new(Vec::new()),
                conflict: Signal::new(false),
            },
            lifecycle: WorkflowLifecycleSignals {
                loaded: Signal::new(false),
                command: Signal::new(None),
                load_error: Signal::new(None),
                workflows: Signal::new(Vec::new()),
            },
            relation_catalog,
        });
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
        Self::project_run(&self.document.draft.read(), &self.document.runs.read())
    }
    fn project_run(draft: &WorkflowSpec, runs: &[WorkflowRun]) -> WorkflowRunSnapshot {
        let Some(run) = runs
            .iter()
            .filter(|run| run.workflow().id() == draft.id())
            .max_by_key(|run| run.snapshot().started_ms())
        else {
            return WorkflowRunSnapshot::idle_for(draft);
        };
        let mut snapshot = run.snapshot().clone();
        let mut submitted = run.workflow().clone();
        submitted.set_revision(draft.revision());
        snapshot.set_result_stale(&submitted != draft);
        snapshot
    }
    pub(crate) fn executed_graph(&self) -> Option<WorkflowSpec> {
        let id = self.document.draft.read().id().to_owned();
        self.document
            .runs
            .read()
            .iter()
            .filter(|run| run.workflow().id() == id)
            .max_by_key(|run| run.snapshot().started_ms())
            .map(|run| run.workflow().clone())
    }
    pub(crate) fn editor_status(&self) -> Option<WorkflowEditorStatus> {
        self.loaded()
            .then(|| WorkflowEditorStatus::new(self.spec(), self.snapshot()))
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

    pub(crate) fn agent_state(&self) -> WorkflowAgentState {
        self.assistant.snapshot(self.loaded())
    }
    pub(crate) fn agent_loaded(&self) -> bool {
        self.assistant.loaded()
    }
    pub(crate) fn agent_command(&self) -> Option<AgentPendingCommand> {
        self.agent_state()
            .session()
            .phase()
            .active()
            .then_some(AgentPendingCommand::Sending)
    }
    pub(crate) fn agent_error(&self) -> Option<String> {
        self.agent_state().session().error().map(str::to_owned)
    }
    pub(crate) const fn agent_drafts(self) -> Signal<BTreeMap<AgentDraftKey, String>> {
        self.agent_drafts
    }
    pub(crate) fn workflows(&self) -> Vec<WorkflowSummary> {
        let mut workflows = self.lifecycle.workflows.read().clone();
        for run in self.document.runs.read().iter() {
            if !workflows
                .iter()
                .any(|workflow| workflow.id() == run.workflow().id())
            {
                workflows.push(WorkflowSummary::new(
                    run.workflow().id(),
                    format!("{} (run)", run.workflow().name()),
                ));
            }
        }
        workflows
    }

    pub(crate) fn workflow_selection_reason(&self) -> Option<String> {
        if !self.loaded() || !self.agent_loaded() {
            return Some("Loading workflows…".to_owned());
        }
        if self.command().is_some() || self.agent_command().is_some() {
            return Some("Wait for the current action to finish.".to_owned());
        }
        if self.agent_state().session().phase().active() {
            return Some("Stop the active work before switching workflows.".to_owned());
        }
        if self.unsaved_changes() {
            return Some("Save or discard your changes before switching workflows.".to_owned());
        }
        if !crate::shell::project::ProjectSessionSignals::current().catalog_available() {
            return Some("Reconnect Spiral before switching workflows.".to_owned());
        }
        None
    }

    pub(crate) fn select_dataset(
        mut self,
        source: spiral_robotics_common::datasets::DatasetSummary,
        _studio: StudioProjectState,
    ) {
        if self.workflow_selection_reason().is_some() {
            return;
        }
        let mut draft = self.document.draft.write();
        draft.nodes_mut().clear();
        draft.edges_mut().clear();
        draft
            .nodes_mut()
            .push(spiral_robotics_common::workflow::WorkflowNodeSpec::new(
                "source",
                source.selection().label(),
                WorkflowNodeConfig::Query {
                    sql: format!(
                        "SELECT * FROM spql.mcap.episodes('{}')",
                        source.selection().sql_name().replace('\'', "''")
                    ),
                    max_rows: 100,
                },
            ));
        draft
            .nodes_mut()
            .push(spiral_robotics_common::workflow::WorkflowNodeSpec::new(
                "result",
                "Results",
                WorkflowNodeConfig::Result,
            ));
        draft
            .edges_mut()
            .push(WorkflowEdgeSpec::new("source", "result"));
        drop(draft);
        self.sync_graph();
    }

    pub(crate) fn set_name(mut self, name: String) {
        self.document.draft.write().set_name(name);
    }

    pub(crate) fn dirty(&self) -> bool {
        *self.document.draft.read() != *self.document.baseline.read().workflow()
    }

    pub(crate) fn revision_conflict(&self) -> bool {
        *self.document.conflict.read()
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
        if self
            .document
            .runs
            .read()
            .iter()
            .any(|run| run.snapshot().phase().active())
        {
            return Some(
                "A workflow is already running. Open its graph to inspect or cancel it.".to_owned(),
            );
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

    pub(crate) fn save_disabled_reason(&self) -> Option<String> {
        if !self.loaded() || !self.agent_loaded() {
            return Some("Loading workflow…".into());
        }
        if let Some(error) = self.load_error() {
            return Some(format!("Could not load the saved Workflow: {error}"));
        }
        if self.revision_conflict() {
            return Some(
                "The workflow changed in another session. Reload the current graph before editing again."
                    .to_owned(),
            );
        }
        if self.command().is_some() {
            return Some("Wait for the current document operation.".to_owned());
        }
        if self.agent_state().session().phase().active() || self.agent_command().is_some() {
            return Some("Wait for the active Assistant turn before saving.".to_owned());
        }
        None
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
        (receipt.revision() == self.spec().revision()).then(|| PanelId::from(CANVAS_PANEL_ID))
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

    pub(crate) fn reload_current(mut self) {
        self.lifecycle.load_error.set(None);
        let id = self.spec().id().to_owned();
        let known = self.workflows().iter().any(|workflow| workflow.id() == id);
        if !self.loaded() {
            self.load_initial();
        } else if known {
            self.open_document(Some(id));
        } else {
            self.install(Self::fresh_document());
        }
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

    pub(crate) fn set_model_image(mut self, node_id: &str, value: String) {
        self.document.draft.write().edit_node(node_id, |config| {
            let WorkflowNodeConfig::Model { image_column, .. } = config else {
                return false;
            };
            *image_column = value;
            true
        });
        self.sync_graph();
    }

    pub(crate) fn set_model_feature(
        mut self,
        target: ModelFeatureTarget<'_>,
        edit: impl FnOnce(&mut WorkflowModelFeature),
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
            features.push(WorkflowModelFeature::new("", "", ""));
            true
        });
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

    pub(crate) fn unsaved_changes(&self) -> bool {
        self.dirty()
            || self.assistant.conversation() != *self.document.baseline.read().conversation()
    }
    fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        dioxus::dioxus_core::Runtime::current().spawn(self.document.draft.origin_scope(), task);
    }
    fn fresh_document() -> WorkflowDocument {
        WorkflowDocument::new(WorkflowSpec::new(
            format!(
                "workflow-{}-{:x}",
                js_sys::Date::now() as u64,
                (js_sys::Math::random() * u64::MAX as f64) as u64
            ),
            "Untitled workflow",
        ))
    }
    fn install(mut self, document: WorkflowDocument) {
        self.assistant.install(document.conversation().clone());
        self.document.draft.set(document.workflow().clone());
        self.document.baseline.set(document);
        self.document.conflict.set(false);
        self.graph.ui.set(WorkflowUiState::default());
        self.lifecycle.loaded.set(true);
        self.lifecycle.load_error.set(None);
        self.sync_graph();
    }
    pub(super) fn replace_authored_graph(
        mut self,
        expected: &WorkflowSpec,
        workflow: WorkflowSpec,
    ) -> Result<(), String> {
        self.document
            .draft
            .write()
            .replace_authored(expected, workflow)?;
        self.sync_graph();
        Ok(())
    }
    pub(crate) fn save(mut self, studio: StudioProjectState) {
        if self.save_disabled_reason().is_some() {
            return;
        }
        let submitted =
            WorkflowDocument::new(self.spec()).with_conversation(self.assistant.conversation());
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Saving));
        self.spawn(async move {
            let client = WorkflowClient::current();
            match client.save(&submitted).await {
                Ok(saved) => {
                    // A save acknowledges its input. Edits made while it was in flight stay local.
                    if self.spec().id() == saved.workflow().id() {
                        self.document
                            .draft
                            .write()
                            .acknowledge_save(saved.workflow());
                        self.document.baseline.set(saved);
                        self.document.conflict.set(false);
                    }
                    if let Ok(workflows) = client.list().await {
                        self.lifecycle.workflows.set(workflows);
                    }
                }
                Err(error) => {
                    self.document.conflict.set(error.code() == spiral_robotics_common::command_failure::CommandFailureCode::Conflict);
                    studio.report_from(MessageSource::Workflow, error.to_string());
                }
            }
            self.lifecycle.command.set(None);
        });
    }
    pub(crate) fn discard(self, _studio: StudioProjectState) {
        if self.command().is_some() {
            return;
        }
        let baseline = self.document.baseline.peek().clone();
        self.install(baseline);
    }
    pub(crate) fn run(mut self, studio: StudioProjectState) {
        if let Some(reason) = self.run_disabled_reason() {
            studio.report_warning_from(MessageSource::Workflow, reason);
            return;
        }
        let workflow = self.spec();
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Starting));
        self.spawn(async move {
            match WorkflowClient::current().start(&workflow).await {
                Ok(run) => self.accept_runs(vec![run]),
                Err(error) => studio.report_from(MessageSource::Workflow, error.to_string()),
            }
            self.lifecycle.command.set(None);
            QueryRefresh::current().invalidate(&[QueryDomain::Workflow]);
        });
    }
    pub(crate) fn cancel(mut self, studio: StudioProjectState) {
        let Some(id) = self.snapshot().run_id().map(str::to_owned) else {
            return;
        };
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Cancelling));
        self.spawn(async move {
            match WorkflowClient::current().cancel(&id).await {
                Ok(run) => self.accept_runs(vec![run]),
                Err(error) => studio.report_from(MessageSource::Workflow, error.to_string()),
            }
            self.lifecycle.command.set(None);
        });
    }
    pub(crate) fn select_workflow(self, workflow_id: Option<String>, _studio: StudioProjectState) {
        if self.workflow_selection_reason().is_none() {
            self.open_document(workflow_id);
        }
    }
    fn open_document(mut self, id: Option<String>) {
        let Some(id) = id else {
            self.install(Self::fresh_document());
            return;
        };
        if !self
            .lifecycle
            .workflows
            .peek()
            .iter()
            .any(|workflow| workflow.id() == id)
        {
            let run = self
                .document
                .runs
                .peek()
                .iter()
                .filter(|run| run.workflow().id() == id)
                .max_by_key(|run| run.snapshot().started_ms())
                .cloned();
            if let Some(run) = run {
                let mut document = WorkflowDocument::new(run.workflow().clone());
                document.workflow_mut().set_revision(0);
                self.install(document);
                return;
            }
        }
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Selecting));
        self.spawn(async move {
            match WorkflowClient::current().load(&id).await {
                Ok(document) => self.install(document),
                Err(error) => self.lifecycle.load_error.set(Some(error.to_string())),
            }
            self.lifecycle.command.set(None);
        });
    }
    pub(crate) fn send_agent(mut self, text: String, _studio: StudioProjectState) {
        if !self.agent_state().readiness().configured()
            || self.command().is_some()
            || self.agent_command().is_some()
        {
            return;
        }
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.assistant.send(self, text.clone());
        AgentDraftKey::new(self.spec().id())
            .clear_if_unchanged(&mut self.agent_drafts.write(), &text);
    }
    pub(crate) fn cancel_agent(self, _studio: StudioProjectState) {
        self.assistant.cancel();
    }
    pub(crate) fn retry_agent(self, studio: StudioProjectState) {
        if let Some(message) =
            self.agent_state()
                .session()
                .messages()
                .iter()
                .rev()
                .find(|message| {
                    message.role() == spiral_robotics_common::workflow::WorkflowAgentRole::User
                })
        {
            self.send_agent(message.text().to_owned(), studio);
        }
    }
    fn load_initial(mut self) {
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Selecting));
        self.spawn(async move {
            let client = WorkflowClient::current();
            let result = async {
                let workflows = client.list().await?;
                let document = match workflows.first() {
                    Some(workflow) => client.load(workflow.id()).await?,
                    None => Self::fresh_document(),
                };
                Ok::<_, crate::client::transport::CommandError>((workflows, document))
            }
            .await;
            match result {
                Ok((workflows, document)) => {
                    self.lifecycle.workflows.set(workflows);
                    self.install(document);
                }
                Err(error) => self.lifecycle.load_error.set(Some(error.to_string())),
            }
            self.lifecycle.command.set(None);
        });
    }
    pub(crate) fn use_polling(self, is_active: ReadSignal<bool>) {
        let sessions = crate::shell::project::ProjectSessionSignals::current();
        use_effect(move || {
            if sessions.catalog_available()
                && !self.loaded()
                && self.command().is_none()
                && self.load_error().is_none()
            {
                self.load_initial();
            }
        });
        use_hook(move || {
            self.spawn(async move {
                self.assistant.connect().await;
            });
        });
        let active = use_memo(move || {
            self.document
                .runs
                .read()
                .iter()
                .any(|run| run.snapshot().phase().active())
                || self.command().is_some()
        });
        let runs = QueryPolicy::new(QueryDomain::Workflow, 500)
            .with_activity(active.into())
            .use_when(
                move || is_active() && sessions.catalog_available(),
                move || {
                    let active = self
                        .document
                        .runs
                        .peek()
                        .iter()
                        .find(|run| run.snapshot().phase().active())
                        .and_then(|run| run.snapshot().run_id())
                        .map(str::to_owned);
                    async move {
                        let client = WorkflowClient::current();
                        match active {
                            Some(id) => client.run(&id).await.map(|run| vec![run]),
                            None => client.runs().await,
                        }
                    }
                },
            );
        self.use_run_observations(runs);
    }

    fn use_run_observations(
        self,
        runs: Resource<Result<Vec<WorkflowRun>, crate::client::CommandError>>,
    ) {
        use_effect(move || {
            // Subscribe even while the first request is pending; finished() is nonreactive.
            let records = runs.read();
            if runs.finished()
                && let Some(Ok(records)) = records.as_ref()
            {
                self.accept_runs(records.clone());
            }
        });
    }
    fn accept_runs(mut self, incoming: Vec<WorkflowRun>) {
        let changed = incoming.iter().any(|run| {
            self.document
                .runs
                .peek()
                .iter()
                .find(|current| current.snapshot().run_id() == run.snapshot().run_id())
                .is_none_or(|current| current != run && current.snapshot().accepts(run.snapshot()))
        });
        if !changed {
            return;
        }
        let mut runs = self.document.runs.write();
        for run in incoming {
            match runs
                .iter_mut()
                .find(|current| current.snapshot().run_id() == run.snapshot().run_id())
            {
                Some(current) if current.snapshot().accepts(run.snapshot()) => *current = run,
                Some(_) => {}
                None => runs.push(run),
            }
        }
        drop(runs);
        self.sync_graph();
    }

    fn sync_graph(mut self) {
        let spec = self.document.draft.peek().to_owned();
        let snapshot = Self::project_run(&spec, &self.document.runs.peek());
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
        self.relation_catalog
            .read_unchecked()
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .cloned()
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
    fn replace_authored(
        &mut self,
        expected: &WorkflowSpec,
        workflow: WorkflowSpec,
    ) -> Result<(), String>;
    fn acknowledge_save(&mut self, saved: &WorkflowSpec);

    fn edit_node(
        &mut self,
        node_id: &str,
        edit: impl FnOnce(&mut WorkflowNodeConfig) -> bool,
    ) -> bool;
    fn insert_siglip_model(&mut self) -> Option<String>;
    fn remove_siglip_model(&mut self, node_id: &str) -> Option<String>;
}

impl WorkflowDraft for WorkflowSpec {
    fn replace_authored(
        &mut self,
        expected: &WorkflowSpec,
        workflow: WorkflowSpec,
    ) -> Result<(), String> {
        if self != expected || workflow.id() != expected.id() {
            return Err("The draft changed during authoring. Your edits were preserved.".into());
        }
        *self = workflow;
        Ok(())
    }
    fn acknowledge_save(&mut self, saved: &WorkflowSpec) {
        if self.id() == saved.id() {
            self.set_revision(saved.revision());
        }
    }

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
        let edge_index = self.edges().iter().position(|edge| {
            self.nodes().iter().any(|node| {
                node.id() == edge.target() && matches!(node.config(), WorkflowNodeConfig::Result)
            })
        })?;
        let edge = self.edges_mut().remove(edge_index);
        let mut suffix = 1;
        let node_id = loop {
            let id = format!("siglip_{suffix}");
            if !self.nodes().iter().any(|node| node.id() == id) {
                break id;
            }
            suffix += 1;
        };
        self.nodes_mut()
            .push(spiral_robotics_common::workflow::WorkflowNodeSpec::new(
                node_id.clone(),
                "Score images",
                WorkflowNodeConfig::Model {
                    image_column: "image".into(),
                    features: vec![WorkflowModelFeature::new("", "", "")],
                },
            ));
        self.edges_mut().extend([
            WorkflowEdgeSpec::new(edge.source(), node_id.clone()),
            WorkflowEdgeSpec::new(node_id.clone(), edge.target()),
        ]);
        Some(node_id)
    }

    fn remove_siglip_model(&mut self, node_id: &str) -> Option<String> {
        let parent = self
            .edges()
            .iter()
            .find(|edge| edge.target() == node_id)?
            .source()
            .to_owned();
        let children = self
            .edges()
            .iter()
            .filter(|edge| edge.source() == node_id)
            .map(|edge| edge.target().to_owned())
            .collect::<Vec<_>>();
        self.nodes_mut().retain(|node| node.id() != node_id);
        self.edges_mut()
            .retain(|edge| edge.source() != node_id && edge.target() != node_id);
        for child in children {
            self.edges_mut()
                .push(WorkflowEdgeSpec::new(parent.clone(), child.clone()));
            self.edit_node(&child, |config| {
                if let WorkflowNodeConfig::Query { sql, .. } = config {
                    *sql = sql.replace(
                        &format!("{{{{node:{node_id}}}}}"),
                        &format!("{{{{node:{parent}}}}}"),
                    );
                }
                true
            });
        }
        Some(parent)
    }
}

trait WorkflowRunPhaseEditing {
    fn topology_mutation_blocked(self, command: Option<WorkflowPendingCommand>) -> bool;
}

impl WorkflowRunPhaseEditing for WorkflowRunPhase {
    fn topology_mutation_blocked(self, command: Option<WorkflowPendingCommand>) -> bool {
        command.is_some()
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
                    model: WorkflowNodeModel::default(),
                }
            }
            WorkflowNodeConfig::Model {
                image_column,
                features,
            } => WorkflowNodeData {
                kind: WorkflowNodeKind::Model,
                description:
                    "SigLIP appends scores to its input rows. Every run recomputes inference."
                        .to_owned(),
                source: WorkflowNodeSource {
                    fields: features
                        .iter()
                        .map(|feature| feature.name().trim())
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    ..WorkflowNodeSource::default()
                },
                execution,
                model: WorkflowNodeModel {
                    features: features.clone(),
                    image_column: image_column.clone(),
                },
            },
            WorkflowNodeConfig::Query { sql, .. } => WorkflowNodeData {
                kind: WorkflowNodeKind::Query,
                description:
                    "Bounded read-only Spiral SQL with server-owned relation placeholders."
                        .to_owned(),
                source: WorkflowNodeSource {
                    sql: Some(sql.clone()),
                    ..WorkflowNodeSource::default()
                },
                execution,
                model: WorkflowNodeModel::default(),
            },
            WorkflowNodeConfig::Result => WorkflowNodeData {
                kind: WorkflowNodeKind::Result,
                description: "Terminal media-aware query result.".to_owned(),
                source: WorkflowNodeSource {
                    fields: self
                        .run_snapshot
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
                model: WorkflowNodeModel::default(),
            },
        }
    }
}

/// Local projection for the shell; it is never transported over HTTP.
#[derive(Clone, PartialEq)]
pub(crate) struct WorkflowEditorStatus {
    workflow: WorkflowSpec,
    snapshot: WorkflowRunSnapshot,
}
impl WorkflowEditorStatus {
    pub(crate) fn new(workflow: WorkflowSpec, snapshot: WorkflowRunSnapshot) -> Self {
        Self { workflow, snapshot }
    }
    pub(crate) fn workflow(&self) -> &WorkflowSpec {
        &self.workflow
    }
    pub(crate) fn snapshot(&self) -> &WorkflowRunSnapshot {
        &self.snapshot
    }
}

#[cfg(test)]
mod run_observation_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn explicit_selection_activates_the_node_inspector() {
        let selected =
            WorkflowUiState::default().reduce(WorkflowUiAction::SelectNode("query".to_owned()));
        assert_eq!(selected.selected_node.as_deref(), Some("query"));
        assert_eq!(selected.active_panel.as_deref(), Some(NODE_PANEL_ID));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn default_editor_is_table_query_result_without_a_fake_model() {
        let graph = WorkflowSpec::default();
        let state = WorkflowEditorStatus::new(graph.clone(), WorkflowRunSnapshot::idle_for(&graph));
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

    #[wasm_bindgen_test::wasm_bindgen_test]
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

    #[wasm_bindgen_test::wasm_bindgen_test]
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn model_insertion_and_removal_preserve_the_existing_sql() {
        let mut graph = WorkflowSpec::default();
        let before = graph.clone();
        let id = graph.insert_siglip_model().unwrap();
        assert_eq!(graph.nodes().len(), 4);
        let model = graph.nodes().iter().find(|node| node.id() == id).unwrap();
        assert!(
            matches!(model.config(), WorkflowNodeConfig::Model { image_column, .. } if image_column == "image")
        );
        assert_eq!(graph.nodes()[1].config(), before.nodes()[1].config());
        graph.remove_siglip_model(&id).unwrap();
        assert_eq!(graph, before);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn topology_mutation_is_blocked_during_start_command_before_sse_arrives() {
        assert!(
            WorkflowRunPhase::Idle
                .topology_mutation_blocked(Some(WorkflowPendingCommand::Starting))
        );
        assert!(!WorkflowRunPhase::Idle.topology_mutation_blocked(None));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn an_inflight_save_updates_the_baseline_version_without_losing_new_edits() {
        let mut draft = WorkflowSpec::default();
        let mut saved = draft.clone();
        saved.set_revision(1);
        draft.set_name("Typed during Save");
        draft.acknowledge_save(&saved);
        assert_eq!(draft.name(), "Typed during Save");
        assert_eq!(draft.revision(), 1);
        assert_ne!(draft, saved);
        let other = WorkflowSpec::new("other", "Another document").with_revision(20);
        draft.acknowledge_save(&other);
        assert_eq!(draft.revision(), 1);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_late_assistant_proposal_preserves_manual_edits() {
        let expected = WorkflowSpec::default();
        let mut draft = expected.clone();
        let mut proposed = expected.clone();
        proposed.set_name("Assistant proposal");
        draft.set_name("User edit while planning");
        assert!(draft.replace_authored(&expected, proposed.clone()).is_err());
        assert_eq!(draft.name(), "User edit while planning");
        let latest = draft.clone();
        draft.replace_authored(&latest, proposed.clone()).unwrap();
        assert_eq!(draft, proposed);
        assert!(!WorkflowRunPhase::Running.topology_mutation_blocked(None));
    }
}
