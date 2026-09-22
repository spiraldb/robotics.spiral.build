//! Workflow meaning first; native execution diagnostics remain inspectable.
//!
//! The graph reads left to right like a pipeline: sources at the left, the
//! workflow output at the right. Each stage carries a kind glyph and identity
//! tint so the shape of a query is legible before any label is read. Details
//! are on demand: selecting a stage floats one sheet over the canvas that
//! answers what it does, its facts, the steps it folds, and what it hands on,
//! with the native operators behind one disclosure.
mod workflow_graph;
use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Edge, EdgeKind, Flow, Handle, HandleKind, MiniMap, Node,
    NodeViewCtx, Point, Rect, Side, use_flow, use_flow_handle,
};
use spiral_robotics_common::workflow::plan::{DiagnosticNode, SpiralPlanReport};
use workflow_graph::{
    StageContent, StageKind, StageOutput, SummaryKind, WorkflowGraph, WorkflowStage,
};

use crate::components::flow_canvas::{
    CanvasControls, CardGraph, FlowFit, FlowMargins, PopoverPlacement, ReadableViewport,
};
use crate::components::icons::{
    DataIcon, JoinStageIcon, MediaStageIcon, ModelStageIcon, QueryNodeIcon, ResultNodeIcon,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, CloseIcon, Disclosure, EmptyState, Fact, FactList,
    NoticeTone,
};

const STAGE_WIDTH: f64 = 224.0;
const STAGE_HEIGHT: f64 = 92.0;
const MIN_READABLE_ZOOM: f64 = 0.7;

#[component]
pub(super) fn WorkflowPlanDisplay(
    report: SpiralPlanReport,
    /// A submitted query is executing: edges carry moving dashes while it does.
    active: bool,
) -> Element {
    rsx! {
        div { class: "workflow-plan-display",
            match WorkflowGraph::from_plan(report.logical()) {
                Ok(graph) => rsx! { WorkflowPlanGraph { graph, active } },
                Err(error) => rsx! {
                    EmptyState { tone: NoticeTone::Danger, title: "The graph needs native expression metadata",
                        p { "{error}" }
                    }
                },
            }
        }
    }
}

/// A stage plus its reading order, so entrance motion can follow the data.
#[derive(Clone, PartialEq)]
struct StageNode {
    stage: WorkflowStage,
    order: usize,
}

#[derive(Clone)]
struct PlanFlow {
    nodes: Vec<Node<StageNode>>,
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
                    StageNode {
                        stage: stage.clone(),
                        order: 0,
                    },
                )
                .size((STAGE_WIDTH, STAGE_HEIGHT))
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
                    match stage.input_label(index) {
                        Some(label) => edge.label(label),
                        None => edge,
                    }
                })
            })
            .collect::<Vec<_>>();
        let order = CardGraph::left_to_right(28.0, 56.0).rank_layout(&mut nodes, &edges);
        for (node, rank) in nodes.iter_mut().zip(order) {
            node.data.order = rank;
        }
        Self { nodes, edges }
    }
}

trait StageInputLabels {
    fn input_label(&self, index: usize) -> Option<String>;
}

impl StageInputLabels for WorkflowStage {
    /// A join's two inputs are its left and right sides. A set operation's
    /// inputs are peers and carry no label; other multi-input stages number
    /// theirs. A single input needs no label.
    fn input_label(&self, index: usize) -> Option<String> {
        if self.inputs.len() < 2 || self.is_set_operation() {
            return None;
        }
        if self.is_join() && self.inputs.len() == 2 {
            return Some(if index == 0 { "left" } else { "right" }.to_owned());
        }
        Some(format!("input {}", index + 1))
    }
}

/// What the canvas has selected, for the popover to explain.
#[derive(Clone, PartialEq)]
enum GraphSelection {
    Stage {
        stage: WorkflowStage,
        /// The stages feeding this one, in native input order.
        inputs: Vec<StageInput>,
    },
    Edge(WorkflowEdgeDetails),
}

