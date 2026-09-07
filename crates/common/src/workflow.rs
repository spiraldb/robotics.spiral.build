//! Shared Workflow command and observation contract.
//!
//! Workflow starts at relations already materialized in Spiral. External
//! ingestion remains outside Workflow; executable Model nodes consume those
//! relations and write server-derived relations for downstream Query nodes.
//! This crate derives only the stable unqualified component of generated
//! names; storage qualifies persisted relations inside Studio's configured
//! namespace.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::sql::workflow::{WorkflowNodeId, WorkflowSql};
use crate::sql::{QualifiedRelation, ReadOnlyQuery, ReadOnlyQueryMode, UnquotedSqlIdentifier};
use serde::{Deserialize, Serialize};

use crate::relations::SpiralQuerySnapshot;

pub const DEFAULT_WORKFLOW_ID: &str = "default";
const MAX_WORKFLOW_QUERY_ROWS: usize = 1_000;
const MAX_SIGLIP_FEATURES: usize = 32;
pub const MAX_SIGLIP_CONCEPT_LENGTH: usize = 80;
const MAX_WORKFLOW_FEATURE_NAME_LENGTH: usize = 96;
pub const MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH: usize = 240;
pub const MAX_WORKFLOW_FEATURE_STREAMS: usize = 8;

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowSpec {
    id: String,
    name: String,
    revision: u64,
    #[serde(default)]
    nodes: Vec<WorkflowNodeSpec>,
    #[serde(default)]
    edges: Vec<WorkflowEdgeSpec>,
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowNodeSpec {
    id: String,
    name: String,
    #[serde(flatten)]
    config: WorkflowNodeConfig,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowNodeConfig {
    Table {
        relation: String,
    },
    Model {
        features: Vec<WorkflowFeatureDefinition>,
    },
    Query {
        sql: String,
        #[serde(default = "WorkflowNodeConfig::default_query_rows")]
        max_rows: usize,
    },
    Result,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowFeatureDefinition {
    name: String,
    description: String,
    concept: String,
    applicability: WorkflowFeatureApplicability,
}

impl WorkflowFeatureDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        concept: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            concept: concept.into(),
            applicability: WorkflowFeatureApplicability {
                source_relation: String::new(),
                media_field: String::new(),
                stream_ids: Vec::new(),
            },
        }
    }

    pub fn with_applicability(mut self, applicability: WorkflowFeatureApplicability) -> Self {
        self.applicability = applicability;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn description(&self) -> &str {
        &self.description
    }
    pub fn concept(&self) -> &str {
        &self.concept
    }
    pub const fn applicability(&self) -> &WorkflowFeatureApplicability {
        &self.applicability
    }
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
    pub fn set_description(&mut self, description: impl Into<String>) {
        self.description = description.into();
    }
    pub fn set_concept(&mut self, concept: impl Into<String>) {
        self.concept = concept.into();
    }
    pub fn set_source_relation(&mut self, relation: impl Into<String>) {
        self.applicability.source_relation = relation.into();
    }
    pub fn set_media_field(&mut self, field: impl Into<String>) {
        self.applicability.media_field = field.into();
    }
    pub fn set_stream_ids(&mut self, stream_ids: Vec<String>) {
        self.applicability.stream_ids = stream_ids;
    }
    /// Whether this feature's output column has a safe, descriptive name.
    pub fn has_valid_name(&self) -> bool {
        UnquotedSqlIdentifier::try_from(self.name.as_str()).is_ok()
            && self.name.ends_with("_feature")
            && self.name.len() <= MAX_WORKFLOW_FEATURE_NAME_LENGTH
    }
}

impl WorkflowFeatureApplicability {
    /// Derive the unqualified server-owned feature cache name component.
    pub fn derived_cache_name(source_relation: &str) -> String {
        format!(
            "features_{}",
            crate::identity::SqlNameComponent::from(source_relation).as_str()
        )
    }

    /// Derive this applicability's server-owned feature cache name component.
    fn cache_name(&self) -> String {
        Self::derived_cache_name(&self.source_relation)
    }

    pub fn new(source_relation: impl Into<String>, media_field: impl Into<String>) -> Self {
        Self {
            source_relation: source_relation.into(),
            media_field: media_field.into(),
            stream_ids: Vec::new(),
        }
    }
    pub fn with_stream_ids(mut self, stream_ids: Vec<String>) -> Self {
        self.stream_ids = stream_ids;
        self
    }
    pub fn source_relation(&self) -> &str {
        &self.source_relation
    }
    pub fn media_field(&self) -> &str {
        &self.media_field
    }
    pub fn stream_ids(&self) -> &[String] {
        &self.stream_ids
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowFeatureApplicability {
    source_relation: String,
    media_field: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stream_ids: Vec<String>,
}

impl WorkflowNodeConfig {
    const fn default_query_rows() -> usize {
        100
    }

    const fn label(&self) -> &'static str {
        match self {
            Self::Table { .. } => "Table",
            Self::Model { .. } => "Model",
            Self::Query { .. } => "Query",
            Self::Result => "Result",
        }
    }

    const fn allows_transition_to(&self, target: &Self) -> bool {
        matches!(
            (self, target),
            (Self::Table { .. }, Self::Query { .. })
                | (Self::Query { .. }, Self::Model { .. })
                | (Self::Query { .. }, Self::Query { .. })
                | (Self::Model { .. }, Self::Query { .. })
                | (Self::Query { .. }, Self::Result)
        )
    }

    /// The writable name component is a pure server-owned derivation, never
    /// graph input. Storage adds the configured namespace before execution.
    pub fn model_output_name(&self) -> Option<String> {
        let Self::Model { features } = self else {
            return None;
        };
        features
            .first()
            .map(|feature| feature.applicability().cache_name())
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowEdgeSpec {
    source: String,
    target: String,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowValidationReport {
    #[serde(default)]
    errors: Vec<WorkflowValidationIssue>,
    #[serde(default)]
    warnings: Vec<WorkflowValidationIssue>,
}

impl WorkflowValidationReport {
    pub fn valid(&self) -> bool {
        self.errors.is_empty()
    }
}
impl WorkflowValidationIssue {
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn recovery(&self) -> &str {
        &self.recovery
    }
    pub fn node_id(&self) -> Option<&str> {
        self.node_id.as_deref()
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn code(&self) -> &str {
        &self.code
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowValidationIssue {
    code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    message: String,
    recovery: String,
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

/// Every mutation admitted by the Workflow runtime. One command type keeps
/// scope and revision guards at the boundary and gives every command the same
/// authoritative [`WorkflowSnapshot`] postcondition.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowCommand {
    Apply {
        scope_id: String,
        expected_revision: u64,
        workflow: WorkflowSpec,
    },
    Run {
        scope_id: String,
        revision: u64,
    },
    Cancel {
        scope_id: String,
    },
}

impl WorkflowCommand {
    pub fn apply(
        scope_id: impl Into<String>,
        expected_revision: u64,
        workflow: WorkflowSpec,
    ) -> Self {
        Self::Apply {
            scope_id: scope_id.into(),
            expected_revision,
            workflow,
        }
    }

    pub fn run(scope_id: impl Into<String>, revision: u64) -> Self {
        Self::Run {
            scope_id: scope_id.into(),
            revision,
        }
    }

    pub fn cancel(scope_id: impl Into<String>) -> Self {
        Self::Cancel {
            scope_id: scope_id.into(),
        }
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkflowSnapshot {
    /// Ephemeral server identity for the active Spiral destination/project.
    /// Revisions are monotonic only within one scope.
    #[serde(default)]
    scope_id: String,
    workflow: WorkflowSpec,
    validation: WorkflowValidationReport,
    snapshot: WorkflowRunSnapshot,
}

impl Default for WorkflowSnapshot {
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct RunIdentity {
    workflow_id: String,
    workflow_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct RunExecution {
    phase: WorkflowRunPhase,
    #[serde(default)]
    nodes: Vec<WorkflowNodeSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<SpiralQuerySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default)]
    result_stale: bool,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct RunTiming {
    #[serde(default)]
    started_ms: u64,
    #[serde(default)]
    finished_ms: u64,
    #[serde(default)]
    updated_ms: u64,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkflowRunSnapshot {
    #[serde(flatten)]
    identity: RunIdentity,
    #[serde(flatten)]
    execution: RunExecution,
    #[serde(flatten)]
    timing: RunTiming,
}

impl WorkflowRunSnapshot {
    pub fn idle_for(workflow: &WorkflowSpec) -> Self {
        Self {
            identity: RunIdentity {
                workflow_id: workflow.id.clone(),
                workflow_revision: workflow.revision,
                run_id: None,
            },
            execution: RunExecution {
                phase: WorkflowRunPhase::Idle,
                nodes: workflow
                    .nodes
                    .iter()
                    .map(|node| WorkflowNodeSnapshot::pending(node.id.clone(), "Not run"))
                    .collect(),
                ..RunExecution::default()
            },
            ..Self::default()
        }
    }

    /// Begin one admitted run. Runtime mutation remains behind focused
    /// transition methods instead of exposing independently constructible
    /// identity, execution, and timing fragments.
    pub fn started(workflow: &WorkflowSpec, run_id: impl Into<String>) -> Self {
        let mut snapshot = Self::idle_for(workflow);
        snapshot.identity.run_id = Some(run_id.into());
        snapshot.execution.phase = WorkflowRunPhase::Validating;
        snapshot
    }

    pub fn with_nodes(mut self, nodes: Vec<WorkflowNodeSnapshot>) -> Self {
        self.execution.nodes = nodes;
        self
    }

    pub const fn with_started_ms(mut self, started_ms: u64) -> Self {
        self.timing.started_ms = started_ms;
        self.timing.updated_ms = started_ms;
        self
    }

    pub fn workflow_id(&self) -> &str {
        &self.identity.workflow_id
    }
    pub const fn workflow_revision(&self) -> u64 {
        self.identity.workflow_revision
    }
    pub fn run_id(&self) -> Option<&str> {
        self.identity.run_id.as_deref()
    }
    pub const fn phase(&self) -> WorkflowRunPhase {
        self.execution.phase
    }
    pub fn nodes(&self) -> &[WorkflowNodeSnapshot] {
        &self.execution.nodes
    }
    pub const fn result(&self) -> Option<&SpiralQuerySnapshot> {
        self.execution.result.as_ref()
    }
    pub fn error(&self) -> Option<&str> {
        self.execution.error.as_deref()
    }
    pub const fn result_stale(&self) -> bool {
        self.execution.result_stale
    }
    pub const fn started_ms(&self) -> u64 {
        self.timing.started_ms
    }
    pub const fn updated_ms(&self) -> u64 {
        self.timing.updated_ms
    }
    pub const fn set_workflow_revision(&mut self, revision: u64) {
        self.identity.workflow_revision = revision;
    }
    pub const fn set_phase(&mut self, phase: WorkflowRunPhase) {
        self.execution.phase = phase;
    }
    pub fn nodes_mut(&mut self) -> &mut Vec<WorkflowNodeSnapshot> {
        &mut self.execution.nodes
    }
    pub fn set_result(&mut self, result: Option<SpiralQuerySnapshot>) {
        self.execution.result = result;
    }
    pub fn set_error(&mut self, error: Option<String>) {
        self.execution.error = error;
    }
    pub const fn set_result_stale(&mut self, result_stale: bool) {
        self.execution.result_stale = result_stale;
    }
    pub const fn set_finished_ms(&mut self, finished_ms: u64) {
        self.timing.finished_ms = finished_ms;
    }
    pub const fn set_updated_ms(&mut self, updated_ms: u64) {
        self.timing.updated_ms = updated_ms;
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_run_id(&mut self, run_id: Option<String>) {
        self.identity.run_id = run_id;
    }
    #[cfg(any(test, feature = "test-support"))]
    pub const fn set_started_ms(&mut self, started_ms: u64) {
        self.timing.started_ms = started_ms;
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct NodeOutcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rows: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    progress: Option<WorkflowNodeProgress>,
    #[serde(default)]
    quarantine: WorkflowQuarantineSummary,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct NodeArtifacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_relation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<WorkflowModelRunMetadata>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowNodeSnapshot {
    node_id: String,
    phase: WorkflowNodePhase,
    detail: String,
    #[serde(default)]
    elapsed_ms: u64,
    #[serde(flatten)]
    outcome: NodeOutcome,
    #[serde(flatten)]
    artifacts: NodeArtifacts,
}

impl WorkflowNodeSnapshot {
    pub fn new(
        node_id: impl Into<String>,
        phase: WorkflowNodePhase,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            phase,
            detail: detail.into(),
            elapsed_ms: 0,
            outcome: NodeOutcome::default(),
            artifacts: NodeArtifacts::default(),
        }
    }

    pub fn with_quarantine(mut self, quarantine: WorkflowQuarantineSummary) -> Self {
        self.outcome.quarantine = quarantine;
        self
    }

    pub fn with_artifacts(
        mut self,
        model: Option<WorkflowModelRunMetadata>,
        warnings: Vec<String>,
    ) -> Self {
        self.artifacts.model = model;
        self.artifacts.warnings = warnings;
        self
    }

    pub(crate) fn pending(node_id: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(node_id, WorkflowNodePhase::Pending, detail)
    }
    pub fn node_id(&self) -> &str {
        &self.node_id
    }
    pub const fn phase(&self) -> WorkflowNodePhase {
        self.phase
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
    pub fn error(&self) -> Option<&str> {
        self.outcome.error.as_deref()
    }
    pub const fn progress(&self) -> Option<&WorkflowNodeProgress> {
        self.outcome.progress.as_ref()
    }
    pub const fn quarantine(&self) -> &WorkflowQuarantineSummary {
        &self.outcome.quarantine
    }
    pub const fn model(&self) -> Option<&WorkflowModelRunMetadata> {
        self.artifacts.model.as_ref()
    }
    pub fn output_relation(&self) -> Option<&str> {
        self.artifacts.output_relation.as_deref()
    }
    pub fn warnings(&self) -> &[String] {
        &self.artifacts.warnings
    }
    pub const fn set_phase(&mut self, phase: WorkflowNodePhase) {
        self.phase = phase;
    }
    pub fn set_detail(&mut self, detail: impl Into<String>) {
        self.detail = detail.into();
    }
    pub const fn set_elapsed_ms(&mut self, elapsed_ms: u64) {
        self.elapsed_ms = elapsed_ms;
    }
    pub const fn set_rows(&mut self, rows: Option<usize>) {
        self.outcome.rows = rows;
    }
    pub fn set_error(&mut self, error: Option<String>) {
        self.outcome.error = error;
    }
    pub fn set_progress(&mut self, progress: Option<WorkflowNodeProgress>) {
        self.outcome.progress = progress;
    }
    pub fn set_quarantine(&mut self, quarantine: WorkflowQuarantineSummary) {
        self.outcome.quarantine = quarantine;
    }
    pub fn set_model(&mut self, model: Option<WorkflowModelRunMetadata>) {
        self.artifacts.model = model;
    }
    pub fn set_output_relation(&mut self, relation: Option<String>) {
        self.artifacts.output_relation = relation;
    }
    pub fn warnings_mut(&mut self) -> &mut Vec<String> {
        &mut self.artifacts.warnings
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_progress(mut self, progress: WorkflowNodeProgress) -> Self {
        self.outcome.progress = Some(progress);
        self
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowNodeProgress {
    completed: u64,
    total: u64,
    unit: String,
}

impl WorkflowNodeProgress {
    pub fn new(completed: u64, total: u64, unit: impl Into<String>) -> Self {
        Self {
            completed,
            total,
            unit: unit.into(),
        }
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ModelIdentity {
    model_id: String,
    artifact_revision: String,
    feature_cache_relation: String,
    fingerprint: String,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ModelExecution {
    #[serde(default)]
    features: Vec<WorkflowFeatureDefinition>,
    reused: bool,
    device: String,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct ModelMetrics {
    #[serde(default)]
    input_rows: u64,
    #[serde(default)]
    output_rows: u64,
    #[serde(default)]
    decode_ms: u64,
    #[serde(default)]
    inference_ms: u64,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowModelRunMetadata {
    #[serde(flatten)]
    identity: ModelIdentity,
    #[serde(flatten)]
    execution: ModelExecution,
    #[serde(flatten)]
    metrics: ModelMetrics,
}

impl WorkflowModelRunMetadata {
    pub fn new(model_id: impl Into<String>, artifact_revision: impl Into<String>) -> Self {
        Self {
            identity: ModelIdentity {
                model_id: model_id.into(),
                artifact_revision: artifact_revision.into(),
                feature_cache_relation: String::new(),
                fingerprint: String::new(),
            },
            execution: ModelExecution {
                features: Vec::new(),
                reused: false,
                device: String::new(),
            },
            metrics: ModelMetrics::default(),
        }
    }

    pub fn with_cache_relation(mut self, relation: impl Into<String>) -> Self {
        self.identity.feature_cache_relation = relation.into();
        self
    }

    pub fn with_features(mut self, features: Vec<WorkflowFeatureDefinition>) -> Self {
        self.execution.features = features;
        self
    }

    pub fn with_device(mut self, device: impl Into<String>) -> Self {
        self.execution.device = device.into();
        self
    }

    pub fn with_fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.identity.fingerprint = fingerprint.into();
        self
    }

    pub const fn with_reused(mut self, reused: bool) -> Self {
        self.execution.reused = reused;
        self
    }

    pub const fn with_row_counts(mut self, input_rows: u64, output_rows: u64) -> Self {
        self.metrics.input_rows = input_rows;
        self.metrics.output_rows = output_rows;
        self
    }

    pub const fn with_timings(mut self, decode_ms: u64, inference_ms: u64) -> Self {
        self.metrics.decode_ms = decode_ms;
        self.metrics.inference_ms = inference_ms;
        self
    }

    pub fn fingerprint(&self) -> &str {
        &self.identity.fingerprint
    }
    pub const fn reused(&self) -> bool {
        self.execution.reused
    }
    pub fn device(&self) -> &str {
        &self.execution.device
    }
    pub const fn decode_ms(&self) -> u64 {
        self.metrics.decode_ms
    }
    pub const fn inference_ms(&self) -> u64 {
        self.metrics.inference_ms
    }
    #[cfg(any(test, feature = "test-support"))]
    pub const fn input_rows(&self) -> u64 {
        self.metrics.input_rows
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn output_rows(&self) -> u64 {
        self.metrics.output_rows
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

/// Public, secret-free readiness for the backend-owned Workflow agent.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowAgentReadiness {
    Unconfigured { detail: String },
    Ready { detail: String },
    Error { detail: String },
}

impl Default for WorkflowAgentReadiness {
    fn default() -> Self {
        Self::Unconfigured {
            detail: "Workflow Assistant is not configured.".to_owned(),
        }
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentRole {
    User,
    Assistant,
    Synthetic,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentMessagePhase {
    Streaming,
    Complete,
    Failed,
    Cancelled,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentToolPhase {
    Running,
    Complete,
    Failed,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowAgentArtifactReceipt {
    Graph {
        label: String,
        revision: u64,
    },
    Run {
        label: String,
        run_id: String,
    },
    Results {
        label: String,
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        row_count: Option<usize>,
    },
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentToolSummary {
    call_id: String,
    name: String,
    phase: WorkflowAgentToolPhase,
    summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    receipt: Option<WorkflowAgentArtifactReceipt>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct MessageIdentity {
    id: String,
    sequence: u64,
    role: WorkflowAgentRole,
    created_ms: u64,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct MessageContent {
    phase: WorkflowAgentMessagePhase,
    text: String,
    tools: Vec<WorkflowAgentToolSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentMessage {
    #[serde(flatten)]
    identity: MessageIdentity,
    #[serde(flatten)]
    content: MessageContent,
}

impl WorkflowAgentMessage {
    pub fn new(id: impl Into<String>, role: WorkflowAgentRole) -> Self {
        Self {
            identity: MessageIdentity {
                id: id.into(),
                sequence: 0,
                role,
                created_ms: 0,
            },
            content: MessageContent {
                phase: WorkflowAgentMessagePhase::Streaming,
                text: String::new(),
                tools: Vec::new(),
                run_id: None,
            },
        }
    }

    pub const fn with_sequence(mut self, sequence: u64) -> Self {
        self.identity.sequence = sequence;
        self
    }

    pub const fn with_phase(mut self, phase: WorkflowAgentMessagePhase) -> Self {
        self.content.phase = phase;
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.content.text = text.into();
        self
    }

    pub const fn with_created_ms(mut self, created_ms: u64) -> Self {
        self.identity.created_ms = created_ms;
        self
    }

    pub fn with_run_id(mut self, run_id: Option<String>) -> Self {
        self.content.run_id = run_id;
        self
    }

    pub fn id(&self) -> &str {
        &self.identity.id
    }
    pub const fn sequence(&self) -> u64 {
        self.identity.sequence
    }
    pub const fn role(&self) -> WorkflowAgentRole {
        self.identity.role
    }
    pub const fn phase(&self) -> WorkflowAgentMessagePhase {
        self.content.phase
    }
    pub fn text(&self) -> &str {
        &self.content.text
    }
    pub fn tools(&self) -> &[WorkflowAgentToolSummary] {
        &self.content.tools
    }
    pub const fn set_phase(&mut self, phase: WorkflowAgentMessagePhase) {
        self.content.phase = phase;
    }
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.content.text = text.into();
    }
    pub fn tools_mut(&mut self) -> &mut Vec<WorkflowAgentToolSummary> {
        &mut self.content.tools
    }
    pub fn set_run_id(&mut self, run_id: Option<String>) {
        self.content.run_id = run_id;
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_tools(mut self, tools: Vec<WorkflowAgentToolSummary>) -> Self {
        self.content.tools = tools;
        self
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SessionIdentity {
    workflow_id: String,
    #[serde(default, alias = "session_id")]
    conversation_id: String,
    revision: u64,
    updated_ms: u64,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SessionActivity {
    phase: WorkflowAgentPhase,
    messages: Vec<WorkflowAgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deferred_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentSession {
    #[serde(flatten)]
    identity: SessionIdentity,
    #[serde(flatten)]
    activity: SessionActivity,
}

impl Default for WorkflowAgentSession {
    fn default() -> Self {
        Self::new(DEFAULT_WORKFLOW_ID, String::new())
    }
}

impl WorkflowAgentSession {
    pub fn new(workflow_id: impl Into<String>, conversation_id: impl Into<String>) -> Self {
        Self {
            identity: SessionIdentity {
                workflow_id: workflow_id.into(),
                conversation_id: conversation_id.into(),
                revision: 0,
                updated_ms: 0,
            },
            activity: SessionActivity {
                phase: WorkflowAgentPhase::Idle,
                messages: Vec::new(),
                active_turn_id: None,
                deferred_run_id: None,
                error: None,
            },
        }
    }

    pub const fn with_updated_ms(mut self, updated_ms: u64) -> Self {
        self.identity.updated_ms = updated_ms;
        self
    }
    pub fn workflow_id(&self) -> &str {
        &self.identity.workflow_id
    }
    pub fn conversation_id(&self) -> &str {
        &self.identity.conversation_id
    }
    pub const fn revision(&self) -> u64 {
        self.identity.revision
    }
    pub const fn phase(&self) -> WorkflowAgentPhase {
        self.activity.phase
    }
    pub fn messages(&self) -> &[WorkflowAgentMessage] {
        &self.activity.messages
    }
    pub fn active_turn_id(&self) -> Option<&str> {
        self.activity.active_turn_id.as_deref()
    }
    pub fn deferred_run_id(&self) -> Option<&str> {
        self.activity.deferred_run_id.as_deref()
    }
    pub fn error(&self) -> Option<&str> {
        self.activity.error.as_deref()
    }
    pub const fn set_revision(&mut self, revision: u64) {
        self.identity.revision = revision;
    }
    pub const fn set_phase(&mut self, phase: WorkflowAgentPhase) {
        self.activity.phase = phase;
    }
    pub fn messages_mut(&mut self) -> &mut Vec<WorkflowAgentMessage> {
        &mut self.activity.messages
    }
    pub fn set_active_turn_id(&mut self, turn_id: Option<String>) {
        self.activity.active_turn_id = turn_id;
    }
    pub fn set_deferred_run_id(&mut self, run_id: Option<String>) {
        self.activity.deferred_run_id = run_id;
    }
    pub fn set_error(&mut self, error: Option<String>) {
        self.activity.error = error;
    }
    pub const fn set_updated_ms(&mut self, updated_ms: u64) {
        self.identity.updated_ms = updated_ms;
    }
    #[cfg(any(test, feature = "test-support"))]
    pub const fn updated_ms(&self) -> u64 {
        self.identity.updated_ms
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_revision(mut self, revision: u64) -> Self {
        self.identity.revision = revision;
        self
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_messages(mut self, messages: Vec<WorkflowAgentMessage>) -> Self {
        self.activity.messages = messages;
        self
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_workflow_id(&mut self, workflow_id: impl Into<String>) {
        self.identity.workflow_id = workflow_id.into();
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentConversationSummary {
    conversation_id: String,
    title: String,
    created_ms: u64,
    updated_ms: u64,
    message_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owned_run_id: Option<String>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct AgentStateContext {
    history_available: bool,
    scope_id: String,
    revision: u64,
    active_conversation_id: String,
    conversation_limit: usize,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAgentSnapshot {
    readiness: WorkflowAgentReadiness,
    #[serde(flatten)]
    context: AgentStateContext,
    #[serde(default)]
    conversations: Vec<WorkflowAgentConversationSummary>,
    session: WorkflowAgentSession,
}

impl WorkflowAgentSnapshot {
    pub fn new(readiness: WorkflowAgentReadiness, session: WorkflowAgentSession) -> Self {
        Self {
            readiness,
            context: AgentStateContext::default(),
            conversations: Vec::new(),
            session,
        }
    }

    pub const fn with_history_available(mut self, history_available: bool) -> Self {
        self.context.history_available = history_available;
        self
    }

    pub fn with_scope(mut self, scope_id: impl Into<String>, revision: u64) -> Self {
        self.context.scope_id = scope_id.into();
        self.context.revision = revision;
        self
    }

    pub fn with_active_conversation(
        mut self,
        conversation_id: impl Into<String>,
        limit: usize,
    ) -> Self {
        self.context.active_conversation_id = conversation_id.into();
        self.context.conversation_limit = limit;
        self
    }
    pub fn with_conversations(
        mut self,
        conversations: Vec<WorkflowAgentConversationSummary>,
    ) -> Self {
        self.conversations = conversations;
        self
    }
    pub const fn readiness(&self) -> &WorkflowAgentReadiness {
        &self.readiness
    }
    pub const fn history_available(&self) -> bool {
        self.context.history_available
    }
    pub fn scope_id(&self) -> &str {
        &self.context.scope_id
    }
    pub const fn revision(&self) -> u64 {
        self.context.revision
    }
    pub fn active_conversation_id(&self) -> &str {
        &self.context.active_conversation_id
    }
    pub fn conversations(&self) -> &[WorkflowAgentConversationSummary] {
        &self.conversations
    }
    pub const fn conversation_limit(&self) -> usize {
        self.context.conversation_limit
    }
    pub const fn session(&self) -> &WorkflowAgentSession {
        &self.session
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn conversations_mut(&mut self) -> &mut Vec<WorkflowAgentConversationSummary> {
        &mut self.conversations
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn session_mut(&mut self) -> &mut WorkflowAgentSession {
        &mut self.session
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn set_history_available(&mut self, history_available: bool) {
        self.context.history_available = history_available;
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn set_revision(&mut self, revision: u64) {
        self.context.revision = revision;
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_scope_id(&mut self, scope_id: impl Into<String>) {
        self.context.scope_id = scope_id.into();
    }
}

/// Runtime steering accepted by the backend-owned Workflow Assistant.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowAgentCommand {
    CreateConversation {
        scope_id: String,
        expected_revision: u64,
    },
    ActivateConversation {
        scope_id: String,
        conversation_id: String,
        expected_revision: u64,
    },
    Turn {
        scope_id: String,
        conversation_id: String,
        text: String,
        expected_revision: u64,
    },
    Cancel {
        scope_id: String,
        conversation_id: String,
        turn_id: String,
    },
}

/// Reserved bounded diagnostic shape for later decode/model phases.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowQuarantineSummary {
    count: u64,
    #[serde(default)]
    examples: Vec<String>,
}

impl WorkflowSpec {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            revision: 0,
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub const fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }

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
                WorkflowNodeConfig::Model { features } => {
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
            if !source.config.allows_transition_to(&target.config) {
                report.errors.push(
                    WorkflowValidationIssue::new(
                        "invalid_transition",
                        &format!(
                            "{} cannot feed {} in a relation-first workflow.",
                            source.config.label(),
                            target.config.label(),
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

        let topology = WorkflowTopology {
            nodes,
            incoming,
            outgoing,
        };
        let nodes = &topology.nodes;
        let incoming = &topology.incoming;
        let outgoing = &topology.outgoing;
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
                    .is_some_and(|source| !matches!(&source.config, WorkflowNodeConfig::Query { .. })) =>
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
            if let WorkflowNodeConfig::Model { features } = &node.config {
                let table_sources = topology.source_relations(&node.id);
                if table_sources.len() != 1 {
                    report.errors.push(WorkflowValidationIssue::new("model_source_count", "A Model feature cache must belong to exactly one authoritative source relation.", "Build the candidate Query from one source Table; join other evidence only after the Model.").with_node_id(&node.id));
                } else if let Some(source) = table_sources.first()
                    && features
                        .iter()
                        .any(|feature| feature.applicability.source_relation != **source)
                {
                    report.errors.push(WorkflowValidationIssue::new("feature_source_mismatch", "Feature applicability does not match the Model's authoritative source Table.", "Apply the workflow again so the server can normalize every feature source.").with_node_id(&node.id));
                }
            }
        }

        if topology.ordered_nodes().is_err() {
            report.errors.push(WorkflowValidationIssue::new(
                "workflow_cycle",
                "Workflow lineage must be acyclic.",
                "Remove the connection that feeds a downstream node back upstream.",
            ));
        }
        report
    }

    pub fn topological_order(&self) -> Result<Vec<String>, String> {
        let topology = WorkflowTopology::from(self);
        if topology.nodes.len() != self.nodes.len() {
            return Err("Workflow node identities must be unique.".to_owned());
        }
        topology.ordered_nodes()
    }

    /// Derive model applicability from graph lineage in both the editor and backend.
    pub fn normalize_model_sources(&mut self) {
        let topology = WorkflowTopology::from(&*self);
        let sources = self
            .nodes
            .iter()
            .filter(|node| matches!(node.config, WorkflowNodeConfig::Model { .. }))
            .map(|node| {
                let relations = topology.source_relations(&node.id);
                let source = if relations.len() == 1 {
                    relations.into_iter().next().unwrap().to_owned()
                } else {
                    String::new()
                };
                (node.id.clone(), source)
            })
            .collect::<BTreeMap<_, _>>();
        for node in &mut self.nodes {
            if let WorkflowNodeConfig::Model { features } = &mut node.config {
                for feature in features {
                    feature.set_source_relation(sources[&node.id].clone());
                }
            }
        }
    }

    pub fn has_model(&self) -> bool {
        self.nodes
            .iter()
            .any(|node| matches!(node.config, WorkflowNodeConfig::Model { .. }))
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
}

impl WorkflowSpec {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn revision(&self) -> u64 {
        self.revision
    }
    pub fn nodes(&self) -> &[WorkflowNodeSpec] {
        &self.nodes
    }
    pub fn edges(&self) -> &[WorkflowEdgeSpec] {
        &self.edges
    }
    pub fn nodes_mut(&mut self) -> &mut Vec<WorkflowNodeSpec> {
        &mut self.nodes
    }
    pub fn edges_mut(&mut self) -> &mut Vec<WorkflowEdgeSpec> {
        &mut self.edges
    }
    pub const fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }
}
impl WorkflowNodeSpec {
    pub fn new(id: impl Into<String>, name: impl Into<String>, config: WorkflowNodeConfig) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            config,
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn config(&self) -> &WorkflowNodeConfig {
        &self.config
    }
    pub fn config_mut(&mut self) -> &mut WorkflowNodeConfig {
        &mut self.config
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_config(&mut self, config: WorkflowNodeConfig) {
        self.config = config;
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_id(&mut self, id: impl Into<String>) {
        self.id = id.into();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
}
impl WorkflowEdgeSpec {
    pub fn new(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
        }
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn target(&self) -> &str {
        &self.target
    }
}
impl WorkflowValidationReport {
    pub fn errors(&self) -> &[WorkflowValidationIssue] {
        &self.errors
    }
}
impl WorkflowSnapshot {
    pub fn from_workflow(scope_id: impl Into<String>, workflow: WorkflowSpec) -> Self {
        let validation = workflow.validate();
        let snapshot = WorkflowRunSnapshot::idle_for(&workflow);
        Self {
            scope_id: scope_id.into(),
            workflow,
            validation,
            snapshot,
        }
    }

    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }
    pub const fn workflow(&self) -> &WorkflowSpec {
        &self.workflow
    }
    pub const fn validation(&self) -> &WorkflowValidationReport {
        &self.validation
    }
    pub const fn snapshot(&self) -> &WorkflowRunSnapshot {
        &self.snapshot
    }
    pub fn set_snapshot(&mut self, snapshot: WorkflowRunSnapshot) {
        self.snapshot = snapshot;
    }
    pub fn set_scope_id(&mut self, scope_id: impl Into<String>) {
        self.scope_id = scope_id.into();
    }
    #[cfg(any(test, feature = "test-support"))]
    pub const fn snapshot_mut(&mut self) -> &mut WorkflowRunSnapshot {
        &mut self.snapshot
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn workflow_mut(&mut self) -> &mut WorkflowSpec {
        &mut self.workflow
    }
}

impl WorkflowNodeProgress {
    pub const fn completed(&self) -> u64 {
        self.completed
    }
    pub const fn total(&self) -> u64 {
        self.total
    }
    pub fn unit(&self) -> &str {
        &self.unit
    }
}
impl WorkflowAgentReadiness {
    pub fn ready(detail: impl Into<String>) -> Self {
        Self::Ready {
            detail: detail.into(),
        }
    }

    pub fn error(detail: impl Into<String>) -> Self {
        Self::Error {
            detail: detail.into(),
        }
    }

    pub const fn configured(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Unconfigured { detail } | Self::Ready { detail, .. } | Self::Error { detail } => {
                detail
            }
        }
    }
}
impl WorkflowAgentArtifactReceipt {
    pub fn graph(label: impl Into<String>, revision: u64) -> Self {
        Self::Graph {
            label: label.into(),
            revision,
        }
    }

    pub fn run(label: impl Into<String>, run_id: impl Into<String>) -> Self {
        Self::Run {
            label: label.into(),
            run_id: run_id.into(),
        }
    }

    pub fn results(
        label: impl Into<String>,
        run_id: impl Into<String>,
        row_count: Option<usize>,
    ) -> Self {
        Self::Results {
            label: label.into(),
            run_id: run_id.into(),
            row_count,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Graph { label, .. } | Self::Run { label, .. } | Self::Results { label, .. } => {
                label
            }
        }
    }

    pub fn run_id(&self) -> Option<&str> {
        match self {
            Self::Run { run_id, .. } | Self::Results { run_id, .. } => Some(run_id),
            Self::Graph { .. } => None,
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    pub const fn revision(&self) -> Option<u64> {
        match self {
            Self::Graph { revision, .. } => Some(*revision),
            Self::Run { .. } | Self::Results { .. } => None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn row_count(&self) -> Option<usize> {
        match self {
            Self::Results { row_count, .. } => *row_count,
            Self::Graph { .. } | Self::Run { .. } => None,
        }
    }
}
impl WorkflowAgentToolSummary {
    pub fn new(
        call_id: impl Into<String>,
        name: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            phase: WorkflowAgentToolPhase::Running,
            summary: summary.into(),
            error: None,
            receipt: None,
        }
    }
    pub const fn set_phase(&mut self, phase: WorkflowAgentToolPhase) {
        self.phase = phase;
    }
    pub fn set_summary(&mut self, summary: impl Into<String>) {
        self.summary = summary.into();
    }
    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }
    pub fn set_receipt(&mut self, receipt: Option<WorkflowAgentArtifactReceipt>) {
        self.receipt = receipt;
    }
    pub fn call_id(&self) -> &str {
        &self.call_id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn phase(&self) -> WorkflowAgentToolPhase {
        self.phase
    }
    pub fn summary(&self) -> &str {
        &self.summary
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub const fn receipt(&self) -> Option<&WorkflowAgentArtifactReceipt> {
        self.receipt.as_ref()
    }
}
impl WorkflowAgentConversationSummary {
    pub fn new(
        conversation_id: impl Into<String>,
        title: impl Into<String>,
        created_ms: u64,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            title: title.into(),
            created_ms,
            updated_ms: created_ms,
            message_count: 0,
            owned_run_id: None,
        }
    }
    pub const fn with_activity(mut self, updated_ms: u64, message_count: usize) -> Self {
        self.updated_ms = updated_ms;
        self.message_count = message_count;
        self
    }
    pub fn with_owned_run_id(mut self, owned_run_id: Option<String>) -> Self {
        self.owned_run_id = owned_run_id;
        self
    }
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub const fn message_count(&self) -> usize {
        self.message_count
    }
    pub fn owned_run_id(&self) -> Option<&str> {
        self.owned_run_id.as_deref()
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_owned_run_id(&mut self, owned_run_id: Option<String>) {
        self.owned_run_id = owned_run_id;
    }
}
impl WorkflowAgentCommand {
    pub fn create_conversation(scope_id: impl Into<String>, expected_revision: u64) -> Self {
        Self::CreateConversation {
            scope_id: scope_id.into(),
            expected_revision,
        }
    }

    pub fn activate_conversation(
        scope_id: impl Into<String>,
        conversation_id: impl Into<String>,
        expected_revision: u64,
    ) -> Self {
        Self::ActivateConversation {
            scope_id: scope_id.into(),
            conversation_id: conversation_id.into(),
            expected_revision,
        }
    }

    pub fn cancel(
        scope_id: impl Into<String>,
        conversation_id: impl Into<String>,
        turn_id: impl Into<String>,
    ) -> Self {
        Self::Cancel {
            scope_id: scope_id.into(),
            conversation_id: conversation_id.into(),
            turn_id: turn_id.into(),
        }
    }

    pub fn scope_id(&self) -> &str {
        match self {
            Self::CreateConversation { scope_id, .. }
            | Self::ActivateConversation { scope_id, .. }
            | Self::Turn { scope_id, .. }
            | Self::Cancel { scope_id, .. } => scope_id,
        }
    }

    pub fn conversation_id(&self) -> Option<&str> {
        match self {
            Self::ActivateConversation {
                conversation_id, ..
            }
            | Self::Turn {
                conversation_id, ..
            }
            | Self::Cancel {
                conversation_id, ..
            } => Some(conversation_id),
            Self::CreateConversation { .. } => None,
        }
    }
}
impl WorkflowQuarantineSummary {
    pub const fn count(&self) -> u64 {
        self.count
    }
    pub fn examples(&self) -> &[String] {
        &self.examples
    }
    pub const fn set_count(&mut self, count: u64) {
        self.count = count;
    }
    pub fn examples_mut(&mut self) -> &mut Vec<String> {
        &mut self.examples
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(count: u64, examples: Vec<String>) -> Self {
        Self { count, examples }
    }
}

/// One indexed representation of lineage, shared by validation and normalization.
struct WorkflowTopology<'a> {
    nodes: BTreeMap<&'a str, &'a WorkflowNodeSpec>,
    incoming: BTreeMap<&'a str, Vec<&'a str>>,
    outgoing: BTreeMap<&'a str, Vec<&'a str>>,
}

impl<'a> From<&'a WorkflowSpec> for WorkflowTopology<'a> {
    fn from(spec: &'a WorkflowSpec) -> Self {
        let mut topology = Self {
            nodes: spec
                .nodes
                .iter()
                .map(|node| (node.id.as_str(), node))
                .collect(),
            incoming: BTreeMap::new(),
            outgoing: BTreeMap::new(),
        };
        for edge in &spec.edges {
            topology
                .incoming
                .entry(&edge.target)
                .or_default()
                .push(&edge.source);
            topology
                .outgoing
                .entry(&edge.source)
                .or_default()
                .push(&edge.target);
        }
        topology
    }
}

impl<'a> WorkflowTopology<'a> {
    fn source_relations(&self, node_id: &str) -> BTreeSet<&'a str> {
        let mut sources = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut pending = self.incoming.get(node_id).cloned().unwrap_or_default();
        while let Some(upstream) = pending.pop() {
            if !visited.insert(upstream) {
                continue;
            }
            match self.nodes.get(upstream).map(|node| &node.config) {
                Some(WorkflowNodeConfig::Table { relation }) => {
                    sources.insert(relation.as_str());
                }
                Some(_) => {
                    pending.extend(self.incoming.get(upstream).into_iter().flatten().copied())
                }
                None => {}
            }
        }
        sources
    }

    fn ordered_nodes(&self) -> Result<Vec<String>, String> {
        for source in self.outgoing.keys() {
            if !self.nodes.contains_key(source) {
                return Err(format!("unknown source {source:?}"));
            }
        }
        for target in self.incoming.keys() {
            if !self.nodes.contains_key(target) {
                return Err(format!("unknown target {target:?}"));
            }
        }
        let mut degrees = self
            .nodes
            .keys()
            .map(|id| (*id, self.incoming.get(id).map_or(0, Vec::len)))
            .collect::<BTreeMap<_, _>>();
        let mut ready = degrees
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
            .collect::<VecDeque<_>>();
        let mut ordered = Vec::with_capacity(self.nodes.len());
        while let Some(id) = ready.pop_front() {
            ordered.push(id.to_owned());
            for target in self.outgoing.get(id).into_iter().flatten() {
                let degree = degrees.get_mut(target).expect("edges were checked");
                *degree -= 1;
                if *degree == 0 {
                    ready.push_back(target);
                }
            }
        }
        if ordered.len() == self.nodes.len() {
            Ok(ordered)
        } else {
            Err("workflow contains a cycle".to_owned())
        }
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
    fn api_state_and_sse_events_share_one_authoritative_shape() {
        let workflow = runnable_spec();
        let state = WorkflowSnapshot {
            scope_id: "workflow-scope-1".to_owned(),
            validation: workflow.validate(),
            snapshot: WorkflowRunSnapshot::idle_for(&workflow),
            workflow,
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["workflow"]["id"], DEFAULT_WORKFLOW_ID);
        assert_eq!(json["scope_id"], "workflow-scope-1");
        assert_eq!(json["snapshot"]["phase"], "idle");
        assert!(json["validation"]["errors"].as_array().unwrap().is_empty());
        let decoded: WorkflowSnapshot = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn agent_state_and_named_update_are_serde_stable_and_secret_free() {
        let readiness = WorkflowAgentReadiness::ready("Ready");
        let conversation = WorkflowAgentConversationSummary {
            conversation_id: "conversation-1".to_owned(),
            title: "Inspect the catalog".to_owned(),
            created_ms: 9,
            updated_ms: 10,
            message_count: 1,
            owned_run_id: None,
        };
        let message = WorkflowAgentMessage::new("message-1", WorkflowAgentRole::Assistant)
            .with_sequence(1)
            .with_phase(WorkflowAgentMessagePhase::Complete)
            .with_text("I inspected the catalog.")
            .with_tools(vec![WorkflowAgentToolSummary {
                call_id: "call-1".to_owned(),
                name: "list_relations".to_owned(),
                phase: WorkflowAgentToolPhase::Complete,
                summary: "Relation catalog complete".to_owned(),
                error: None,
                receipt: None,
            }])
            .with_created_ms(10);
        let session = WorkflowAgentSession::new(DEFAULT_WORKFLOW_ID, "conversation-1")
            .with_revision(2)
            .with_messages(vec![message]);
        let state = WorkflowAgentSnapshot::new(readiness, session)
            .with_history_available(true)
            .with_scope("agent-scope-1", 3)
            .with_active_conversation("conversation-1", 20)
            .with_conversations(vec![conversation]);
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["session"]["revision"], 2);
        assert_eq!(json["readiness"]["status"], "ready");
        assert_eq!(json["revision"], 3);
        assert_eq!(json["active_conversation_id"], "conversation-1");
        assert_eq!(json["conversations"][0]["title"], "Inspect the catalog");
        let encoded = json.to_string();
        assert!(!encoded.contains("api_key"));
        assert!(!encoded.contains("client_secret"));
        let decoded: WorkflowAgentSnapshot = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn agent_conversation_commands_are_explicitly_scoped_and_secret_free() {
        let create = WorkflowAgentCommand::create_conversation("scope-1", 4);
        let activate = WorkflowAgentCommand::activate_conversation("scope-1", "conversation-2", 4);
        let cancel = WorkflowAgentCommand::cancel("scope-1", "conversation-2", "turn-8");
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
    fn feature_cache_name_is_server_owned_and_stable() {
        assert_eq!(
            WorkflowFeatureApplicability::derived_cache_name("pick/place"),
            format!(
                "features_{}",
                crate::identity::SqlNameComponent::from("pick/place").as_str()
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
    fn model_source_normalization_follows_lineage_and_clears_ambiguous_or_missing_sources() {
        let mut spec = runnable_spec();
        spec.nodes.push(WorkflowNodeSpec::new(
            "model",
            "Features",
            WorkflowNodeConfig::Model {
                features: vec![feature("stale.source", "is_raining_feature", "rain")],
            },
        ));
        spec.edges.push(WorkflowEdgeSpec::new("query", "model"));
        spec.normalize_model_sources();
        let source = |spec: &WorkflowSpec| match &spec.nodes.last().unwrap().config {
            WorkflowNodeConfig::Model { features } => {
                features[0].applicability().source_relation().to_owned()
            }
            _ => unreachable!(),
        };
        assert_eq!(source(&spec), "pick_place.camera_frames");
        spec.nodes.insert(
            0,
            WorkflowNodeSpec::new(
                "other",
                "Other frames",
                WorkflowNodeConfig::Table {
                    relation: "other.frames".to_owned(),
                },
            ),
        );
        spec.edges.push(WorkflowEdgeSpec::new("other", "query"));
        spec.normalize_model_sources();
        assert!(source(&spec).is_empty());
        assert!(
            spec.validate()
                .errors
                .iter()
                .any(|issue| issue.code == "model_source_count")
        );
        spec.edges.retain(|edge| edge.source != "other");
        spec.normalize_model_sources();
        assert_eq!(source(&spec), "pick_place.camera_frames");
        spec.edges.retain(|edge| edge.target != "model");
        spec.normalize_model_sources();
        assert!(source(&spec).is_empty());
    }

    #[test]
    fn graph_validation_rejects_duplicates_cycles_and_nonterminal_results() {
        let mut duplicate = runnable_spec();
        duplicate.nodes.push(duplicate.nodes[0].clone());
        assert!(duplicate.topological_order().is_err());
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
                    features: vec![
                        feature(
                            "pick_place.camera_frames",
                            "is_red_cube_feature",
                            "red cube",
                        ),
                        feature("pick_place.camera_frames", "is_gripper_feature", "gripper"),
                    ],
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
                        features: vec![feature(
                            "pick_place.video_manifest",
                            "is_holding_cube_feature",
                            "holding cube",
                        )],
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
            "features": [{
                "name": "is_raining_feature",
                "description": "Probability that rain is visible.",
                "concept": "raining",
                "applicability": {
                    "source_relation": "weather.camera_frames",
                    "media_field": "image",
                    "stream_ids": ["camera.front"]
                }
            }]
        }))
        .unwrap();
        let WorkflowNodeConfig::Model { features, .. } = model else {
            unreachable!()
        };
        assert_eq!(features[0].name, "is_raining_feature");
        assert_eq!(features[0].applicability.stream_ids, ["camera.front"]);
    }

    #[test]
    fn model_wire_shape_rejects_backend_owned_identity_and_output_fields() {
        for forbidden in ["model_id", "feature_cache_relation"] {
            let mut model = serde_json::json!({
                "kind": "model",
                "features": []
            });
            model[forbidden] = serde_json::json!("caller-controlled");
            assert!(serde_json::from_value::<WorkflowNodeConfig>(model).is_err());
        }
    }

    #[test]
    fn model_output_is_derived_from_typed_feature_applicability() {
        let config = WorkflowNodeConfig::Model {
            features: vec![feature(
                "weather.camera_frames",
                "is_raining_feature",
                "rain",
            )],
        };
        assert_eq!(
            config.model_output_name(),
            Some(WorkflowFeatureApplicability::derived_cache_name(
                "weather.camera_frames"
            ))
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
