//! Workflow meaning first; native execution diagnostics remain inspectable.
mod workflow_graph;
use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Edge, EdgeKind, Flow, Handle, HandleKind, LayoutDirection,
    LayoutOptions, MiniMap, Node, NodeViewCtx, Point, Rect, Side, Viewport, use_flow,
    use_flow_handle,
};
use spiral_robotics_common::workflow::plan::{DiagnosticNode, SpiralPlanReport};
use workflow_graph::{StageContent, StageOutput, WorkflowGraph, WorkflowStage};

#[component]
pub(super) fn WorkflowPlanDisplay(
    report: SpiralPlanReport,
    on_select: EventHandler<WorkflowGraphDetails>,
) -> Element {
    rsx! {
        div { class: "workflow-plan-display",
            match WorkflowGraph::from_plan(report.logical()) {
                Ok(graph) => rsx! { WorkflowPlanGraph { graph, report, on_select } },
                Err(error) => rsx! {
                    div { class: "workflow-panel-empty", "{error}" }
                },
            }
        }
    }
}

#[derive(Clone)]
struct PlanFlow {
    nodes: Vec<Node<WorkflowStage>>,
    edges: Vec<Edge>,
}
impl From<&WorkflowGraph> for PlanFlow {
    fn from(graph: &WorkflowGraph) -> Self {
        let mut nodes = graph
            .stages
            .iter()
            .map(|stage| {
                Node::with_data(
                    stage.id.clone(),
                    &stage.content.title,
                    Point::ZERO,
                    stage.clone(),
                )
                .size((256.0, 136.0))
                .sides(Side::Left, Side::Right)
                .draggable(false)
            })
            .collect::<Vec<_>>();
        let edges = graph
            .stages
            .iter()
            .flat_map(|stage| {
                stage.inputs.iter().enumerate().map(move |(index, input)| {
                    let edge = Edge::new(input.clone(), stage.id.clone())
                        .id(format!("{}:{index}", stage.id))
                        .target_handle(format!("input-{index}"))
                        .kind(EdgeKind::SmoothStep);
                    if stage.inputs.len() > 1 {
                        edge.label(format!("input {}", index + 1))
                    } else {
                        edge
                    }
                })
            })
            .collect::<Vec<_>>();
        let geometry = nodes
            .iter()
            .map(|node| dioxus_flow::LayoutNode {
                id: node.id.clone(),
                size: node.rect().size(),
            })
            .collect::<Vec<_>>();
        let connections = edges
            .iter()
            .map(|edge| (edge.source.clone(), edge.target.clone()))
            .collect::<Vec<_>>();
        let positions = dioxus_flow::compute_layout(
            &geometry,
            &connections,
            &LayoutOptions::default()
                .direction(LayoutDirection::LeftToRight)
                .gaps(28.0, 48.0),
        );
        for node in &mut nodes {
            if let Some(position) = positions.get(&node.id) {
                node.position = *position;
            }
        }
        Self { nodes, edges }
    }
}

/// The graph owns interpretation; its parent owns the active inspection.
#[derive(Clone, PartialEq)]
pub(super) struct WorkflowGraphDetails {
    selection: GraphSelection,
    report: SpiralPlanReport,
}
#[derive(Clone, PartialEq)]
enum GraphSelection {
    Stage(WorkflowStage),
    Edge(WorkflowEdgeDetails),
}

#[derive(Clone, PartialEq)]
struct WorkflowEdgeDetails {
    source: WorkflowStage,
    target_title: String,
}
impl WorkflowEdgeDetails {
    fn from_edge(edge: &Edge, nodes: &[Node<WorkflowStage>]) -> Option<Self> {
        let source = nodes.iter().find(|node| node.id == edge.source)?;
        let target = nodes.iter().find(|node| node.id == edge.target)?;
        Some(Self {
            source: source.data.clone(),
            target_title: target.data.content.title.clone(),
        })
    }
}