/// What an input stage is called and which columns it hands on, so the
/// inspector can say which columns a stage adds.
#[derive(Clone, PartialEq)]
struct StageInput {
    title: String,
    columns: Vec<String>,
}

#[derive(Clone, PartialEq)]
struct WorkflowEdgeDetails {
    source: WorkflowStage,
    target: String,
    target_title: String,
}
impl WorkflowEdgeDetails {
    fn from_edge(edge: &Edge, nodes: &[Node<StageNode>]) -> Option<Self> {
        let source = nodes.iter().find(|node| node.id == edge.source)?;
        let target = nodes.iter().find(|node| node.id == edge.target)?;
        Some(Self {
            source: source.data.stage.clone(),
            target: target.id.clone(),
            target_title: target.data.stage.content.title.clone(),
        })
    }
}

impl GraphSelection {
    /// The stage ids the selection is drawn from, for anchoring its sheet.
    fn anchors(&self) -> Vec<&str> {
        match self {
            Self::Stage { stage, .. } => vec![stage.id.as_str()],
            Self::Edge(details) => vec![details.source.id.as_str(), details.target.as_str()],
        }
    }
}

/// What the canvas itself has selected: a card, a connection, or nothing.
#[derive(Clone, Copy)]
struct CanvasSelection {
    nodes: Signal<Vec<Node<StageNode>>>,
    edges: Signal<Vec<Edge>>,
}

impl CanvasSelection {
    /// Closing the sheet is deselecting: the canvas and the sheet always agree
    /// on what is selected.
    fn clear(mut self) {
        self.nodes.with_mut(|nodes| {
            for node in nodes.iter_mut() {
                node.selected = false;
            }
        });
        self.edges.with_mut(|edges| {
            for edge in edges.iter_mut() {
                edge.selected = false;
            }
        });
    }
}

#[component]
fn WorkflowPlanGraph(graph: WorkflowGraph, active: bool) -> Element {
    let initial = use_hook(|| PlanFlow::from(&graph));
    let nodes = use_signal(|| initial.nodes);
    let mut edges = use_signal(|| initial.edges);
    let selection = CanvasSelection { nodes, edges };
    let flow = use_flow_handle::<StageNode>();
    let overflowing = use_signal(|| false);
    use_effect(use_reactive((&active,), move |(active,)| {
        if edges.peek().iter().any(|edge| edge.animated != active) {
            edges.with_mut(|edges| {
                for edge in edges.iter_mut() {
                    edge.animated = active;
                }
            });
        }
    }));
    let selected = use_memo(move || {
        let nodes = nodes.read();
        if let Some(edge) = edges.read().iter().find(|edge| edge.selected) {
            WorkflowEdgeDetails::from_edge(edge, &nodes).map(GraphSelection::Edge)
        } else {
            nodes.iter().find(|node| node.selected).map(|node| {
                let inputs = node
                    .data
                    .stage
                    .inputs
                    .iter()
                    .filter_map(|input| nodes.iter().find(|candidate| &candidate.id == input))
                    .map(|input| StageInput {
                        title: input.data.stage.content.title.clone(),
                        columns: input.data.stage.output_columns(),
                    })
                    .collect();
                GraphSelection::Stage {
                    stage: node.data.stage.clone(),
                    inputs,
                }
            })
        }
    });
    let root = graph.root.clone();
    let card_root = root.clone();
    let stage_count = graph.stages.len();
    // A selection made off the pane — from the sheet's own links or a resize —
    // brings its card back into view.
    let revealed = match selected() {
        Some(GraphSelection::Stage { stage, .. }) => stage.id.clone(),
        Some(GraphSelection::Edge(details)) => details.source.id.clone(),
        None => String::new(),
    };
    rsx! {
        div { class: "workflow-plan-graph",
            div { class: "workflow-flow-wrap",
                // Escape deselects from wherever focus sits on the canvas:
                // the pane, the selected card, or the sheet itself.
                onkeydown: move |event| {
                    if event.key() == Key::Escape && selected.peek().is_some() {
                        selection.clear();
                    }
                },
                Flow { id: "workflow-native-plan", class: "workflow-flow",
                    nodes, edges, handle: flow, fit_view: false, fit_view_padding: 0.1,
                    min_zoom: 0.1, max_zoom: 1.5, nodes_draggable: false, delete_key: false,
                    is_valid_connection: move |_| false,
                    node_view: move |ctx: NodeViewCtx<StageNode>| rsx! { PlanStageCard { ctx, root: card_root.clone() } },
                    Background { variant: BackgroundVariant::Dots, gap: 22.0, size: 0.7 }
                    // The controls own the bottom 40px; the rest of the pane is canvas.
                    FlowFit {
                        fit: ReadableViewport::new(FlowMargins::new(24.0, 20.0, 48.0), MIN_READABLE_ZOOM),
                        overflowing,
                        reveal: revealed,
                    }
                    CanvasControls {
                        label: "Graph view of {stage_count} stages",
                        fit_label: "Fit the whole graph",
                    }
                    if overflowing() { MiniMap { width: 112.0, height: 64.0 } }
                    if let Some(shown) = selected() {
                        StagePopover { selection: shown, root: root.clone(), on_close: move |_| selection.clear() }
                    }
                }
            }
        }
    }
}

