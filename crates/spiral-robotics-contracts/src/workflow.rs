//! Serde-stable Workflow API and persistence contract.
//!
//! Workflow starts at relations already materialized in Spiral. External
//! ingestion and model execution are deliberately outside the Stage-2 runtime;
//! the reserved Model schema lets a saved graph survive the later capability
//! addition without pretending that it can run today.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use spiral_robotics_core::query::{ReadOnlyMode, ensure_read_only};
use spiral_robotics_core::workflow::{
    is_workflow_node_id, scan_node_placeholders, validate_qualified_relation,
};

use crate::api::{QueryColumn, SpiralQueryResponse};

pub const DEFAULT_WORKFLOW_ID: &str = "default";
pub const MAX_WORKFLOW_QUERY_ROWS: usize = 1_000;

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
    /// Reserved for Stage 3. The server derives `output_relation`; callers do
    /// not choose a writable identifier.
    Model {
        model_id: String,
        #[serde(default)]
        configuration: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_relation: Option<String>,
    },
    Query {
        sql: String,
        #[serde(default = "default_workflow_query_rows")]
        max_rows: usize,
    },
    Result,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SaveWorkflowRequest {
    pub expected_revision: u64,
    pub workflow: WorkflowSpec,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunWorkflowRequest {
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkflowStateResponse {
    pub workflow: WorkflowSpec,
    pub validation: WorkflowValidationReport,
    pub snapshot: WorkflowRunSnapshot,
}

impl Default for WorkflowStateResponse {
    fn default() -> Self {
        let workflow = WorkflowSpec::default();
        Self {
            validation: validate_workflow_spec(&workflow),
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
                    elapsed_ms: 0,
                    rows: None,
                    error: None,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodePhase {
    #[default]
    Pending,
    Validating,
    Running,
    Complete,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowCapabilities {
    pub schema_version: u32,
    pub node_kinds: Vec<WorkflowNodeCapability>,
    pub max_query_rows: usize,
    pub relation_discovery: String,
}

impl Default for WorkflowCapabilities {
    fn default() -> Self {
        Self {
            schema_version: 1,
            node_kinds: vec![
                WorkflowNodeCapability {
                    kind: WorkflowNodeKind::Table,
                    executable: true,
                    detail: "Reads an existing qualified Spiral relation.".to_owned(),
                },
                WorkflowNodeCapability {
                    kind: WorkflowNodeKind::Model,
                    executable: false,
                    detail: "Model execution arrives in Stage 3.".to_owned(),
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
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowNodeCapability {
    pub kind: WorkflowNodeKind,
    pub executable: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowRelationsResponse {
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

pub fn validate_workflow_spec(spec: &WorkflowSpec) -> WorkflowValidationReport {
    let mut report = WorkflowValidationReport::default();
    if !is_workflow_node_id(&spec.id) {
        report.errors.push(issue(
            "invalid_workflow_id",
            None,
            "Workflow ID must start with a letter or '_' and contain only letters, numbers, '_' or '-'.",
            "Use a stable identifier such as relation-query.",
        ));
    }
    if spec.name.trim().is_empty() {
        report.errors.push(issue(
            "missing_workflow_name",
            None,
            "Workflow name cannot be empty.",
            "Name the saved workflow before applying it.",
        ));
    }

    let mut nodes = BTreeMap::new();
    for node in &spec.nodes {
        if !is_workflow_node_id(&node.id) {
            report.errors.push(issue(
                "invalid_node_id",
                Some(&node.id),
                "Node IDs may contain letters, numbers, '_' and '-' only.",
                "Choose a stable node ID without SQL punctuation.",
            ));
        }
        if nodes.insert(node.id.as_str(), node).is_some() {
            report.errors.push(issue(
                "duplicate_node_id",
                Some(&node.id),
                "Node IDs must be unique within a workflow.",
                "Rename one of the duplicate nodes.",
            ));
        }
        match &node.config {
            WorkflowNodeConfig::Table { relation } => {
                if relation.is_empty() {
                    report.errors.push(issue(
                        "table_relation_required",
                        Some(&node.id),
                        "Choose an existing Spiral relation for this Table node.",
                        "Select a qualified relation discovered from the active Spiral project.",
                    ));
                } else if let Err(error) = validate_qualified_relation(relation) {
                    report.errors.push(issue(
                        "invalid_relation",
                        Some(&node.id),
                        &error.to_string(),
                        "Choose a relation returned by Workflow relation discovery.",
                    ));
                }
            }
            WorkflowNodeConfig::Model {
                model_id,
                output_relation,
                ..
            } => {
                if model_id.trim().is_empty() {
                    report.errors.push(issue(
                        "model_id_required",
                        Some(&node.id),
                        "Reserved Model nodes require a model capability ID.",
                        "Remove the Model node for Stage 2 or choose a future advertised capability.",
                    ));
                }
                if output_relation
                    .as_ref()
                    .is_some_and(|relation| validate_qualified_relation(relation).is_err())
                {
                    report.errors.push(issue(
                        "invalid_model_output",
                        Some(&node.id),
                        "The server-derived Model output relation is invalid.",
                        "Apply the workflow again so the server can derive the relation name.",
                    ));
                }
                report.warnings.push(issue(
                    "model_execution_unavailable",
                    Some(&node.id),
                    "Model execution is not available in Stage 2.",
                    "Remove this Model node to run SQL now; Model execution arrives in Stage 3.",
                ));
            }
            WorkflowNodeConfig::Query { sql, max_rows } => {
                if let Err(error) = ensure_read_only(sql, ReadOnlyMode::Training) {
                    report.errors.push(issue(
                        "query_not_read_only",
                        Some(&node.id),
                        &error.to_string().replace("Training SQL", "Workflow SQL"),
                        "Use one read-only SELECT or WITH query.",
                    ));
                }
                if !(1..=MAX_WORKFLOW_QUERY_ROWS).contains(max_rows) {
                    report.errors.push(issue(
                        "query_row_limit",
                        Some(&node.id),
                        &format!(
                            "Workflow query row limit must be between 1 and {MAX_WORKFLOW_QUERY_ROWS}."
                        ),
                        "Choose a bounded row limit.",
                    ));
                }
                if let Err(error) = scan_node_placeholders(sql) {
                    report.errors.push(issue(
                        "malformed_placeholder",
                        Some(&node.id),
                        &error.to_string(),
                        "Use exact placeholders such as {{node:table}}.",
                    ));
                }
            }
            WorkflowNodeConfig::Result => {}
        }
    }

    if spec.nodes.is_empty() {
        report.errors.push(issue(
            "empty_workflow",
            None,
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
            report.errors.push(issue(
                "duplicate_edge",
                Some(&edge.target),
                "Workflow edges must be unique.",
                "Remove the repeated connection.",
            ));
        }
        let Some(source) = nodes.get(edge.source.as_str()) else {
            report.errors.push(issue(
                "unknown_edge_source",
                Some(&edge.source),
                "A workflow edge references an unknown source node.",
                "Reconnect the edge to an existing node.",
            ));
            continue;
        };
        let Some(target) = nodes.get(edge.target.as_str()) else {
            report.errors.push(issue(
                "unknown_edge_target",
                Some(&edge.target),
                "A workflow edge references an unknown target node.",
                "Reconnect the edge to an existing node.",
            ));
            continue;
        };
        if !allowed_transition(source.config.kind(), target.config.kind()) {
            report.errors.push(issue(
                "invalid_transition",
                Some(&edge.target),
                &format!(
                    "{} cannot feed {} in a relation-first workflow.",
                    source.config.kind().label(),
                    target.config.kind().label(),
                ),
                "Use Table→Query, Table→Model, Model→Query, or Query→Result.",
            ));
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
        report.errors.push(issue(
            "missing_table_root",
            None,
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
        report.errors.push(issue(
            "missing_result",
            None,
            "A Stage-2 Workflow needs exactly one terminal Result node.",
            "Connect the final Query to one Result node.",
        ));
    } else if result_count > 1 {
        report.errors.push(issue(
            "multiple_results",
            None,
            "A Stage-2 Workflow supports exactly one terminal Result node.",
            "Keep one Result node so the run owns one unambiguous payload.",
        ));
    }

    for node in &spec.nodes {
        let in_degree = incoming.get(node.id.as_str()).map_or(0, Vec::len);
        let out_degree = outgoing.get(node.id.as_str()).map_or(0, Vec::len);
        if out_degree == 0 && !matches!(node.config, WorkflowNodeConfig::Result) {
            report.errors.push(issue(
                "orphan_terminal",
                Some(&node.id),
                "Every non-Result node must contribute to the terminal Result.",
                "Connect this node downstream or remove the orphan branch.",
            ));
        }
        match &node.config {
            WorkflowNodeConfig::Table { .. } if in_degree != 0 => report.errors.push(issue(
                "table_not_root",
                Some(&node.id),
                "Table nodes must be roots.",
                "Remove incoming edges from this Table node.",
            )),
            WorkflowNodeConfig::Result if out_degree != 0 => report.errors.push(issue(
                "result_not_terminal",
                Some(&node.id),
                "Result nodes must be terminal.",
                "Remove outgoing edges from this Result node.",
            )),
            WorkflowNodeConfig::Result if in_degree != 1 => report.errors.push(issue(
                "result_input_count",
                Some(&node.id),
                "A Result node must have exactly one Query input.",
                "Connect one Query node to this Result.",
            )),
            WorkflowNodeConfig::Query { sql, .. } => {
                if let Ok(placeholders) = scan_node_placeholders(sql) {
                    let upstream = incoming
                        .get(node.id.as_str())
                        .into_iter()
                        .flatten()
                        .copied()
                        .collect::<BTreeSet<_>>();
                    if placeholders.is_empty() {
                        report.errors.push(issue(
                            "query_placeholder_required",
                            Some(&node.id),
                            "Workflow SQL must reference an upstream node placeholder.",
                            "Use a placeholder such as {{node:table}} in FROM or JOIN.",
                        ));
                    }
                    for placeholder in &placeholders {
                        if !nodes.contains_key(placeholder.as_str()) {
                            report.errors.push(issue(
                                "unknown_placeholder",
                                Some(&node.id),
                                &format!("Workflow SQL references unknown node {placeholder:?}."),
                                "Reference an existing upstream Table or Model node.",
                            ));
                        } else if !upstream.contains(placeholder.as_str()) {
                            report.errors.push(issue(
                                "non_upstream_placeholder",
                                Some(&node.id),
                                &format!(
                                    "Workflow SQL references {placeholder:?}, which is not connected upstream."
                                ),
                                "Connect that node to this Query or remove the placeholder.",
                            ));
                        }
                    }
                    for upstream_id in upstream {
                        if !placeholders.iter().any(|id| id == upstream_id) {
                            report.errors.push(issue(
                                "unused_upstream_edge",
                                Some(&node.id),
                                &format!(
                                    "Upstream node {upstream_id:?} is connected but not referenced by Workflow SQL."
                                ),
                                "Use its {{node:<id>}} placeholder or remove the edge.",
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if topological_order(spec).is_err() {
        report.errors.push(issue(
            "workflow_cycle",
            None,
            "Workflow lineage must be acyclic.",
            "Remove the connection that feeds a downstream node back upstream.",
        ));
    }
    report
}

pub fn topological_order(spec: &WorkflowSpec) -> Result<Vec<String>, String> {
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

pub fn workflow_has_model(spec: &WorkflowSpec) -> bool {
    spec.nodes
        .iter()
        .any(|node| matches!(node.config, WorkflowNodeConfig::Model { .. }))
}

fn allowed_transition(source: WorkflowNodeKind, target: WorkflowNodeKind) -> bool {
    matches!(
        (source, target),
        (WorkflowNodeKind::Table, WorkflowNodeKind::Model)
            | (WorkflowNodeKind::Table, WorkflowNodeKind::Query)
            | (WorkflowNodeKind::Model, WorkflowNodeKind::Query)
            | (WorkflowNodeKind::Query, WorkflowNodeKind::Result)
    )
}

fn issue(
    code: &str,
    node_id: Option<&str>,
    message: &str,
    recovery: &str,
) -> WorkflowValidationIssue {
    WorkflowValidationIssue {
        code: code.to_owned(),
        node_id: node_id.map(str::to_owned),
        message: message.to_owned(),
        recovery: recovery.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(validate_workflow_spec(&decoded).valid());
    }

    #[test]
    fn api_state_and_sse_update_round_trip_as_one_authoritative_shape() {
        let workflow = runnable_spec();
        let state = WorkflowStateResponse {
            validation: validate_workflow_spec(&workflow),
            snapshot: WorkflowRunSnapshot::idle_for(&workflow),
            workflow,
        };
        let json = serde_json::to_value(WorkflowUpdate {
            state: state.clone(),
        })
        .unwrap();
        assert_eq!(json["state"]["workflow"]["id"], DEFAULT_WORKFLOW_ID);
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
    fn runnable_table_query_result_is_valid() {
        let spec = runnable_spec();
        assert!(validate_workflow_spec(&spec).valid());
        assert_eq!(
            topological_order(&spec).unwrap(),
            ["frames", "query", "result"]
        );
    }

    #[test]
    fn graph_validation_rejects_duplicates_cycles_and_nonterminal_results() {
        let mut duplicate = runnable_spec();
        duplicate.nodes.push(duplicate.nodes[0].clone());
        assert!(
            validate_workflow_spec(&duplicate)
                .errors
                .iter()
                .any(|issue| issue.code == "duplicate_node_id")
        );

        let mut cycle = runnable_spec();
        cycle.edges.push(WorkflowEdgeSpec {
            source: "result".to_owned(),
            target: "query".to_owned(),
        });
        let report = validate_workflow_spec(&cycle);
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
            validate_workflow_spec(&unknown)
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
            validate_workflow_spec(&non_upstream)
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
            validate_workflow_spec(&malformed)
                .errors
                .iter()
                .any(|issue| issue.code == "malformed_placeholder")
        );
    }

    #[test]
    fn repeated_placeholder_is_valid_and_model_is_reserved_but_not_executable() {
        let mut repeated = runnable_spec();
        let WorkflowNodeConfig::Query { sql, .. } = &mut repeated.nodes[1].config else {
            unreachable!()
        };
        *sql = "SELECT * FROM {{node:frames}} a JOIN {{node:frames}} b ON a.frame_index = b.frame_index".to_owned();
        assert!(validate_workflow_spec(&repeated).valid());

        let mut model = runnable_spec();
        model.nodes.insert(
            1,
            WorkflowNodeSpec {
                id: "model".to_owned(),
                name: "Reserved model".to_owned(),
                config: WorkflowNodeConfig::Model {
                    model_id: "siglip-base".to_owned(),
                    configuration: BTreeMap::new(),
                    output_relation: Some("workflow.model_output".to_owned()),
                },
            },
        );
        assert!(workflow_has_model(&model));
        assert!(
            validate_workflow_spec(&model)
                .warnings
                .iter()
                .any(|issue| issue.code == "model_execution_unavailable")
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
        let report = validate_workflow_spec(&spec);
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
        let report = validate_workflow_spec(&missing);
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
        let report = validate_workflow_spec(&multiple);
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
        let report = validate_workflow_spec(&table_only);
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
        let report = validate_workflow_spec(&orphan);
        assert!(report.errors.iter().any(|issue| {
            issue.code == "orphan_terminal" && issue.node_id.as_deref() == Some("orphan")
        }));
    }
}
