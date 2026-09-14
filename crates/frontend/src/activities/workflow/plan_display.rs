//! Workflow meaning first; native execution diagnostics remain inspectable.
mod workflow_graph;
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Edge, EdgeKind, Flow, Handle, HandleKind, LayoutDirection,
    LayoutOptions, MiniMap, Node, NodeViewCtx, Point, Rect, Side, Viewport, use_flow,
    use_flow_handle,
};
use spiral_robotics_common::workflow::plan::{DiagnosticNode, DiagnosticPlan, SpiralPlanReport};
use workflow_graph::{WorkflowGraph, WorkflowStage};

#[derive(Clone, Copy, PartialEq)]
enum PlanStage {
    Workflow,
    Logical,
    Optimized,
    Physical,
}
impl PlanStage {
    fn label(self) -> &'static str {
        match self {
            Self::Workflow => "Workflow",
            Self::Logical => "Logical",
            Self::Optimized => "Optimized logical",
            Self::Physical => "Physical",
        }
    }
    fn graph(self, report: &SpiralPlanReport) -> Option<&DiagnosticPlan> {
        match self {
            Self::Workflow | Self::Logical => Some(report.logical()),
            Self::Optimized => Some(report.optimized()),
            Self::Physical => report.physical(),
        }
    }
}