/// Prose that may hold a path. Breaks are offered after each slash so a
/// topic never splits inside a word.
#[component]
fn PathText(text: String) -> Element {
    let segments = text.split('/').collect::<Vec<_>>();
    rsx! {
        for (index, segment) in segments.iter().enumerate() {
            if index > 0 { "/" wbr {} }
            "{segment}"
        }
    }
}

#[component]
fn StageKindGlyph(kind: StageKind) -> Element {
    match kind {
        StageKind::Source => rsx! { DataIcon {} },
        StageKind::Media => rsx! { MediaStageIcon {} },
        StageKind::Model => rsx! { ModelStageIcon {} },
        StageKind::Shape => rsx! { QueryNodeIcon {} },
        StageKind::Join => rsx! { JoinStageIcon {} },
        StageKind::Output => rsx! { ResultNodeIcon {} },
    }
}

/// A summary or fact value. Expressions retain their exact native text.
#[component]
fn StageText(text: String, kind: SummaryKind) -> Element {
    match kind {
        SummaryKind::Expression => rsx! { code { "{text}" } },
        SummaryKind::Prose => rsx! { PathText { text } },
    }
}

/// Details on demand: one paper sheet beside the selected stage or
/// connection, following it through pan and zoom. Pointer and wheel events
/// stay inside it so reading the sheet never pans or zooms the graph beneath;
/// Escape reaches the canvas wrap above and deselects.
#[component]
fn StagePopover(selection: GraphSelection, root: String, on_close: EventHandler<()>) -> Element {
    let core = use_flow();
    let viewport = *core.viewport.read();
    let container = *core.container.read();
    let anchor = {
        let geometry = core.geoms.read();
        selection
            .anchors()
            .into_iter()
            .filter_map(|id| geometry.iter().find(|node| node.id == id))
            .map(|node| Rect {
                x: node.rect.x * viewport.zoom + viewport.x,
                y: node.rect.y * viewport.zoom + viewport.y,
                width: node.rect.width * viewport.zoom,
                height: node.rect.height * viewport.zoom,
            })
            .reduce(|a, b| a.union(&b))
            .unwrap_or(Rect::ZERO)
    };
    let placement = PopoverPlacement::of(anchor, container);
    rsx! {
        section {
            class: "workflow-stage-popover",
            style: "{placement}",
            "aria-label": "Stage details",
            onpointerdown: move |event| event.stop_propagation(),
            onwheel: move |event| event.stop_propagation(),
            match selection {
                GraphSelection::Stage { stage, inputs } => rsx! {
                    StageDetails { key: "{stage.id}", stage, inputs, root, on_close }
                },
                GraphSelection::Edge(details) => rsx! {
                    EdgeDetails { key: "{details.source.id}:{details.target_title}", details, root, on_close }
                },
            }
        }
    }
}

