//! Serde-stable Workflow API and persistence contract.
//!
//! Workflow starts at relations already materialized in Spiral. External
//! ingestion remains outside Workflow; executable Model nodes consume those
//! relations and write server-derived relations for downstream Query nodes.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use spiral_robotics_core::query::{ReadOnlyQuery, ReadOnlyQueryMode, UnquotedSqlIdentifier};
use spiral_robotics_core::workflow::{QualifiedRelation, WorkflowNodeId, WorkflowSql};

use crate::relations::{QueryColumn, SpiralQueryResponse};

pub const DEFAULT_WORKFLOW_ID: &str = "default";
pub const MAX_WORKFLOW_QUERY_ROWS: usize = 1_000;
pub const SIGLIP_BASE_MODEL_ID: &str = "siglip-base";
pub const SIGLIP_BASE_REPOSITORY: &str = "google/siglip-base-patch16-224";
pub const SIGLIP_BASE_REVISION: &str = "7fd15f0689c79d79e38b1c2e2e2370a7bf2761ed";
pub const MAX_SIGLIP_FEATURES: usize = 32;
pub const MAX_SIGLIP_CONCEPT_LENGTH: usize = 80;
pub const MAX_WORKFLOW_FEATURE_NAME_LENGTH: usize = 96;
pub const MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH: usize = 240;
pub const MAX_WORKFLOW_FEATURE_STREAMS: usize = 8;
pub const WORKFLOW_FEATURE_REGISTRY_RELATION: &str = "workflow.feature_registry";
pub const MAX_WORKFLOW_AGENT_INPUT_CHARS: usize = 8_000;
pub const MAX_WORKFLOW_AGENT_CONVERSATIONS: usize = 20;
pub const MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION: usize = 80;
pub const MAX_WORKFLOW_AGENT_CONVERSATION_TITLE_CHARS: usize = 64;
pub const MAX_WORKFLOW_AGENT_QUERY_ROWS: usize = 50;