#[component]
fn WorkflowPlanGraph(
    graph: WorkflowGraph,
    report: SpiralPlanReport,
    on_select: EventHandler<WorkflowGraphDetails>,
) -> Element {
    let initial = use_hook(|| PlanFlow::from(&graph));
    let nodes = use_signal(|| initial.nodes);
    let edges = use_signal(|| initial.edges);
    let flow = use_flow_handle::<WorkflowStage>();
    let selected = use_memo(move || {
        let nodes = nodes.read();
        if let Some(edge) = edges.read().iter().find(|edge| edge.selected) {
            WorkflowEdgeDetails::from_edge(edge, &nodes).map(GraphSelection::Edge)
        } else {
            nodes
                .iter()
                .find(|node| node.selected)
                .map(|node| GraphSelection::Stage(node.data.clone()))
        }
    });
    use_effect(move || {
        if let Some(selection) = selected() {
            on_select.call(WorkflowGraphDetails {
                selection,
                report: report.clone(),
            });
        }
    });
    let root = graph.root.clone();
    rsx! {
        div { class: "workflow-plan-graph",
            div { class: "workflow-flow-wrap workflow-diagnostic-canvas",
                Flow { id: "workflow-native-plan", class: "workflow-flow",
                    nodes, edges, handle: flow, fit_view: false, fit_view_padding: 0.08,
                    min_zoom: 0.1, max_zoom: 1.5, nodes_draggable: false, delete_key: false,
                    is_valid_connection: move |_| false,
                    node_view: move |ctx: NodeViewCtx<WorkflowStage>| rsx! { PlanOperatorCard { ctx, root: root.clone() } },
                    Background { variant: BackgroundVariant::Dots, gap: 24.0, size: 0.6 }
                    PlanInitialViewport {}
                    if graph.stages.len() > 6 { MiniMap { width: 160.0, height: 52.0 } }
                }
            }
        }
    }
}

#[component]
pub(super) fn WorkflowGraphInspector(selection: Option<WorkflowGraphDetails>) -> Element {
    rsx! {
        section { class: "workflow-plan-inspector", "aria-label": "Workflow graph details",
            match selection {
                Some(WorkflowGraphDetails { selection: GraphSelection::Stage(stage), report }) => rsx! {
                    WorkflowStageInspector { stage, report }
                },
                Some(WorkflowGraphDetails { selection: GraphSelection::Edge(details), .. }) => rsx! {
                    h3 { "Connection schema" }
                    p { "{details.source.content.title} → {details.target_title}" }
                    match details.source.output() {
                        StageOutput::Relation(output) => rsx! {
                            h4 { "Columns" }
                            PlanOutputSchema { output: output.clone() }
                        },
                        StageOutput::Expression { expression, dtype } => rsx! {
                            h4 { "Value type" }
                            if let Some(dtype) = dtype {
                                pre { "{dtype}" }
                            } else {
                                p { "Type metadata is unavailable for this expression." }
                            }
                            details { summary { "Function expression" } pre { "{expression}" } }
                        },
                    }
                },
                None => rsx! {
                    div { class: "workflow-panel-empty",
                        "Select a stage to inspect its operations or an edge to inspect its schema."
                    }
                },
            }
        }
    }
}

#[component]
fn WorkflowStageInspector(stage: WorkflowStage, report: SpiralPlanReport) -> Element {
    rsx! {
        h3 { "{stage.content.title}" }
        p { "{stage.content.summary}" }
        if stage.content.function.is_none() {
            if let Some(expression) = &stage.content.expression { pre { "{expression}" } }
        }
        if stage.operators.len() > 1 {
            h4 { "What happens in this stage" }
            ol { class: "workflow-stage-steps",
                for node in &stage.operators {
                    { let content = StageContent::relational(node);
                      rsx! { li { strong { "{content.title}" } p { "{content.summary}" } } } }
                }
            }
        }
        if let Some(expression) = &stage.content.expression {
            if stage.content.function.is_some() {
                details { summary { "Function expression" } pre { "{expression}" } }
            }
        }
        for node in &stage.operators {
            details { class: "workflow-stage-native-details",
                summary { "{node.operator()} · #{node.id()}" }
                PlanOperatorDetails { node: node.clone() }
            }
        }
        details { class: "workflow-plan-report", summary { "Full plan JSON" }
            pre { "{serde_json::to_string_pretty(&report).unwrap_or_default()}" }
        }
    }
}