/// What a stage does, its facts, the steps it folds, and the columns it
/// hands on. Its inputs are the edges already drawn into it.
#[component]
fn StageDetails(
    stage: WorkflowStage,
    inputs: Vec<StageInput>,
    root: String,
    on_close: EventHandler<()>,
) -> Element {
    let kind = stage.kind(&root);
    let footer = stage.footer(&root);
    // The heading and summary already say it once.
    let facts = stage
        .facts()
        .into_iter()
        .filter(|fact| fact.value != stage.content.summary && fact.value != footer)
        .collect::<Vec<_>>();
    let produced = stage.output_columns();
    let upstream = inputs
        .iter()
        .flat_map(|input| input.columns.iter().cloned())
        .collect::<Vec<_>>();
    let added = if upstream.is_empty() {
        Vec::new()
    } else {
        produced
            .iter()
            .filter(|column| !upstream.contains(column))
            .cloned()
            .collect::<Vec<_>>()
    };
    rsx! {
        div { class: "workflow-inspector-body",
            header { class: "workflow-inspector-heading",
                span { class: "workflow-stage-mark workflow-stage-{kind.class_name()}", "aria-hidden": "true",
                    StageKindGlyph { kind }
                }
                div {
                    h3 { "{stage.content.title}" }
                    small { "{kind.label()} · {footer}" }
                }
                Button { purpose: ActionPurpose::Close, variant: ButtonVariant::Icon,
                    aria_label: "Close stage details", title: "Close · Esc",
                    onclick: move |_| on_close.call(()), CloseIcon {} }
            }
            if !stage.content.summary.is_empty() {
                p { class: "workflow-inspector-summary",
                    StageText { text: stage.content.summary.clone(), kind: stage.content.summary_kind }
                }
            }
            if let Some(expression) = &stage.content.expression {
                if stage.content.function.is_none() && !stage.reads_source() {
                    div { class: "workflow-inspector-rule",
                        code { "{expression}" }
                    }
                }
            }
            if !facts.is_empty() {
                FactList { class: "workflow-facts",
                    for (index, fact) in facts.iter().enumerate() {
                        Fact { key: "{index}", label: "{fact.label}",
                            StageText { text: fact.value.clone(), kind: fact.kind }
                        }
                    }
                }
            }
            if stage.operators.len() > 1 {
                h4 { "Steps" }
                ol { class: "workflow-stage-steps",
                    for node in &stage.operators {
                        { let content = StageContent::relational(node);
                          rsx! { li { strong { "{content.title}" }
                              if !content.summary.is_empty() { p { StageText { text: content.summary.clone(), kind: content.summary_kind } } } } } }
                    }
                }
            }
            // A call's produced type is already among its facts.
            if let StageOutput::Relation(output) = stage.output() {
                h4 {
                    if produced.is_empty() { "Columns" }
                    else if added.is_empty() { "Columns · {produced.len()}" }
                    else if added.len() == 1 { "Columns · {produced.len()} · 1 new" }
                    else { "Columns · {produced.len()} · {added.len()} new" }
                }
                StageColumns { output: output.clone(), added: added.clone() }
            }
            Disclosure { class: "workflow-inspector-disclosure workflow-inspector-native",
                summary: rsx! {
                    if stage.operators.len() == 1 { "Native operator" } else { "Native operators · {stage.operators.len()}" }
                },
                for node in &stage.operators {
                    PlanOperatorDetails { key: "{node.id()}", node: node.clone() }
                }
            }
        }
    }
}