pub fn derived_feature_cache_relation(source_relation: &str) -> String {
    format!(
        "workflow.features_{}",
        spiral_robotics_core::identity::SqlNameComponent::from(source_relation).as_str()
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowSpec {
    pub id: String,
    pub name: String,
    pub revision: u64,
    #[serde(default)]
    pub nodes: Vec<WorkflowNodeSpec>,
    #[serde(default)]
    pub edges: Vec<WorkflowEdgeSpec>,
}

impl Default for WorkflowSpec {
    fn default() -> Self {
        Self {
            id: DEFAULT_WORKFLOW_ID.to_owned(),
            name: "Relation query".to_owned(),
            revision: 0,
            nodes: vec![
                WorkflowNodeSpec {
                    id: "table".to_owned(),
                    name: "Choose a Spiral table".to_owned(),
                    config: WorkflowNodeConfig::Table {
                        relation: String::new(),
                    },
                },
                WorkflowNodeSpec {
                    id: "query".to_owned(),
                    name: "Read relation".to_owned(),
                    config: WorkflowNodeConfig::Query {
                        sql: "SELECT * FROM {{node:table}} LIMIT 100".to_owned(),
                        max_rows: 100,
                    },
                },
                WorkflowNodeSpec {
                    id: "result".to_owned(),
                    name: "Results".to_owned(),
                    config: WorkflowNodeConfig::Result,
                },
            ],
            edges: vec![
                WorkflowEdgeSpec {
                    source: "table".to_owned(),
                    target: "query".to_owned(),
                },
                WorkflowEdgeSpec {
                    source: "query".to_owned(),
                    target: "result".to_owned(),
                },
            ],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowNodeSpec {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub config: WorkflowNodeConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowNodeConfig {
    Table {
        relation: String,
    },
    /// The server derives `feature_cache_relation`; callers do not choose a
    /// writable identifier.
    Model {
        model_id: String,
        features: Vec<WorkflowFeatureDefinition>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feature_cache_relation: Option<String>,
    },
    Query {
        sql: String,
        #[serde(default = "default_workflow_query_rows")]
        max_rows: usize,
    },
    Result,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowFeatureDefinition {
    pub name: String,
    pub description: String,
    pub concept: String,
    pub applicability: WorkflowFeatureApplicability,
}

impl WorkflowFeatureDefinition {
    /// Whether this feature's output column has a safe, descriptive name.
    pub fn has_valid_name(&self) -> bool {
        UnquotedSqlIdentifier::try_from(self.name.as_str()).is_ok()
            && self.name.ends_with("_feature")
            && self.name.len() <= MAX_WORKFLOW_FEATURE_NAME_LENGTH
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowFeatureApplicability {
    pub source_relation: String,
    pub media_field: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_ids: Vec<String>,
}

fn default_workflow_query_rows() -> usize {
    100
}

impl WorkflowNodeConfig {
    pub const fn kind(&self) -> WorkflowNodeKind {
        match self {
            Self::Table { .. } => WorkflowNodeKind::Table,
            Self::Model { .. } => WorkflowNodeKind::Model,
            Self::Query { .. } => WorkflowNodeKind::Query,
            Self::Result => WorkflowNodeKind::Result,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeKind {
    Table,
    Model,
    Query,
    Result,
}

impl WorkflowNodeKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Table => "Table",
            Self::Model => "Model",
            Self::Query => "Query",
            Self::Result => "Result",
        }
    }

    const fn allows_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Table, Self::Query)
                | (Self::Query, Self::Model)
                | (Self::Query, Self::Query)
                | (Self::Model, Self::Query)
                | (Self::Query, Self::Result)
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowEdgeSpec {
    pub source: String,
    pub target: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowValidationReport {
    #[serde(default)]
    pub errors: Vec<WorkflowValidationIssue>,
    #[serde(default)]
    pub warnings: Vec<WorkflowValidationIssue>,
}

impl WorkflowValidationReport {
    pub fn valid(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowValidationIssue {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub message: String,
    pub recovery: String,
}

impl WorkflowValidationIssue {
    fn new(code: &str, message: &str, recovery: &str) -> Self {
        Self {
            code: code.to_owned(),
            node_id: None,
            message: message.to_owned(),
            recovery: recovery.to_owned(),
        }
    }

    fn with_node_id(mut self, node_id: &str) -> Self {
        self.node_id = Some(node_id.to_owned());
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SaveWorkflowRequest {
    pub scope_id: String,
    pub expected_revision: u64,
    pub workflow: WorkflowSpec,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunWorkflowRequest {
    pub scope_id: String,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CancelWorkflowRequest {
    pub scope_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkflowStateResponse {
    /// Ephemeral server identity for the active Spiral destination/project.
    /// Revisions are monotonic only within one scope.
    #[serde(default)]
    pub scope_id: String,
    pub workflow: WorkflowSpec,
    pub validation: WorkflowValidationReport,
    pub snapshot: WorkflowRunSnapshot,
}

impl Default for WorkflowStateResponse {
    fn default() -> Self {
        let workflow = WorkflowSpec::default();
        Self {
            scope_id: String::new(),
            validation: workflow.validate(),
            snapshot: WorkflowRunSnapshot::idle_for(&workflow),
            workflow,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkflowRunSnapshot {
    pub workflow_id: String,
    pub workflow_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub phase: WorkflowRunPhase,
    #[serde(default)]
    pub nodes: Vec<WorkflowNodeSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<SpiralQueryResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub result_stale: bool,
    #[serde(default)]
    pub started_ms: u64,
    #[serde(default)]
    pub finished_ms: u64,
    #[serde(default)]
    pub updated_ms: u64,
}

impl WorkflowRunSnapshot {
    pub fn idle_for(workflow: &WorkflowSpec) -> Self {
        Self {
            workflow_id: workflow.id.clone(),
            workflow_revision: workflow.revision,
            phase: WorkflowRunPhase::Idle,
            nodes: workflow
                .nodes
                .iter()
                .map(|node| WorkflowNodeSnapshot {
                    node_id: node.id.clone(),
                    phase: WorkflowNodePhase::Pending,
                    detail: "Not run".to_owned(),
                    ..WorkflowNodeSnapshot::default()
                })
                .collect(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunPhase {
    #[default]
    Idle,
    Validating,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl WorkflowRunPhase {
    pub const fn active(self) -> bool {
        matches!(self, Self::Validating | Self::Running | Self::Cancelling)
    }

    pub const fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowNodeSnapshot {
    pub node_id: String,
    pub phase: WorkflowNodePhase,
    pub detail: String,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<WorkflowNodeProgress>,
    #[serde(default)]
    pub quarantine: WorkflowQuarantineSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<WorkflowModelRunMetadata>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowNodeProgress {
    pub completed: u64,
    pub total: u64,
    pub unit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowModelRunMetadata {
    pub model_id: String,
    pub artifact_revision: String,
    pub feature_cache_relation: String,
    #[serde(default)]
    pub features: Vec<WorkflowFeatureDefinition>,
    pub fingerprint: String,
    pub reused: bool,
    pub device: String,
    #[serde(default)]
    pub input_rows: u64,
    #[serde(default)]
    pub output_rows: u64,
    #[serde(default)]
    pub decode_ms: u64,
    #[serde(default)]
    pub inference_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodePhase {
    #[default]
    Pending,
    Validating,
    Loading,
    Running,
    Complete,
    CompleteWithWarnings,
    Failed,
    Stale,
    Cancelling,
    Cancelled,
    Unsupported,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkflowUpdate {
    pub state: WorkflowStateResponse,
}

/// Public, secret-free readiness for the backend-owned Workflow agent.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentReadiness {
    pub configured: bool,
    pub status: WorkflowAgentReadinessStatus,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentReadinessStatus {
    #[default]
    Unconfigured,
    Ready,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentRole {
    User,
    Assistant,
    Synthetic,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentMessagePhase {
    Streaming,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentToolPhase {
    Running,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentArtifactKind {
    Graph,
    Run,
    Results,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentArtifactReceipt {
    pub kind: WorkflowAgentArtifactKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_count: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentToolSummary {
    pub call_id: String,
    pub name: String,
    pub phase: WorkflowAgentToolPhase,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<WorkflowAgentArtifactReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentMessage {
    pub id: String,
    pub sequence: u64,
    pub role: WorkflowAgentRole,
    pub phase: WorkflowAgentMessagePhase,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tools: Vec<WorkflowAgentToolSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default)]
    pub created_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentPhase {
    #[default]
    Idle,
    Responding,
    WaitingForRun,
    Cancelling,
    Failed,
    Cancelled,
    Interrupted,
}

impl WorkflowAgentPhase {
    pub const fn active(self) -> bool {
        matches!(
            self,
            Self::Responding | Self::WaitingForRun | Self::Cancelling
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentSession {
    pub workflow_id: String,
    #[serde(default, alias = "session_id")]
    pub conversation_id: String,
    pub revision: u64,
    pub phase: WorkflowAgentPhase,
    #[serde(default)]
    pub messages: Vec<WorkflowAgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub updated_ms: u64,
}

impl Default for WorkflowAgentSession {
    fn default() -> Self {
        Self {
            workflow_id: DEFAULT_WORKFLOW_ID.to_owned(),
            conversation_id: String::new(),
            revision: 0,
            phase: WorkflowAgentPhase::Idle,
            messages: Vec::new(),
            active_turn_id: None,
            deferred_run_id: None,
            error: None,
            updated_ms: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentConversationSummary {
    pub conversation_id: String,
    pub title: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub message_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_run_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentStateResponse {
    pub readiness: WorkflowAgentReadiness,
    pub history_available: bool,
    pub scope_id: String,
    pub revision: u64,
    pub active_conversation_id: String,
    #[serde(default)]
    pub conversations: Vec<WorkflowAgentConversationSummary>,
    pub conversation_limit: usize,
    pub session: WorkflowAgentSession,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentTurnRequest {
    pub scope_id: String,
    pub conversation_id: String,
    pub text: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentCreateConversationRequest {
    pub scope_id: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentActivateConversationRequest {
    pub scope_id: String,
    pub conversation_id: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentCancelRequest {
    pub scope_id: String,
    pub conversation_id: String,
    pub turn_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentTurnAccepted {
    pub turn_id: String,
    pub state: WorkflowAgentStateResponse,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentUpdate {
    pub state: WorkflowAgentStateResponse,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowCapabilities {
    pub schema_version: u32,
    pub node_kinds: Vec<WorkflowNodeCapability>,
    pub max_query_rows: usize,
    pub relation_discovery: String,
    #[serde(default)]
    pub models: Vec<WorkflowModelCapability>,
}

impl Default for WorkflowCapabilities {
    fn default() -> Self {
        Self {
            schema_version: 3,
            node_kinds: vec![
                WorkflowNodeCapability {
                    kind: WorkflowNodeKind::Table,
                    executable: true,
                    detail: "Reads an existing qualified Spiral relation.".to_owned(),
                },
                WorkflowNodeCapability {
                    kind: WorkflowNodeKind::Model,
                    executable: true,
                    detail: "Scores an existing Spiral image relation with pinned SigLIP artifacts."
                        .to_owned(),
                },
                WorkflowNodeCapability {
                    kind: WorkflowNodeKind::Query,
                    executable: true,
                    detail: "Runs bounded read-only Spiral SQL.".to_owned(),
                },
                WorkflowNodeCapability {
                    kind: WorkflowNodeKind::Result,
                    executable: true,
                    detail: "Displays the terminal query payload.".to_owned(),
                },
            ],
            max_query_rows: MAX_WORKFLOW_QUERY_ROWS,
            relation_discovery: "Spiral ADBC catalog entries with Arrow schemas; row counts are omitted because catalog metadata does not expose a bounded count.".to_owned(),
            models: vec![WorkflowModelCapability::siglip_base()],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowNodeCapability {
    pub kind: WorkflowNodeKind,
    pub executable: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowModelCapability {
    pub id: String,
    pub label: String,
    pub repository: String,
    pub artifact_revision: String,
    pub input_contract: String,
    pub max_features: usize,
    pub device_policy: String,
}

impl WorkflowModelCapability {
    pub fn siglip_base() -> Self {
        Self {
            id: SIGLIP_BASE_MODEL_ID.to_owned(),
            label: "SigLIP base".to_owned(),
            repository: SIGLIP_BASE_REPOSITORY.to_owned(),
            artifact_revision: SIGLIP_BASE_REVISION.to_owned(),
            input_contract: "One upstream Query that returns the stable frame identity fields plus each decoded spql.image.Image field named by a typed feature definition. The Model registers compatible definitions and backfills nullable cells in the source-scoped Studio feature cache."
                .to_owned(),
            max_features: MAX_SIGLIP_FEATURES,
            device_policy: "Metal when available; CPU fallback".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowRelationsResponse {
    /// Must match [`WorkflowStateResponse::scope_id`] before the catalog may
    /// be projected into the editor.
    #[serde(default)]
    pub scope_id: String,
    pub relations: Vec<WorkflowRelation>,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limitation: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowRelation {
    pub name: String,
    pub relation_type: String,
    pub columns: Vec<QueryColumn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_count: Option<u64>,
}

/// Reserved bounded diagnostic shape for later decode/model phases.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowQuarantineSummary {
    pub count: u64,
    #[serde(default)]
    pub examples: Vec<String>,
}

/// One document persisted atomically under the workflow identity.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkflowDocument {
    pub workflow: WorkflowSpec,
    pub snapshot: WorkflowRunSnapshot,
}

impl Default for WorkflowDocument {
    fn default() -> Self {
        let workflow = WorkflowSpec::default();
        let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        Self { workflow, snapshot }
    }
}

impl WorkflowSpec {
    pub fn validate(&self) -> WorkflowValidationReport {
        let spec = self;
        let mut report = WorkflowValidationReport::default();
        if WorkflowNodeId::try_from(spec.id.as_str()).is_err() {
            report.errors.push(WorkflowValidationIssue::new("invalid_workflow_id", "Workflow ID must start with a letter or '_' and contain only letters, numbers, '_' or '-'.", "Use a stable identifier such as relation-query."));
        }
        if spec.name.trim().is_empty() {
            report.errors.push(WorkflowValidationIssue::new(
                "missing_workflow_name",
                "Workflow name cannot be empty.",
                "Name the saved workflow before applying it.",
            ));
        }

        let mut nodes = BTreeMap::new();
        for node in &spec.nodes {
            if WorkflowNodeId::try_from(node.id.as_str()).is_err() {
                report.errors.push(
                    WorkflowValidationIssue::new(
                        "invalid_node_id",
                        "Node IDs may contain letters, numbers, '_' and '-' only.",
                        "Choose a stable node ID without SQL punctuation.",
                    )
                    .with_node_id(&node.id),
                );
            }
            if nodes.insert(node.id.as_str(), node).is_some() {
                report.errors.push(
                    WorkflowValidationIssue::new(
                        "duplicate_node_id",
                        "Node IDs must be unique within a workflow.",
                        "Rename one of the duplicate nodes.",
                    )
                    .with_node_id(&node.id),
                );
            }
            match &node.config {
                WorkflowNodeConfig::Table { relation } => {
                    if relation.is_empty() {
                        report.errors.push(WorkflowValidationIssue::new("table_relation_required", "Choose an existing Spiral relation for this Table node.", "Select a qualified relation discovered from the active Spiral project.").with_node_id(&node.id));
                    } else if let Err(error) = QualifiedRelation::try_from(relation.as_str()) {
                        report.errors.push(
                            WorkflowValidationIssue::new(
                                "invalid_relation",
                                &error.to_string(),
                                "Choose a relation returned by Workflow relation discovery.",
                            )
                            .with_node_id(&node.id),
                        );
                    }
                }
                WorkflowNodeConfig::Model {
                    model_id,
                    features,
                    feature_cache_relation,
                } => {
                    if model_id != SIGLIP_BASE_MODEL_ID {
                        report.errors.push(
                            WorkflowValidationIssue::new(
                                "unsupported_model",
                                "This Studio build supports only the siglip-base Model capability.",
                                "Choose SigLIP base from the advertised Workflow capabilities.",
                            )
                            .with_node_id(&node.id),
                        );
                    }
                    if features.is_empty() {
                        report.errors.push(
                            WorkflowValidationIssue::new(
                                "model_features_required",
                                "SigLIP needs at least one typed feature definition.",
                                "Add a descriptive feature such as is_gripper_open_feature.",
                            )
                            .with_node_id(&node.id),
                        );
                    }
                    if features.len() > MAX_SIGLIP_FEATURES {
                        report.errors.push(WorkflowValidationIssue::new("too_many_model_features", &format!(
                            "SigLIP supports at most {MAX_SIGLIP_FEATURES} features per Model node."
                        ), "Remove features or split the work into sequential Model nodes.").with_node_id(&node.id));
                    }
                    let mut feature_names = BTreeSet::new();
                    for (feature_index, feature) in features.iter().enumerate() {
                        let untouched = feature.name.trim().is_empty()
                            && feature.description.trim().is_empty()
                            && feature.concept.trim().is_empty();
                        if untouched {
                            report.errors.push(
                                WorkflowValidationIssue::new(
                                    "incomplete_feature_definition",
                                    &format!("Complete Feature {}.", feature_index + 1),
                                    "Enter a feature name, description, and model concept.",
                                )
                                .with_node_id(&node.id),
                            );
                        } else {
                            if !feature.has_valid_name() {
                                report.errors.push(WorkflowValidationIssue::new("invalid_feature_name", "Feature names must be safe descriptive identifiers ending in _feature.", "Use a name such as is_raining_feature or is_gripper_open_feature.").with_node_id(&node.id));
                            } else if !feature_names.insert(feature.name.as_str()) {
                                report.errors.push(
                                    WorkflowValidationIssue::new(
                                        "duplicate_feature_name",
                                        "Feature names must be unique within one Model node.",
                                        "Remove the duplicate feature definition.",
                                    )
                                    .with_node_id(&node.id),
                                );
                            }
                            if feature.description.trim().is_empty()
                                || feature.description.chars().count()
                                    > MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH
                            {
                                report.errors.push(WorkflowValidationIssue::new("invalid_feature_description", "Feature descriptions must be present and bounded.", "Describe what a non-null probability means in 240 characters or fewer.").with_node_id(&node.id));
                            }
                            if feature.concept.trim().is_empty()
                                || feature.concept.chars().count() > MAX_SIGLIP_CONCEPT_LENGTH
                            {
                                report.errors.push(
                                    WorkflowValidationIssue::new(
                                        "invalid_feature_concept",
                                        "Feature model text must be present and bounded.",
                                        "Use a short visual phrase of 80 characters or fewer.",
                                    )
                                    .with_node_id(&node.id),
                                );
                            }
                        }
                        if QualifiedRelation::try_from(
                            feature.applicability.source_relation.as_str(),
                        )
                        .is_err()
                        {
                            report.errors.push(WorkflowValidationIssue::new("invalid_feature_source", "Feature applicability needs one authoritative qualified source relation.", "Choose the upstream source Table relation.").with_node_id(&node.id));
                        }
                        if UnquotedSqlIdentifier::try_from(
                            feature.applicability.media_field.as_str(),
                        )
                        .is_err()
                        {
                            report.errors.push(WorkflowValidationIssue::new("invalid_feature_media_field", "Feature applicability media_field must be one safe candidate image field.", "Project a decoded image with a stable field name such as image.").with_node_id(&node.id));
                        }
                        let streams = &feature.applicability.stream_ids;
                        if streams.len() > MAX_WORKFLOW_FEATURE_STREAMS
                            || streams
                                .iter()
                                .any(|stream| stream.trim().is_empty() || stream.len() > 128)
                            || streams.iter().collect::<BTreeSet<_>>().len() != streams.len()
                        {
                            report.errors.push(WorkflowValidationIssue::new("invalid_feature_streams", "Feature applicability supports up to eight unique non-empty stream IDs.", "Remove duplicate or unsupported stream filters.").with_node_id(&node.id));
                        }
                    }
                    if feature_cache_relation.as_ref().is_some_and(|relation| {
                        QualifiedRelation::try_from(relation.as_str()).is_err()
                    }) {
                        report.errors.push(WorkflowValidationIssue::new("invalid_model_output", "The server-derived Model output relation is invalid.", "Apply the workflow again so the server can derive the relation name.").with_node_id(&node.id));
                    }
                }
                WorkflowNodeConfig::Query { sql, max_rows } => {
                    if let Err(error) =
                        ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Training).authorize()
                    {
                        report.errors.push(
                            WorkflowValidationIssue::new(
                                "query_not_read_only",
                                &error.to_string().replace("Training SQL", "Workflow SQL"),
                                "Use one read-only SELECT or WITH query.",
                            )
                            .with_node_id(&node.id),
                        );
                    }
                    if !(1..=MAX_WORKFLOW_QUERY_ROWS).contains(max_rows) {
                        report.errors.push(WorkflowValidationIssue::new("query_row_limit", &format!(
                            "Workflow query row limit must be between 1 and {MAX_WORKFLOW_QUERY_ROWS}."
                        ), "Choose a bounded row limit.").with_node_id(&node.id));
                    }
                    if let Err(error) = WorkflowSql::from(sql.as_str()).node_placeholders() {
                        report.errors.push(
                            WorkflowValidationIssue::new(
                                "malformed_placeholder",
                                &error.to_string(),
                                "Use exact placeholders such as {{node:table}}.",
                            )
                            .with_node_id(&node.id),
                        );
                    }
                }
                WorkflowNodeConfig::Result => {}
            }
        }

        if spec.nodes.is_empty() {
            report.errors.push(WorkflowValidationIssue::new(
                "empty_workflow",
                "A workflow needs Table, Query, and Result nodes.",
                "Add at least one Table root and one terminal Result.",
            ));
            return report;
        }

        let mut outgoing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut incoming: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut unique_edges = BTreeSet::new();
        for edge in &spec.edges {
            if !unique_edges.insert((edge.source.as_str(), edge.target.as_str())) {
                report.errors.push(
                    WorkflowValidationIssue::new(
                        "duplicate_edge",
                        "Workflow edges must be unique.",
                        "Remove the repeated connection.",
                    )
                    .with_node_id(&edge.target),
                );
            }
            let Some(source) = nodes.get(edge.source.as_str()) else {
                report.errors.push(
                    WorkflowValidationIssue::new(
                        "unknown_edge_source",
                        "A workflow edge references an unknown source node.",
                        "Reconnect the edge to an existing node.",
                    )
                    .with_node_id(&edge.source),
                );
                continue;
            };
            let Some(target) = nodes.get(edge.target.as_str()) else {
                report.errors.push(
                    WorkflowValidationIssue::new(
                        "unknown_edge_target",
                        "A workflow edge references an unknown target node.",
                        "Reconnect the edge to an existing node.",
                    )
                    .with_node_id(&edge.target),
                );
                continue;
            };
            if !source
                .config
                .kind()
                .allows_transition_to(target.config.kind())
            {
                report.errors.push(
                    WorkflowValidationIssue::new(
                        "invalid_transition",
                        &format!(
                            "{} cannot feed {} in a relation-first workflow.",
                            source.config.kind().label(),
                            target.config.kind().label(),
                        ),
                        "Use Table→Query, Query→Model, Query→Query, Model→Query, or Query→Result.",
                    )
                    .with_node_id(&edge.target),
                );
            }
            outgoing
                .entry(edge.source.as_str())
                .or_default()
                .push(edge.target.as_str());
            incoming
                .entry(edge.target.as_str())
                .or_default()
                .push(edge.source.as_str());
        }

        let table_roots = spec
            .nodes
            .iter()
            .filter(|node| {
                matches!(node.config, WorkflowNodeConfig::Table { .. })
                    && incoming.get(node.id.as_str()).is_none_or(Vec::is_empty)
            })
            .count();
        if table_roots == 0 {
            report.errors.push(WorkflowValidationIssue::new(
                "missing_table_root",
                "A workflow needs at least one Table root.",
                "Choose an existing Spiral relation as the first node.",
            ));
        }

        let result_count = spec
            .nodes
            .iter()
            .filter(|node| matches!(node.config, WorkflowNodeConfig::Result))
            .count();
        if result_count == 0 {
            report.errors.push(WorkflowValidationIssue::new(
                "missing_result",
                "A Workflow needs exactly one terminal Result node.",
                "Connect the final Query to one Result node.",
            ));
        } else if result_count > 1 {
            report.errors.push(WorkflowValidationIssue::new(
                "multiple_results",
                "A Workflow supports exactly one terminal Result node.",
                "Keep one Result node so the run owns one unambiguous payload.",
            ));
        }

        for node in &spec.nodes {
            let in_degree = incoming.get(node.id.as_str()).map_or(0, Vec::len);
            let out_degree = outgoing.get(node.id.as_str()).map_or(0, Vec::len);
            if out_degree == 0 && !matches!(node.config, WorkflowNodeConfig::Result) {
                report.errors.push(
                    WorkflowValidationIssue::new(
                        "orphan_terminal",
                        "Every non-Result node must contribute to the terminal Result.",
                        "Connect this node downstream or remove the orphan branch.",
                    )
                    .with_node_id(&node.id),
                );
            }
            match &node.config {
            WorkflowNodeConfig::Table { .. } if in_degree != 0 => report.errors.push(WorkflowValidationIssue::new("table_not_root", "Table nodes must be roots.", "Remove incoming edges from this Table node.").with_node_id(&node.id)),
            WorkflowNodeConfig::Result if out_degree != 0 => report.errors.push(WorkflowValidationIssue::new("result_not_terminal", "Result nodes must be terminal.", "Remove outgoing edges from this Result node.").with_node_id(&node.id)),
            WorkflowNodeConfig::Result if in_degree != 1 => report.errors.push(WorkflowValidationIssue::new("result_input_count", "A Result node must have exactly one Query input.", "Connect one Query node to this Result.").with_node_id(&node.id)),
            WorkflowNodeConfig::Model { .. } if in_degree != 1 => report.errors.push(WorkflowValidationIssue::new("model_input_count", "A SigLIP Model node needs exactly one upstream candidate Query.", "Connect one read-only Query that returns stable frame identity and a decoded image field.").with_node_id(&node.id)),
            WorkflowNodeConfig::Model { .. }
                if incoming
                    .get(node.id.as_str())
                    .and_then(|sources| sources.first())
                    .and_then(|source| nodes.get(*source))
                    .is_some_and(|source| source.config.kind() != WorkflowNodeKind::Query) =>
            {
                report.errors.push(WorkflowValidationIssue::new("model_input_kind", "A SigLIP Model enriches the rows selected by an upstream candidate Query.", "Insert a Query between the source Table and this Model, and project the stable frame keys plus one decoded image field.").with_node_id(&node.id));
            }
            WorkflowNodeConfig::Query { sql, .. } => {
                if let Ok(placeholders) = WorkflowSql::from(sql.as_str()).node_placeholders() {
                    let upstream = incoming
                        .get(node.id.as_str())
                        .into_iter()
                        .flatten()
                        .copied()
                        .collect::<BTreeSet<_>>();
                    if placeholders.is_empty() {
                        report.errors.push(WorkflowValidationIssue::new("query_placeholder_required", "Workflow SQL must reference an upstream node placeholder.", "Use a placeholder such as {{node:table}} in FROM or JOIN.").with_node_id(&node.id));
                    }
                    for placeholder in &placeholders {
                        if !nodes.contains_key(placeholder.as_str()) {
                            report.errors.push(WorkflowValidationIssue::new("unknown_placeholder", &format!("Workflow SQL references unknown node {placeholder:?}."), "Reference an existing connected upstream Table, Query, or Model node.").with_node_id(&node.id));
                        } else if !upstream.contains(placeholder.as_str()) {
                            report.errors.push(WorkflowValidationIssue::new("non_upstream_placeholder", &format!(
                                    "Workflow SQL references {placeholder:?}, which is not connected upstream."
                                ), "Connect that node to this Query or remove the placeholder.").with_node_id(&node.id));
                        }
                    }
                    for upstream_id in upstream {
                        if !placeholders.iter().any(|id| id == upstream_id) {
                            report.errors.push(WorkflowValidationIssue::new("unused_upstream_edge", &format!(
                                    "Upstream node {upstream_id:?} is connected but not referenced by Workflow SQL."
                                ), "Use its {{node:<id>}} placeholder or remove the edge.").with_node_id(&node.id));
                        }
                    }
                }
            }
            _ => {}
        }
            if let WorkflowNodeConfig::Model {
                features,
                feature_cache_relation,
                ..
            } = &node.config
            {
                let mut table_sources = BTreeSet::new();
                let mut visited = BTreeSet::new();
                let mut pending = incoming.get(node.id.as_str()).cloned().unwrap_or_default();
                while let Some(upstream_id) = pending.pop() {
                    if !visited.insert(upstream_id) {
                        continue;
                    }
                    let Some(upstream) = nodes.get(upstream_id) else {
                        continue;
                    };
                    match &upstream.config {
                        WorkflowNodeConfig::Table { relation } => {
                            table_sources.insert(relation.as_str());
                        }
                        _ => {
                            pending.extend(incoming.get(upstream_id).into_iter().flatten().copied())
                        }
                    }
                }
                if table_sources.len() != 1 {
                    report.errors.push(WorkflowValidationIssue::new("model_source_count", "A Model feature cache must belong to exactly one authoritative source relation.", "Build the candidate Query from one source Table; join other evidence only after the Model.").with_node_id(&node.id));
                } else if let Some(source) = table_sources.first() {
                    if feature_cache_relation
                        .as_ref()
                        .is_some_and(|output| output != &derived_feature_cache_relation(source))
                    {
                        report.errors.push(WorkflowValidationIssue::new("invalid_model_output", "The Model feature-cache relation is not derived from its source relation.", "Apply the workflow again so the server can derive the source-scoped cache.").with_node_id(&node.id));
                    }
                    if features
                        .iter()
                        .any(|feature| feature.applicability.source_relation != **source)
                    {
                        report.errors.push(WorkflowValidationIssue::new("feature_source_mismatch", "Feature applicability does not match the Model's authoritative source Table.", "Apply the workflow again so the server can normalize every feature source.").with_node_id(&node.id));
                    }
                }
            }
        }

        if self.topological_order().is_err() {
            report.errors.push(WorkflowValidationIssue::new(
                "workflow_cycle",
                "Workflow lineage must be acyclic.",
                "Remove the connection that feeds a downstream node back upstream.",
            ));
        }
        report
    }

    pub fn topological_order(&self) -> Result<Vec<String>, String> {
        let spec = self;
        let mut in_degree = spec
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), 0_usize))
            .collect::<BTreeMap<_, _>>();
        let mut outgoing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in &spec.edges {
            if !in_degree.contains_key(edge.source.as_str()) {
                return Err(format!("unknown source {:?}", edge.source));
            }
            let Some(degree) = in_degree.get_mut(edge.target.as_str()) else {
                return Err(format!("unknown target {:?}", edge.target));
            };
            *degree += 1;
            outgoing
                .entry(edge.source.as_str())
                .or_default()
                .push(edge.target.as_str());
        }
        let mut ready = in_degree
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
            .collect::<VecDeque<_>>();
        let mut ordered = Vec::with_capacity(spec.nodes.len());
        while let Some(id) = ready.pop_front() {
            ordered.push(id.to_owned());
            for target in outgoing.get(id).into_iter().flatten() {
                let degree = in_degree
                    .get_mut(target)
                    .expect("known edges have known targets");
                *degree -= 1;
                if *degree == 0 {
                    ready.push_back(target);
                }
            }
        }
        if ordered.len() == spec.nodes.len() {
            Ok(ordered)
        } else {
            Err("workflow contains a cycle".to_owned())
        }
    }

    pub fn has_model(&self) -> bool {
        self.nodes
            .iter()
            .any(|node| matches!(node.config, WorkflowNodeConfig::Model { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature(source: &str, name: &str, concept: &str) -> WorkflowFeatureDefinition {
        WorkflowFeatureDefinition {
            name: name.to_owned(),
            description: format!("Probability that the frame shows {concept}."),
            concept: concept.to_owned(),
            applicability: WorkflowFeatureApplicability {
                source_relation: source.to_owned(),
                media_field: "image".to_owned(),
                stream_ids: Vec::new(),
            },
        }
    }

    fn runnable_spec() -> WorkflowSpec {
        WorkflowSpec {
            nodes: vec![
                WorkflowNodeSpec {
                    id: "frames".to_owned(),
                    name: "Frames".to_owned(),
                    config: WorkflowNodeConfig::Table {
                        relation: "pick_place.camera_frames".to_owned(),
                    },
                },
                WorkflowNodeSpec {
                    id: "query".to_owned(),
                    name: "Read frames".to_owned(),
                    config: WorkflowNodeConfig::Query {
                        sql: "SELECT * FROM {{node:frames}} LIMIT 10".to_owned(),
                        max_rows: 10,
                    },
                },
                WorkflowNodeSpec {
                    id: "result".to_owned(),
                    name: "Results".to_owned(),
                    config: WorkflowNodeConfig::Result,
                },
            ],
            edges: vec![
                WorkflowEdgeSpec {
                    source: "frames".to_owned(),
                    target: "query".to_owned(),
                },
                WorkflowEdgeSpec {
                    source: "query".to_owned(),
                    target: "result".to_owned(),
                },
            ],
            ..WorkflowSpec::default()
        }
    }

    #[test]
    fn serde_shape_is_tagged_and_stable() {
        let json = serde_json::to_value(runnable_spec()).unwrap();
        assert_eq!(json["nodes"][0]["kind"], "table");
        assert_eq!(json["nodes"][1]["kind"], "query");
        assert_eq!(json["nodes"][2]["kind"], "result");
        let decoded: WorkflowSpec = serde_json::from_value(json).unwrap();
        assert!(decoded.validate().valid());
    }

    #[test]
    fn api_state_and_sse_update_round_trip_as_one_authoritative_shape() {
        let workflow = runnable_spec();
        let state = WorkflowStateResponse {
            scope_id: "workflow-scope-1".to_owned(),
            validation: workflow.validate(),
            snapshot: WorkflowRunSnapshot::idle_for(&workflow),
            workflow,
        };
        let json = serde_json::to_value(WorkflowUpdate {
            state: state.clone(),
        })
        .unwrap();
        assert_eq!(json["state"]["workflow"]["id"], DEFAULT_WORKFLOW_ID);
        assert_eq!(json["state"]["scope_id"], "workflow-scope-1");
        assert_eq!(json["state"]["snapshot"]["phase"], "idle");
        assert!(
            json["state"]["validation"]["errors"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let decoded: WorkflowUpdate = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.state, state);
    }

    #[test]
    fn agent_state_and_named_update_are_serde_stable_and_secret_free() {
        let mut state = WorkflowAgentStateResponse {
            readiness: WorkflowAgentReadiness {
                configured: true,
                status: WorkflowAgentReadinessStatus::Ready,
                provider: "provider.example".to_owned(),
                model: "model-a".to_owned(),
                detail: "Ready".to_owned(),
            },
            scope_id: "agent-scope-1".to_owned(),
            history_available: true,
            revision: 3,
            active_conversation_id: "conversation-1".to_owned(),
            conversation_limit: MAX_WORKFLOW_AGENT_CONVERSATIONS,
            ..WorkflowAgentStateResponse::default()
        };
        state.conversations.push(WorkflowAgentConversationSummary {
            conversation_id: "conversation-1".to_owned(),
            title: "Inspect the catalog".to_owned(),
            created_ms: 9,
            updated_ms: 10,
            message_count: 1,
            owned_run_id: None,
        });
        state.session.conversation_id = "conversation-1".to_owned();
        state.session.messages.push(WorkflowAgentMessage {
            id: "message-1".to_owned(),
            sequence: 1,
            role: WorkflowAgentRole::Assistant,
            phase: WorkflowAgentMessagePhase::Complete,
            text: "I inspected the catalog.".to_owned(),
            tools: vec![WorkflowAgentToolSummary {
                call_id: "call-1".to_owned(),
                name: "list_relations".to_owned(),
                phase: WorkflowAgentToolPhase::Complete,
                summary: "Relation catalog complete".to_owned(),
                error: None,
                receipt: None,
            }],
            run_id: None,
            created_ms: 10,
        });
        state.session.revision = 2;
        let json = serde_json::to_value(WorkflowAgentUpdate {
            state: state.clone(),
        })
        .unwrap();
        assert_eq!(json["state"]["session"]["revision"], 2);
        assert_eq!(json["state"]["readiness"]["status"], "ready");
        assert_eq!(json["state"]["revision"], 3);
        assert_eq!(json["state"]["active_conversation_id"], "conversation-1");
        assert_eq!(
            json["state"]["conversations"][0]["title"],
            "Inspect the catalog"
        );
        let encoded = json.to_string();
        assert!(!encoded.contains("api_key"));
        assert!(!encoded.contains("client_secret"));
        let decoded: WorkflowAgentUpdate = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.state, state);
    }

    #[test]
    fn agent_conversation_commands_are_explicitly_scoped_and_secret_free() {
        let create = WorkflowAgentCreateConversationRequest {
            scope_id: "scope-1".to_owned(),
            expected_revision: 4,
        };
        let activate = WorkflowAgentActivateConversationRequest {
            scope_id: "scope-1".to_owned(),
            conversation_id: "conversation-2".to_owned(),
            expected_revision: 4,
        };
        let cancel = WorkflowAgentCancelRequest {
            scope_id: "scope-1".to_owned(),
            conversation_id: "conversation-2".to_owned(),
            turn_id: "turn-8".to_owned(),
        };
        for encoded in [
            serde_json::to_string(&create).unwrap(),
            serde_json::to_string(&activate).unwrap(),
            serde_json::to_string(&cancel).unwrap(),
        ] {
            assert!(!encoded.contains("api_key"));
            assert!(!encoded.contains("provider_messages"));
            assert!(encoded.contains("scope-1"));
        }
    }

    #[test]
    fn feature_cache_relation_is_server_owned_and_stable() {
        assert_eq!(
            derived_feature_cache_relation("pick/place"),
            format!(
                "workflow.features_{}",
                spiral_robotics_core::identity::SqlNameComponent::from("pick/place").as_str()
            )
        );
    }

    #[test]
    fn agent_phase_reports_only_real_active_work() {
        assert!(WorkflowAgentPhase::Responding.active());
        assert!(WorkflowAgentPhase::WaitingForRun.active());
        assert!(WorkflowAgentPhase::Cancelling.active());
        assert!(!WorkflowAgentPhase::Idle.active());
        assert!(!WorkflowAgentPhase::Failed.active());
    }

    #[test]
    fn runnable_table_query_result_is_valid() {
        let spec = runnable_spec();
        assert!(spec.validate().valid());
        assert_eq!(
            spec.topological_order().unwrap(),
            ["frames", "query", "result"]
        );
    }

    #[test]
    fn graph_validation_rejects_duplicates_cycles_and_nonterminal_results() {
        let mut duplicate = runnable_spec();
        duplicate.nodes.push(duplicate.nodes[0].clone());
        assert!(
            duplicate
                .validate()
                .errors
                .iter()
                .any(|issue| issue.code == "duplicate_node_id")
        );

        let mut cycle = runnable_spec();
        cycle.edges.push(WorkflowEdgeSpec {
            source: "result".to_owned(),
            target: "query".to_owned(),
        });
        let report = cycle.validate();
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.code == "result_not_terminal")
        );
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.code == "workflow_cycle")
        );
    }

    #[test]
    fn placeholders_must_be_known_connected_and_exact() {
        let mut unknown = runnable_spec();
        let WorkflowNodeConfig::Query { sql, .. } = &mut unknown.nodes[1].config else {
            unreachable!()
        };
        *sql = "SELECT * FROM {{node:missing}}".to_owned();
        assert!(
            unknown
                .validate()
                .errors
                .iter()
                .any(|issue| issue.code == "unknown_placeholder")
        );

        let mut non_upstream = runnable_spec();
        non_upstream.nodes.insert(
            1,
            WorkflowNodeSpec {
                id: "other".to_owned(),
                name: "Other".to_owned(),
                config: WorkflowNodeConfig::Table {
                    relation: "pick_place.joint_state".to_owned(),
                },
            },
        );
        let WorkflowNodeConfig::Query { sql, .. } = &mut non_upstream.nodes[2].config else {
            unreachable!()
        };
        *sql = "SELECT * FROM {{node:other}}".to_owned();
        assert!(
            non_upstream
                .validate()
                .errors
                .iter()
                .any(|issue| issue.code == "non_upstream_placeholder")
        );

        let mut malformed = runnable_spec();
        let WorkflowNodeConfig::Query { sql, .. } = &mut malformed.nodes[1].config else {
            unreachable!()
        };
        *sql = "SELECT * FROM {{node:frames}".to_owned();
        assert!(
            malformed
                .validate()
                .errors
                .iter()
                .any(|issue| issue.code == "malformed_placeholder")
        );
    }

    #[test]
    fn repeated_placeholder_is_valid_and_siglip_model_is_executable() {
        let mut repeated = runnable_spec();
        let WorkflowNodeConfig::Query { sql, .. } = &mut repeated.nodes[1].config else {
            unreachable!()
        };
        *sql = "SELECT * FROM {{node:frames}} a JOIN {{node:frames}} b ON a.frame_index = b.frame_index".to_owned();
        assert!(repeated.validate().valid());

        let mut model = runnable_spec();
        model.nodes.insert(
            2,
            WorkflowNodeSpec {
                id: "model".to_owned(),
                name: "Reserved model".to_owned(),
                config: WorkflowNodeConfig::Model {
                    model_id: SIGLIP_BASE_MODEL_ID.to_owned(),
                    features: vec![
                        feature(
                            "pick_place.camera_frames",
                            "is_red_cube_feature",
                            "red cube",
                        ),
                        feature("pick_place.camera_frames", "is_gripper_feature", "gripper"),
                    ],
                    feature_cache_relation: Some(derived_feature_cache_relation(
                        "pick_place.camera_frames",
                    )),
                },
            },
        );
        model.nodes.insert(
            3,
            WorkflowNodeSpec {
                id: "final_query".to_owned(),
                name: "Join features".to_owned(),
                config: WorkflowNodeConfig::Query {
                    sql: "SELECT candidates.*, features.is_red_cube_feature, features.is_gripper_feature FROM {{node:query}} candidates JOIN {{node:model}} features ON candidates.episode_id = features.episode_id AND candidates.stream_id = features.stream_id AND candidates.segment_index = features.segment_index AND candidates.frame_index = features.frame_index AND candidates.episode_time_ns = features.episode_time_ns".to_owned(),
                    max_rows: 100,
                },
            },
        );
        model.edges.insert(
            1,
            WorkflowEdgeSpec {
                source: "query".to_owned(),
                target: "model".to_owned(),
            },
        );
        model.edges.insert(
            2,
            WorkflowEdgeSpec {
                source: "query".to_owned(),
                target: "final_query".to_owned(),
            },
        );
        model.edges.insert(
            3,
            WorkflowEdgeSpec {
                source: "model".to_owned(),
                target: "final_query".to_owned(),
            },
        );
        model.edges[4].source = "final_query".to_owned();
        assert!(model.has_model());
        assert!(model.validate().valid());
    }

    #[test]
    fn sql_candidate_model_cache_and_joint_table_converge_in_final_query() {
        let spec = WorkflowSpec {
            id: "demo".to_owned(),
            name: "Holding evidence".to_owned(),
            revision: 0,
            nodes: vec![
                WorkflowNodeSpec {
                    id: "manifest".to_owned(),
                    name: "Camera manifest".to_owned(),
                    config: WorkflowNodeConfig::Table {
                        relation: "pick_place.video_manifest".to_owned(),
                    },
                },
                WorkflowNodeSpec {
                    id: "candidates".to_owned(),
                    name: "Periodic candidates".to_owned(),
                    config: WorkflowNodeConfig::Query {
                        sql: "SELECT episode_id, stream_id, segment_index, frame_index, episode_time_ns, image FROM {{node:manifest}}".to_owned(),
                        max_rows: 100,
                    },
                },
                WorkflowNodeSpec {
                    id: "holding_features".to_owned(),
                    name: "Backfill holding feature".to_owned(),
                    config: WorkflowNodeConfig::Model {
                        model_id: SIGLIP_BASE_MODEL_ID.to_owned(),
                        features: vec![feature(
                            "pick_place.video_manifest",
                            "is_holding_cube_feature",
                            "holding cube",
                        )],
                        feature_cache_relation: Some(derived_feature_cache_relation("pick_place.video_manifest")),
                    },
                },
                WorkflowNodeSpec {
                    id: "joints".to_owned(),
                    name: "Joint evidence".to_owned(),
                    config: WorkflowNodeConfig::Table {
                        relation: "pick_place.joint_samples".to_owned(),
                    },
                },
                WorkflowNodeSpec {
                    id: "query".to_owned(),
                    name: "Join evidence".to_owned(),
                    config: WorkflowNodeConfig::Query {
                        sql: "SELECT candidates.*, features.is_holding_cube_feature, joints.positions FROM {{node:candidates}} candidates JOIN {{node:holding_features}} features ON candidates.episode_id = features.episode_id AND candidates.frame_index = features.frame_index JOIN {{node:joints}} joints ON candidates.episode_id = joints.episode_id".to_owned(),
                        max_rows: 100,
                    },
                },
                WorkflowNodeSpec {
                    id: "result".to_owned(),
                    name: "Matching moments".to_owned(),
                    config: WorkflowNodeConfig::Result,
                },
            ],
            edges: vec![
                WorkflowEdgeSpec { source: "manifest".to_owned(), target: "candidates".to_owned() },
                WorkflowEdgeSpec { source: "candidates".to_owned(), target: "holding_features".to_owned() },
                WorkflowEdgeSpec { source: "candidates".to_owned(), target: "query".to_owned() },
                WorkflowEdgeSpec { source: "holding_features".to_owned(), target: "query".to_owned() },
                WorkflowEdgeSpec { source: "joints".to_owned(), target: "query".to_owned() },
                WorkflowEdgeSpec { source: "query".to_owned(), target: "result".to_owned() },
            ],
        };

        assert!(spec.validate().valid());

        let mut direct_table_model = spec.clone();
        direct_table_model.edges[1].source = "manifest".to_owned();
        assert!(
            direct_table_model
                .validate()
                .errors
                .iter()
                .any(|issue| issue.code == "model_input_kind")
        );
    }

    #[test]
    fn typed_feature_names_are_descriptive_and_safe() {
        let source_relation = "weather.camera_frames";
        assert!(feature(source_relation, "is_raining_feature", "rain").has_valid_name());
        assert!(
            feature(source_relation, "is_gripper_open_feature", "open gripper").has_valid_name()
        );
        assert!(!feature(source_relation, "score_1", "rain").has_valid_name());
        assert!(!feature(source_relation, "is-raining-feature", "rain").has_valid_name());
    }

    #[test]
    fn model_wire_shape_requires_typed_feature_definitions() {
        let model: WorkflowNodeConfig = serde_json::from_value(serde_json::json!({
            "kind": "model",
            "model_id": "siglip-base",
            "features": [{
                "name": "is_raining_feature",
                "description": "Probability that rain is visible.",
                "concept": "raining",
                "applicability": {
                    "source_relation": "weather.camera_frames",
                    "media_field": "image",
                    "stream_ids": ["camera.front"]
                }
            }],
            "feature_cache_relation": "workflow.features_weather"
        }))
        .unwrap();
        let WorkflowNodeConfig::Model { features, .. } = model else {
            unreachable!()
        };
        assert_eq!(features[0].name, "is_raining_feature");
        assert_eq!(features[0].applicability.stream_ids, ["camera.front"]);
    }

    #[test]
    fn capabilities_pin_the_executable_siglip_artifact() {
        let capabilities = WorkflowCapabilities::default();
        assert_eq!(capabilities.schema_version, 3);
        assert_eq!(
            capabilities.models,
            [WorkflowModelCapability::siglip_base()]
        );
        assert!(
            capabilities
                .node_kinds
                .iter()
                .any(|capability| capability.kind == WorkflowNodeKind::Model
                    && capability.executable)
        );
    }

    #[test]
    fn mutating_sql_and_relation_injection_are_rejected() {
        let mut spec = runnable_spec();
        let WorkflowNodeConfig::Table { relation } = &mut spec.nodes[0].config else {
            unreachable!()
        };
        *relation = "pick_place.frames;DROP".to_owned();
        let WorkflowNodeConfig::Query { sql, .. } = &mut spec.nodes[1].config else {
            unreachable!()
        };
        *sql = "DELETE FROM {{node:frames}}".to_owned();
        let report = spec.validate();
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.code == "invalid_relation")
        );
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.code == "query_not_read_only")
        );
    }

    #[test]
    fn exactly_one_result_owns_the_terminal_payload() {
        let mut missing = runnable_spec();
        missing.nodes.pop();
        missing.edges.pop();
        let report = missing.validate();
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.code == "missing_result")
        );

        let mut multiple = runnable_spec();
        multiple.nodes.push(WorkflowNodeSpec {
            id: "other_result".to_owned(),
            name: "Other result".to_owned(),
            config: WorkflowNodeConfig::Result,
        });
        multiple.edges.push(WorkflowEdgeSpec {
            source: "query".to_owned(),
            target: "other_result".to_owned(),
        });
        let report = multiple.validate();
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.code == "multiple_results")
        );
    }

    #[test]
    fn orphan_branches_cannot_make_execution_order_define_the_result() {
        let mut table_only = runnable_spec();
        table_only.nodes.truncate(1);
        table_only.edges.clear();
        let report = table_only.validate();
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.code == "missing_result")
        );
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.code == "orphan_terminal")
        );

        let mut orphan = runnable_spec();
        orphan.nodes.push(WorkflowNodeSpec {
            id: "orphan".to_owned(),
            name: "Orphan table".to_owned(),
            config: WorkflowNodeConfig::Table {
                relation: "pick_place.joint_state".to_owned(),
            },
        });
        let report = orphan.validate();
        assert!(report.errors.iter().any(|issue| {
            issue.code == "orphan_terminal" && issue.node_id.as_deref() == Some("orphan")
        }));
    }
}