#[component]
fn PlanInitialViewport() -> Element {
    let core = use_flow();
    let mut previous_layout = use_signal(|| None::<(Rect, Rect)>);
    let mut automatic_viewport = use_signal(|| None::<Viewport>);
    let mut user_controlled = use_signal(|| false);
    use_effect(move || {
        let container = *core.container.read();
        let geometry = core.geoms.read();
        if container.width <= 0.0 || container.height <= 0.0 {
            return;
        }
        let Some(bounds) = geometry
            .iter()
            .map(|node| node.rect)
            .reduce(|a, b| a.union(&b))
        else {
            return;
        };
        let previous = *previous_layout.peek();
        if previous == Some((container, bounds)) {
            return;
        }
        let current = *core.viewport.peek();
        if automatic_viewport
            .peek()
            .is_some_and(|last| last != current)
        {
            user_controlled.set(true);
        }
        previous_layout.set(Some((container, bounds)));
        // Dock layout can settle after Flow mounts. Refit until the user moves
        // the viewport, then preserve its graph-space center and zoom on resize.
        if *user_controlled.peek() {
            if let Some((previous, _)) = previous {
                core.set_viewport(
                    Viewport::new(
                        current.x + (container.width - previous.width) / 2.0,
                        current.y + (container.height - previous.height) / 2.0,
                        current.zoom,
                    ),
                    0,
                );
            }
            return;
        }
        let safe = Rect::new(
            24.0,
            24.0,
            (container.width - 48.0).max(1.0),
            (container.height - 48.0).max(1.0),
        );
        let mut viewport = Viewport::fit(Some(bounds), safe, 0.75, 1.0);
        if viewport.zoom <= 0.75
            && let Some(source) = geometry.iter().min_by(|a, b| a.rect.x.total_cmp(&b.rect.x))
        {
            viewport = Viewport::new(
                24.0 - source.rect.x * viewport.zoom,
                container.height / 2.0 - source.rect.center().y * viewport.zoom,
                viewport.zoom,
            );
        }
        automatic_viewport.set(Some(viewport));
        core.set_viewport(viewport, 0);
    });
    rsx! {}
}

#[component]
fn PlanOperatorCard(ctx: NodeViewCtx<WorkflowStage>, root: String) -> Element {
    let stage = &ctx.node.data;
    rsx! {
        div { class: "workflow-plan-operator", class: if ctx.node.selected { "is-selected" },
            class: if stage.content.function.is_some() { "is-spql" },
            div { class: "workflow-plan-operator-heading", strong { title: "{stage.content.title}", "{stage.content.title}" } }
            p { class: "workflow-stage-summary", title: "{stage.content.summary}", "{stage.content.summary}" }
            small {
                if let Some(function) = &stage.content.function { "{function}" }
                else if stage.id == root { "Workflow output · {stage.operators.len()} SQL operations" }
                else if stage.operators.len() == 1 { "1 SQL operation" }
                else { "{stage.operators.len()} SQL operations" }
            }
            for (index, _) in stage.inputs.iter().enumerate() {
                Handle { id: format!("input-{index}"), kind: HandleKind::Target, position: Side::Left,
                    offset: (index + 1) as f64 / (stage.inputs.len() + 1) as f64 }
            }
            if stage.id != root { Handle { kind: HandleKind::Source, position: Side::Right } }
        }
    }
}