/// What travels along a selected connection.
#[component]
fn EdgeDetails(details: WorkflowEdgeDetails, root: String, on_close: EventHandler<()>) -> Element {
    let kind = details.source.kind(&root);
    let carried = details.source.output_columns();
    rsx! {
        div { class: "workflow-inspector-body",
            header { class: "workflow-inspector-heading",
                span { class: "workflow-stage-mark workflow-stage-{kind.class_name()}", "aria-hidden": "true",
                    StageKindGlyph { kind }
                }
                div {
                    h3 { "{details.source.content.title} → {details.target_title}" }
                    small { "Connection" }
                }
                Button { purpose: ActionPurpose::Close, variant: ButtonVariant::Icon,
                    aria_label: "Close connection details", title: "Close · Esc",
                    onclick: move |_| on_close.call(()), CloseIcon {} }
            }
            match details.source.output() {
                StageOutput::Relation(output) => rsx! {
                    h4 {
                        if carried.is_empty() { "Columns carried" } else { "Columns carried · {carried.len()}" }
                    }
                    StageColumns { output: output.clone(), added: Vec::new() }
                },
                StageOutput::Expression { expression, dtype } => rsx! {
                    FactList { class: "workflow-facts",
                        if let Some(dtype) = dtype {
                            Fact { label: "Type", code { "{dtype}" } }
                        }
                        Fact { label: "Call", code { "{expression}" } }
                    }
                },
            }
        }
    }
}

/// The columns a stage hands on, as names. The type rides in the tooltip;
/// the native disclosure keeps the full schema.
#[component]
fn StageColumns(output: serde_json::Value, added: Vec<String>) -> Element {
    let fields = output
        .pointer("/schema/fields")
        .and_then(serde_json::Value::as_array);
    rsx! {
        match fields {
            Some(fields) if !fields.is_empty() => rsx! {
                ul { class: "workflow-columns",
                    for field in fields {
                        { let name = field["name"].as_str().unwrap_or_default();
                          let dtype = field["dtype"].as_str().map(str::to_owned).unwrap_or_else(|| field["dtype"].to_string());
                          let title = if field["nullable"].as_bool() == Some(true) { format!("{dtype} · nullable") } else { dtype };
                          let is_new = added.iter().any(|column| column == name);
                          rsx! { li { key: "{name}", code { class: if is_new { "is-new" }, title: "{title}", "{name}" } } } }
                    }
                }
            },
            Some(_) => rsx! { p { class: "workflow-inspector-note", "No named output columns." } },
            None => rsx! { p { class: "workflow-inspector-note", "Schema metadata is unavailable." } },
        }
    }
}

