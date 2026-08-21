//! Durable client state for the Workflow scaffold.
//!
//! The controller is hoisted in `StudioShell`, so canvas position, node
//! selection, and the active inspector tab survive route changes. Stage 1 is
//! deliberately local-only: no run or persistence behavior is implied here.

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;
use dioxus_flow::{Edge, EdgeKind, Node, Side};
use spiral_robotics_panels::PanelId;

pub(crate) const ASSISTANT_PANEL_ID: &str = "workflow-assistant";
pub(crate) const NODE_PANEL_ID: &str = "workflow-node";
pub(crate) const CANVAS_PANEL_ID: &str = "workflow-canvas";
pub(crate) const RESULTS_PANEL_ID: &str = "workflow-results";

pub(crate) const CAMERA_TABLE_ID: &str = "camera-frames";
pub(crate) const JOINT_TABLE_ID: &str = "joint-state";
pub(crate) const MODEL_ID: &str = "siglip";
pub(crate) const QUERY_ID: &str = "holding-while-still";
pub(crate) const RESULT_ID: &str = "matching-moments";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowNodeKind {
    Table,
    Model,
    Query,
    Result,
}

impl WorkflowNodeKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Table => "Table",
            Self::Model => "Model",
            Self::Query => "Query",
            Self::Result => "Result",
        }
    }

    pub(crate) const fn class_name(self) -> &'static str {
        match self {
            Self::Table => "workflow-node-kind-table",
            Self::Model => "workflow-node-kind-model",
            Self::Query => "workflow-node-kind-query",
            Self::Result => "workflow-node-kind-result",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowNodeState {
    ExampleRelation,
    NotRun,
    Waiting,
    Empty,
}

impl WorkflowNodeState {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::ExampleRelation => "Example relation",
            Self::NotRun => "Not run",
            Self::Waiting => "Waiting for model",
            Self::Empty => "No results yet",
        }
    }

    pub(crate) const fn class_name(self) -> &'static str {
        match self {
            Self::ExampleRelation => "workflow-node-state-example",
            Self::NotRun | Self::Waiting | Self::Empty => "workflow-node-state-idle",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WorkflowNodeData {
    pub kind: WorkflowNodeKind,
    pub fields: Vec<String>,
    pub inputs: Vec<String>,
    pub output_relation: Option<String>,
    pub sql: Option<String>,
    pub description: String,
    pub state: WorkflowNodeState,
}

impl WorkflowNodeData {
    fn table(fields: &[&str], rows: &str, description: &str) -> Self {
        Self {
            kind: WorkflowNodeKind::Table,
            fields: fields.iter().map(|value| (*value).to_owned()).collect(),
            inputs: Vec::new(),
            output_relation: None,
            sql: None,
            description: format!("{description} · {rows}"),
            state: WorkflowNodeState::ExampleRelation,
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

#[derive(Clone, Copy)]
pub(crate) struct WorkflowController {
    nodes: Signal<Vec<Node<WorkflowNodeData>>>,
    edges: Signal<Vec<Edge>>,
    ui: Signal<WorkflowUiState>,
}

pub(crate) fn use_workflow_controller() -> WorkflowController {
    debug_assert!(
        validate_relation_first_graph(&static_workflow_nodes(), &static_workflow_edges()).is_ok()
    );
    let controller = WorkflowController {
        nodes: use_signal(static_workflow_nodes),
        edges: use_signal(static_workflow_edges),
        ui: use_signal(WorkflowUiState::default),
    };
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

    pub(crate) fn selected_node(&self) -> Option<String> {
        self.ui.read().selected_node.clone()
    }

    pub(crate) fn selected_node_data(&self) -> Option<(String, WorkflowNodeData)> {
        let selected = self.selected_node()?;
        self.nodes
            .read()
            .iter()
            .find(|node| node.id == selected)
            .map(|node| (node.label.clone(), node.data.clone()))
    }

    pub(crate) fn active_panel(&self) -> Option<PanelId> {
        self.ui.read().active_panel.clone().map(PanelId::from)
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
}

pub(crate) fn static_workflow_nodes() -> Vec<Node<WorkflowNodeData>> {
    vec![
        Node::with_data(
            CAMERA_TABLE_ID,
            "pick_place.camera_frames",
            (24.0, 30.0),
            WorkflowNodeData::table(
                &["episode_id", "frame_index", "media"],
                "12,840 example rows",
                "Spiral relation",
            ),
        )
        .node_type("table")
        .sides(Side::Left, Side::Right),
        Node::with_data(
            JOINT_TABLE_ID,
            "pick_place.joint_state",
            (24.0, 208.0),
            WorkflowNodeData::table(
                &["episode_time_ns", "position"],
                "51,360 example rows",
                "Spiral relation",
            ),
        )
        .node_type("table")
        .sides(Side::Left, Side::Right),
        Node::with_data(
            MODEL_ID,
            "SigLIP base",
            (336.0, 88.0),
            WorkflowNodeData {
                kind: WorkflowNodeKind::Model,
                fields: vec!["cube".to_owned(), "gripper".to_owned()],
                inputs: vec!["pick_place.camera_frames".to_owned()],
                output_relation: Some("workflow.siglip_scores".to_owned()),
                sql: None,
                description: "Reads media and materializes scores in Spiral.".to_owned(),
                state: WorkflowNodeState::NotRun,
            },
        )
        .node_type("model")
        .sides(Side::Left, Side::Right),
        Node::with_data(
            QUERY_ID,
            "Holding while still",
            (650.0, 88.0),
            WorkflowNodeData {
                kind: WorkflowNodeKind::Query,
                fields: vec!["scores + joint state".to_owned()],
                inputs: vec![
                    "workflow.siglip_scores".to_owned(),
                    "pick_place.joint_state".to_owned(),
                ],
                output_relation: None,
                sql: Some(
                    "SELECT … FROM workflow.siglip_scores\nJOIN pick_place.joint_state …"
                        .to_owned(),
                ),
                description: "Read-only Spiral SQL joins both upstream relations.".to_owned(),
                state: WorkflowNodeState::Waiting,
            },
        )
        .node_type("query")
        .sides(Side::Left, Side::Right),
        Node::with_data(
            RESULT_ID,
            "Matching moments",
            (966.0, 88.0),
            WorkflowNodeData {
                kind: WorkflowNodeKind::Result,
                fields: vec![
                    "preview".to_owned(),
                    "episode".to_owned(),
                    "time".to_owned(),
                    "score".to_owned(),
                ],
                inputs: vec!["Holding while still".to_owned()],
                output_relation: None,
                sql: None,
                description: "Terminal media-aware query result view.".to_owned(),
                state: WorkflowNodeState::Empty,
            },
        )
        .node_type("result")
        .sides(Side::Left, Side::Right),
    ]
}

pub(crate) fn static_workflow_edges() -> Vec<Edge> {
    vec![
        Edge::new(CAMERA_TABLE_ID, MODEL_ID).kind(EdgeKind::SmoothStep),
        Edge::new(MODEL_ID, QUERY_ID)
            .target_handle("scores")
            .kind(EdgeKind::SmoothStep),
        Edge::new(JOINT_TABLE_ID, QUERY_ID)
            .target_handle("joint-state")
            .kind(EdgeKind::SmoothStep),
        Edge::new(QUERY_ID, RESULT_ID).kind(EdgeKind::SmoothStep),
    ]
}

pub(crate) fn validate_relation_first_graph(
    nodes: &[Node<WorkflowNodeData>],
    edges: &[Edge],
) -> Result<(), String> {
    let mut ids = HashSet::new();
    let kinds = nodes
        .iter()
        .map(|node| {
            if !ids.insert(node.id.as_str()) {
                return Err(format!("duplicate node id {}", node.id));
            }
            if node.data.kind == WorkflowNodeKind::Model && node.data.output_relation.is_none() {
                return Err(format!("model {} has no output relation", node.id));
            }
            Ok((node.id.as_str(), node.data.kind))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;

    let allowed = |source, target| {
        matches!(
            (source, target),
            (WorkflowNodeKind::Table, WorkflowNodeKind::Model)
                | (WorkflowNodeKind::Table, WorkflowNodeKind::Query)
                | (WorkflowNodeKind::Model, WorkflowNodeKind::Query)
                | (WorkflowNodeKind::Query, WorkflowNodeKind::Result)
        )
    };
    for edge in edges {
        let source = kinds
            .get(edge.source.as_str())
            .ok_or_else(|| format!("unknown source {}", edge.source))?;
        let target = kinds
            .get(edge.target.as_str())
            .ok_or_else(|| format!("unknown target {}", edge.target))?;
        if !allowed(*source, *target) {
            return Err(format!(
                "invalid relation flow {:?} -> {:?}",
                source, target
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_graph_is_relation_first_with_multiple_table_roots() {
        let nodes = static_workflow_nodes();
        let edges = static_workflow_edges();

        assert_eq!(nodes.len(), 5);
        assert_eq!(edges.len(), 4);
        assert_eq!(
            nodes
                .iter()
                .filter(|node| node.data.kind == WorkflowNodeKind::Table)
                .count(),
            2
        );
        assert!(validate_relation_first_graph(&nodes, &edges).is_ok());
        assert_eq!(
            nodes
                .iter()
                .find(|node| node.id == MODEL_ID)
                .and_then(|node| node.data.output_relation.as_deref()),
            Some("workflow.siglip_scores")
        );
    }

    #[test]
    fn selecting_a_node_opens_the_inspector_but_initial_state_keeps_assistant() {
        let initial = WorkflowUiState::default();
        assert_eq!(initial.active_panel, None);

        let selected =
            reduce_workflow_ui(&initial, WorkflowUiAction::SelectNode(MODEL_ID.to_owned()));
        assert_eq!(selected.selected_node.as_deref(), Some(MODEL_ID));
        assert_eq!(selected.active_panel.as_deref(), Some(NODE_PANEL_ID));
    }
}