#[component]
fn PlanOperatorDetails(node: DiagnosticNode) -> Element {
    let content = StageContent::relational(&node);
    rsx! {
        h3 { "{content.title}" }
        p { "{content.summary}" }
        p { class: "workflow-plan-node-id", "Operator #{node.id()}" }
        h4 { "Output schema" }
        PlanOutputSchema { output: node.output().clone() }
        h4 { "Properties" }
        pre { "{serde_json::to_string_pretty(node.properties()).unwrap_or_default()}" }
        if let Some(details) = node.details() { h4 { "Operator details" } pre { "{details}" } }
        details { summary { "Full output metadata" } pre { "{serde_json::to_string_pretty(node.output()).unwrap_or_default()}" } }
        if let Some(execution) = node.execution() {
            details { summary { "Execution identity" } pre { "{serde_json::to_string_pretty(execution).unwrap_or_default()}" } }
        }
    }
}

#[component]
fn PlanOutputSchema(output: serde_json::Value) -> Element {
    let fields = output
        .pointer("/schema/fields")
        .and_then(serde_json::Value::as_array);
    rsx! {
        if let Some(fields) = fields {
            dl { class: "workflow-plan-schema",
                for field in fields {
                    { let name = field["name"].as_str().unwrap_or_default();
                      let dtype = field["dtype"].as_str().map(str::to_owned).unwrap_or_else(|| field["dtype"].to_string());
                      rsx! { div {
                          dt { "{name}" }
                          dd { code { "{dtype}" } if field["nullable"].as_bool() == Some(true) { span { "Nullable" } } }
                      } } }
                }
            }
            if fields.is_empty() { p { "No named output columns." } }
        } else { p { "Schema metadata is unavailable." } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spiral_robotics_common::workflow::plan::DiagnosticPlan;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn each_join_edge_inspects_its_own_source_schema() {
        let plan: DiagnosticPlan = serde_json::from_value(json!({"schema_version":1,"root":0,"nodes":[
            {"id":0,"operator":"Join","inputs":[1,2],"output":{},"properties":{"expressions":{"schema_version":1,"complete":true,"roots":[],"nodes":[]}}},
            {"id":1,"operator":"Source","inputs":[],"output":{"schema":{"fields":[{"name":"left_id","dtype":"i64","nullable":false}]}},"properties":{"expressions":{"schema_version":1,"complete":true,"roots":[],"nodes":[]}}},
            {"id":2,"operator":"Source","inputs":[],"output":{"schema":{"fields":[{"name":"right_id","dtype":"utf8?","nullable":true}]}},"properties":{"expressions":{"schema_version":1,"complete":true,"roots":[],"nodes":[]}}}
        ]})).unwrap();
        let flow = PlanFlow::from(&WorkflowGraph::from_plan(&plan).unwrap());
        for (edge, name) in flow.edges.iter().zip(["left_id", "right_id"]) {
            let details = WorkflowEdgeDetails::from_edge(edge, &flow.nodes).unwrap();
            let StageOutput::Relation(output) = details.source.output() else {
                panic!("source must carry rows")
            };
            assert_eq!(output["schema"]["fields"][0]["name"], name);
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn flow_preserves_shared_inputs_and_native_nodes_without_editing_the_plan() {
        let plan: DiagnosticPlan =
            serde_json::from_value(json!({"schema_version":1,"root":0,"nodes":[
                {"id":0,"operator":"Union","inputs":[1,1],"output":{},"properties":{"expressions":{"schema_version":1,"complete":true,"roots":[],"nodes":[]}}},
                {"id":1,"operator":"Values","inputs":[],"output":{},"properties":{"expressions":{"schema_version":1,"complete":true,"roots":[],"nodes":[]}}}
            ]}))
            .unwrap();
        let original = plan.clone();
        let flow = PlanFlow::from(&WorkflowGraph::from_plan(&plan).unwrap());
        assert_eq!(flow.nodes.len(), 2);
        assert_eq!(flow.edges.len(), 2);
        assert_ne!(flow.edges[0].id, flow.edges[1].id);
        assert_ne!(flow.edges[0].target_handle, flow.edges[1].target_handle);
        assert_eq!(flow.edges[0].source, "operator-1");
        assert_eq!(flow.edges[0].target, "operator-0");
        assert!(flow.nodes.iter().all(|node| !node.draggable));
        assert!(flow.nodes[1].position.x < flow.nodes[0].position.x);
        assert_eq!(plan, original);
    }
}