#[component]
fn PlanStageCard(ctx: NodeViewCtx<StageNode>, root: String) -> Element {
    let stage = &ctx.node.data.stage;
    let kind = stage.kind(&root);
    let footer = stage.footer(&root);
    rsx! {
        div { class: "workflow-stage-card workflow-stage-{kind.class_name()}",
            class: if ctx.node.selected { "is-selected" },
            style: "--stage-order: {ctx.node.data.order}",
            // No tooltips restating the card: the sheet shows every line in
            // full when the card is selected.
            div { class: "workflow-stage-heading",
                span { class: "workflow-stage-mark", "aria-hidden": "true", StageKindGlyph { kind } }
                strong { class: "workflow-stage-title", "{stage.content.title}" }
            }
            p { class: "workflow-stage-summary",
                if stage.content.summary.is_empty() { "{footer}" }
                else { StageText { text: stage.content.summary.clone(), kind: stage.content.summary_kind } }
            }
            footer { class: "workflow-stage-footer",
                span { class: "workflow-stage-kind", "{kind.label()}" }
                if stage.content.function.is_some() {
                    code { "{footer}" }
                } else {
                    span { "{footer}" }
                }
                if stage.id == root && kind != StageKind::Output {
                    span { class: "workflow-stage-output-flag", "Output" }
                }
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
        div { class: "workflow-native-operator",
            header {
                strong { "{content.title}" }
                code { "{node.operator()} · #{node.id()}" }
            }
            if !content.summary.is_empty() {
                p { StageText { text: content.summary.clone(), kind: content.summary_kind } }
            }
            h5 { "Output schema" }
            PlanOutputSchema { output: node.output().clone(), added: Vec::new() }
            h5 { "Properties" }
            pre { "{serde_json::to_string_pretty(node.properties()).unwrap_or_default()}" }
            if let Some(details) = node.details() { h5 { "Operator details" } pre { "{details}" } }
            Disclosure { class: "workflow-inspector-disclosure", summary: rsx! { "Full output metadata" },
                pre { "{serde_json::to_string_pretty(node.output()).unwrap_or_default()}" }
            }
            if let Some(execution) = node.execution() {
                Disclosure { class: "workflow-inspector-disclosure", summary: rsx! { "Execution identity" },
                    pre { "{serde_json::to_string_pretty(execution).unwrap_or_default()}" }
                }
            }
        }
    }
}

#[component]
fn PlanOutputSchema(output: serde_json::Value, added: Vec<String>) -> Element {
    let fields = output
        .pointer("/schema/fields")
        .and_then(serde_json::Value::as_array);
    rsx! {
        if let Some(fields) = fields {
            if fields.is_empty() {
                p { class: "workflow-inspector-note", "No named output columns." }
            } else {
                table { class: "workflow-schema",
                    thead { tr { th { scope: "col", "Column" } th { scope: "col", "Type" } } }
                    tbody {
                        for field in fields {
                            { let name = field["name"].as_str().unwrap_or_default();
                              let dtype = field["dtype"].as_str().map(str::to_owned).unwrap_or_else(|| field["dtype"].to_string());
                              let nullable = field["nullable"].as_bool() == Some(true);
                              let is_new = added.iter().any(|column| column == name);
                              rsx! { tr { key: "{name}", class: if is_new { "is-new" },
                                  td {
                                      code { "{name}" }
                                      if is_new { span { class: "workflow-schema-new", "new" } }
                                  }
                                  td { span { class: "workflow-schema-type", "{dtype}" } if nullable { span { class: "workflow-schema-nullable", "nullable" } } }
                              } } }
                        }
                    }
                }
            }
        } else {
            p { class: "workflow-inspector-note", "Schema metadata is unavailable." }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spiral_robotics_common::workflow::plan::DiagnosticPlan;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn each_join_edge_inspects_its_own_source_schema_and_names_its_side() {
        let plan: DiagnosticPlan = serde_json::from_value(json!({"schema_version":1,"root":0,"nodes":[
            {"id":0,"operator":"Join","inputs":[1,2],"output":{},"properties":{"expressions":{"schema_version":1,"complete":true,"roots":[],"nodes":[]}}},
            {"id":1,"operator":"Source","inputs":[],"output":{"schema":{"fields":[{"name":"left_id","dtype":"i64","nullable":false}]}},"properties":{"expressions":{"schema_version":1,"complete":true,"roots":[],"nodes":[]}}},
            {"id":2,"operator":"Source","inputs":[],"output":{"schema":{"fields":[{"name":"right_id","dtype":"utf8?","nullable":true}]}},"properties":{"expressions":{"schema_version":1,"complete":true,"roots":[],"nodes":[]}}}
        ]})).unwrap();
        let flow = PlanFlow::from(&WorkflowGraph::from_plan(&plan).unwrap());
        for (edge, (name, side)) in flow
            .edges
            .iter()
            .zip([("left_id", "left"), ("right_id", "right")])
        {
            let details = WorkflowEdgeDetails::from_edge(edge, &flow.nodes).unwrap();
            let StageOutput::Relation(output) = details.source.output() else {
                panic!("source must carry rows")
            };
            assert_eq!(output["schema"]["fields"][0]["name"], name);
            assert_eq!(edge.label.as_deref(), Some(side));
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn flow_reads_left_to_right_and_orders_stages_by_position() {
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
        assert_eq!(flow.edges[0].label, None);
        assert!(flow.nodes.iter().all(|node| !node.draggable));
        let source = flow
            .nodes
            .iter()
            .find(|node| node.id == "operator-1")
            .unwrap();
        let output = flow
            .nodes
            .iter()
            .find(|node| node.id == "operator-0")
            .unwrap();
        assert!(source.position.x < output.position.x);
        assert_eq!(source.data.order, 0);
        assert_eq!(output.data.order, 1);
        assert_eq!(plan, original);
    }
}