#[component]
pub(super) fn NativePlanDisplay(report: SpiralPlanReport) -> Element {
    let mut stage = use_signal(|| PlanStage::Workflow);
    let graph = stage().graph(&report).cloned();
    let stage_controls = rsx! {
        select { id: "workflow-plan-stage", "aria-label": "Plan stage", value: stage().label(),
            onchange: move |event| stage.set(match event.value().as_str() {
                "Physical" => PlanStage::Physical, "Optimized logical" => PlanStage::Optimized, "Logical" => PlanStage::Logical, _ => PlanStage::Workflow,
            }),
            for option in [PlanStage::Workflow, PlanStage::Logical, PlanStage::Optimized, PlanStage::Physical] {
                option { value: option.label(), "{option.label()}" }
            }
        }
    };
    rsx! {
        div { class: "workflow-plan-display",
            if let Some(graph) = graph {
                if stage() == PlanStage::Workflow {
                    match WorkflowGraph::from_plan(&graph) {
                        Ok(graph) => rsx! { NativePlanGraph { key: "{stage().label()}", graph, stage_controls, report } },
                        Err(error) => rsx! {
                            div { class: "workflow-plan-navigation", {stage_controls} }
                            div { class: "workflow-panel-empty", "{error}" }
                        },
                    }
                } else {
                    NativePlanGraph { key: "{stage().label()}", graph: WorkflowGraph::native(&graph), stage_controls, report }
                }
            } else {
                div { class: "workflow-plan-navigation", {stage_controls} }
                div { class: "workflow-panel-empty", "No physical plan is available. Inspect the logical plan and planning error." }
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
                let mut node = Node::with_data(
                    stage.id.clone(),
                    &stage.content.title,
                    Point::ZERO,
                    stage.clone(),
                )
                .size((224.0, 112.0))
                .sides(Side::Left, Side::Right)
                .draggable(false);
                node.selected = stage.id == graph.root;
                node
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

#[component]
fn NativePlanGraph(
    graph: WorkflowGraph,
    stage_controls: Element,
    report: SpiralPlanReport,
) -> Element {
    let initial = use_hook(|| PlanFlow::from(&graph));
    let mut details_open = use_signal(|| false);
    let mut nodes = use_signal(|| initial.nodes);
    let edges = use_signal(|| initial.edges);
    let flow = use_flow_handle::<WorkflowStage>();
    let selected = nodes
        .read()
        .iter()
        .find(|node| node.selected)
        .map(|node| node.data.clone());
    let root = graph.root.clone();
    rsx! {
        div { class: "workflow-plan-graph",
            div { class: "workflow-plan-navigation",
                {stage_controls}
                select { id: "workflow-plan-node", "aria-label": "Find a stage", value: selected.as_ref().map(|node| node.id.clone()).unwrap_or_default(),
                    onchange: move |event| {
                        nodes.with_mut(|nodes| {
                            for node in nodes { node.selected = node.id == event.value(); }
                        });
                        if let Some(node) = nodes.peek().iter().find(|node| node.selected)
                            && let Some(core) = flow.core() {
                                let zoom = core.viewport.peek().zoom;
                                core.zoom_by(0.9 / zoom, None, 0);
                                core.center_on(node.rect().center(), 0);
                            }
                    },
                    option { value: "", disabled: true, "Select a stage" }
                    for node in &graph.stages { option { value: "{node.id}", "{node.content.title}" } }
                }
                span { class: "workflow-plan-count", "{graph.stages.len()} stages" }
                Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Link,
                    aria_pressed: details_open(), disabled: selected.is_none(),
                    onclick: move |_| details_open.toggle(), "Details" }
                Action { purpose: ActionPurpose::Fit, variant: ActionVariant::Link,
                    title: "Show the complete graph", onclick: move |_| flow.fit_view(0), "Overview" }
                Action { purpose: ActionPurpose::Fit, variant: ActionVariant::Link,
                    title: "Zoom out", onclick: move |_| flow.zoom_out(0), "−" }
                Action { purpose: ActionPurpose::Fit, variant: ActionVariant::Link,
                    title: "Zoom in", onclick: move |_| flow.zoom_in(0), "+" }
            }
            div { class: "workflow-plan-body",
                div { class: "workflow-flow-wrap workflow-diagnostic-canvas",
                    Flow { id: "workflow-native-plan", class: "workflow-flow",
                        nodes, edges, handle: flow, fit_view: false, fit_view_padding: 0.08,
                        min_zoom: 0.1, max_zoom: 1.5, nodes_draggable: false, delete_key: false,
                        is_valid_connection: move |_| false,
                        on_node_click: move |_| details_open.set(true),
                        node_view: move |ctx: NodeViewCtx<WorkflowStage>| rsx! { PlanOperatorCard { ctx, root: root.clone() } },
                        Background { variant: BackgroundVariant::Dots, gap: 24.0, size: 0.6 }
                        PlanInitialViewport {}
                        if graph.stages.len() > 6 { MiniMap { width: 160.0, height: 52.0 } }
                    }
                }
                if details_open() {
                    aside { class: "workflow-plan-inspector", "aria-label": "Workflow stage details",
                        header { class: "workflow-inspector-actions",
                            strong { "Stage details" }
                            Action { purpose: ActionPurpose::Close, variant: ActionVariant::Link,
                                onclick: move |_| details_open.set(false), "Close" }
                        }
                        if let Some(stage) = &selected {
                            h3 { "{stage.content.title}" }
                            p { "{stage.content.summary}" }
                            if let Some(expression) = &stage.content.expression { pre { "{expression}" } }
                            for node in &stage.operators {
                                details { class: "workflow-stage-native-details",
                                    summary { "{node.operator()} · #{node.id()}" }
                                    PlanOperatorDetails { node: node.clone() }
                                }
                            }
                        }
                        else { p { "Select a stage to inspect its operations and outputs." } }
                        details { class: "workflow-plan-report", summary { "Full plan JSON" }
                            pre { "{serde_json::to_string_pretty(&report).unwrap_or_default()}" }
                        }
                    }
                }
            }
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
            div { class: "workflow-plan-operator-heading", strong { "{stage.content.title}" } }
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
    let fields = node
        .output()
        .pointer("/schema/fields")
        .and_then(serde_json::Value::as_array);
    rsx! {
        h3 { "{node.operator()}" }
        p { class: "workflow-plan-node-id", "Operator #{node.id()}" }
        h4 { "Output schema" }
        if let Some(fields) = fields.filter(|fields| !fields.is_empty()) {
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
        } else { p { "No named output columns." } }
        h4 { "Properties" }
        pre { "{serde_json::to_string_pretty(node.properties()).unwrap_or_default()}" }
        if let Some(details) = node.details() { h4 { "Operator details" } pre { "{details}" } }
        details { summary { "Full output metadata" } pre { "{serde_json::to_string_pretty(node.output()).unwrap_or_default()}" } }
        if let Some(execution) = node.execution() {
            details { summary { "Execution identity" } pre { "{serde_json::to_string_pretty(execution).unwrap_or_default()}" } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn flow_preserves_shared_inputs_and_native_nodes_without_editing_the_plan() {
        let plan: DiagnosticPlan =
            serde_json::from_value(json!({"schema_version":1,"root":0,"nodes":[
                {"id":0,"operator":"Union","inputs":[1,1],"output":{},"properties":{}},
                {"id":1,"operator":"Values","inputs":[],"output":{},"properties":{}}
            ]}))
            .unwrap();
        let original = plan.clone();
        let flow = PlanFlow::from(&WorkflowGraph::native(&plan));
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
