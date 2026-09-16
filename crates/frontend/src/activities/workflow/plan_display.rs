//! Workflow meaning first; native execution diagnostics remain inspectable.
//!
//! The graph reads left to right like a pipeline: sources at the left, the
//! workflow output at the right. Each stage carries a kind glyph and identity
//! tint so the shape of a query is legible before any label is read. The
//! inspector answers the questions a selected stage raises, in order: what it
//! does, what it reads or requires, what it hands on, and only then how the
//! native operators spell it.
mod workflow_graph;
use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Edge, EdgeKind, Flow, Handle, HandleKind, LayoutDirection,
    LayoutOptions, MiniMap, Node, NodeViewCtx, Point, Rect, Side, Viewport, use_flow,
    use_flow_handle,
};
use spiral_robotics_common::workflow::plan::{DiagnosticNode, SpiralPlanReport};
use workflow_graph::{
    StageContent, StageKind, StageOutput, SummaryKind, WorkflowGraph, WorkflowStage,
};

use super::highlight::{ExpressionHighlight, SqlHighlight};
use crate::components::icons::{
    DataIcon, FitViewIcon, JoinStageIcon, MediaStageIcon, ModelStageIcon, QueryNodeIcon,
    ResultNodeIcon, WorkflowIcon, ZoomInIcon, ZoomOutIcon,
};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};

const STAGE_WIDTH: f64 = 224.0;
const STAGE_HEIGHT: f64 = 92.0;
const MIN_READABLE_ZOOM: f64 = 0.7;

#[component]
pub(super) fn WorkflowPlanDisplay(
    report: SpiralPlanReport,
    /// A submitted query is executing: edges carry moving dashes while it does.
    active: bool,
    /// A stage requested from outside the canvas, selected and centred here.
    focus_stage: Signal<Option<String>>,
    on_select: EventHandler<WorkflowGraphDetails>,
) -> Element {
    rsx! {
        div { class: "workflow-plan-display",
            match WorkflowGraph::from_plan(report.logical()) {
                Ok(graph) => rsx! { WorkflowPlanGraph { graph, report, active, focus_stage, on_select } },
                Err(error) => rsx! {
                    div { class: "workflow-panel-empty", role: "alert",
                        strong { "The graph needs native expression metadata" }
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
                .gaps(28.0, 56.0),
        );
        for node in &mut nodes {
            if let Some(position) = positions.get(&node.id) {
                node.position = *position;
            }
        }
        // Reading order follows the layout: left to right, then top to bottom.
        let mut order = nodes
            .iter()
            .map(|node| (node.position.x, node.position.y, node.id.clone()))
            .collect::<Vec<_>>();
        order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        for (rank, (_, _, id)) in order.into_iter().enumerate() {
            if let Some(node) = nodes.iter_mut().find(|node| node.id == id) {
                node.data.order = rank;
            }
        }
        Self { nodes, edges }
    }
}

impl PlanFlow {
    /// Stage ids in reading order: left to right, then top to bottom.
    fn reading_order(&self) -> Vec<String> {
        let mut nodes = self.nodes.iter().collect::<Vec<_>>();
        nodes.sort_by_key(|node| node.data.order);
        nodes.into_iter().map(|node| node.id.clone()).collect()
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

/// The graph owns interpretation; its parent owns the active inspection.
#[derive(Clone, PartialEq)]
pub(super) struct WorkflowGraphDetails {
    selection: GraphSelection,
    report: SpiralPlanReport,
    root: String,
}
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
    target_title: String,
}
impl WorkflowEdgeDetails {
    fn from_edge(edge: &Edge, nodes: &[Node<StageNode>]) -> Option<Self> {
        let source = nodes.iter().find(|node| node.id == edge.source)?;
        let target = nodes.iter().find(|node| node.id == edge.target)?;
        Some(Self {
            source: source.data.stage.clone(),
            target_title: target.data.stage.content.title.clone(),
        })
    }
}

#[component]
fn WorkflowPlanGraph(
    graph: WorkflowGraph,
    report: SpiralPlanReport,
    active: bool,
    focus_stage: Signal<Option<String>>,
    on_select: EventHandler<WorkflowGraphDetails>,
) -> Element {
    let initial = use_hook(|| PlanFlow::from(&graph));
    let nodes = use_signal(|| initial.nodes);
    let mut edges = use_signal(|| initial.edges);
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
    let selection_root = root.clone();
    use_effect(move || {
        if let Some(selection) = selected() {
            on_select.call(WorkflowGraphDetails {
                selection,
                report: report.clone(),
                root: selection_root.clone(),
            });
        }
    });
    let stage_count = graph.stages.len();
    rsx! {
        div { class: "workflow-plan-graph",
            div { class: "workflow-flow-wrap",
                Flow { id: "workflow-native-plan", class: "workflow-flow",
                    nodes, edges, handle: flow, fit_view: false, fit_view_padding: 0.1,
                    min_zoom: 0.1, max_zoom: 1.5, nodes_draggable: false, delete_key: false,
                    is_valid_connection: move |_| false,
                    node_view: move |ctx: NodeViewCtx<StageNode>| rsx! { PlanStageCard { ctx, root: root.clone() } },
                    Background { variant: BackgroundVariant::Dots, gap: 22.0, size: 0.7 }
                    PlanInitialViewport { overflowing }
                    PlanFocus { nodes, edges, focus_stage }
                    PlanCanvasControls { stage_count }
                    if overflowing() { MiniMap { width: 112.0, height: 64.0 } }
                }
            }
        }
    }
}

/// Zoom and fit live on the canvas itself, quiet until needed, with the
/// stage count beside them so a pipeline wider than the pane still says how
/// much of it there is.
#[component]
fn PlanCanvasControls(stage_count: usize) -> Element {
    let core = use_flow();
    rsx! {
        div { class: "workflow-canvas-controls", role: "toolbar", "aria-label": "Graph view of {stage_count} stages",
            onpointerdown: move |event| event.stop_propagation(),
            span { class: "workflow-canvas-count",
                if stage_count == 1 { "1 stage" } else { "{stage_count} stages" }
            }
            Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon,
                aria_label: "Zoom in", title: "Zoom in",
                onclick: move |_| core.zoom_in(200), ZoomInIcon {} }
            Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon,
                aria_label: "Zoom out", title: "Zoom out",
                onclick: move |_| core.zoom_out(200), ZoomOutIcon {} }
            Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon,
                aria_label: "Fit the whole graph", title: "Fit the whole graph",
                onclick: move |_| core.fit_view(400), FitViewIcon {} }
        }
    }
}

/// Applies a stage request from the overview: select it on the canvas,
/// centre it, and clear the request.
#[component]
fn PlanFocus(
    nodes: Signal<Vec<Node<StageNode>>>,
    edges: Signal<Vec<Edge>>,
    focus_stage: Signal<Option<String>>,
) -> Element {
    let core = use_flow();
    use_effect(move || {
        let Some(id) = focus_stage() else {
            return;
        };
        let mut nodes = nodes;
        nodes.with_mut(|nodes| {
            for node in nodes.iter_mut() {
                node.selected = node.id == id;
            }
        });
        let mut edges = edges;
        if edges.peek().iter().any(|edge| edge.selected) {
            edges.with_mut(|edges| {
                for edge in edges.iter_mut() {
                    edge.selected = false;
                }
            });
        }
        if let Some(geometry) = core.geoms.peek().iter().find(|node| node.id == id) {
            core.center_on(geometry.rect.center(), 300);
        }
        let mut focus_stage = focus_stage;
        focus_stage.set(None);
    });
    rsx! {}
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

/// What the whole workflow does, shown while nothing is selected: the reads,
/// the joins, what it returns, and every stage as a way in.
#[component]
fn WorkflowOverview(report: SpiralPlanReport, focus_stage: Signal<Option<String>>) -> Element {
    let graph = WorkflowGraph::from_plan(report.logical()).ok();
    let Some(graph) = graph else {
        return rsx! {
            div { class: "workflow-panel-empty",
                strong { "Select a stage" }
                p { "Each stage explains what it does, what it reads or requires, and the columns it adds." }
            }
        };
    };
    let root = graph.root.clone();
    let order = PlanFlow::from(&graph).reading_order();
    let stages = order
        .iter()
        .filter_map(|id| graph.stages.iter().find(|stage| &stage.id == id))
        .collect::<Vec<_>>();
    let sources = stages
        .iter()
        .filter(|stage| stage.kind(&root) == StageKind::Source)
        .collect::<Vec<_>>();
    let joins = stages
        .iter()
        .filter(|stage| stage.combines_inputs())
        .collect::<Vec<_>>();
    let output = graph.stages.iter().find(|stage| stage.id == root);
    let columns = output
        .map(|stage| stage.output_columns().len())
        .unwrap_or(0);
    let mut focus_stage = focus_stage;
    rsx! {
        div { class: "workflow-inspector-body workflow-overview",
            header { class: "workflow-inspector-heading",
                span { class: "workflow-stage-mark workflow-stage-shape", "aria-hidden": "true", WorkflowIcon {} }
                div {
                    h3 { "Workflow" }
                    small {
                        if stages.len() == 1 { "1 stage" } else { "{stages.len()} stages" }
                        if columns > 0 { " · returns {columns} columns" }
                    }
                }
            }
            if !sources.is_empty() {
                h4 { if sources.len() == 1 { "Reads" } else { "Reads from" } }
                dl { class: "workflow-facts",
                    for source in sources.iter() {
                        div { key: "{source.id}",
                            dt { "{StageContent::provider_label(&source.footer(&root))}" }
                            dd {
                                strong { "{source.content.title}" }
                                if !source.content.summary.is_empty() {
                                    span { class: "workflow-overview-summary", title: "{source.content.summary}",
                                        PathText { text: source.content.summary.clone() }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !joins.is_empty() {
                h4 { "Combines" }
                dl { class: "workflow-facts",
                    for join in joins.iter() {
                        div { key: "{join.id}",
                            dt { "{join.content.title}" }
                            dd { StageText { text: join.content.summary.clone(), kind: join.content.summary_kind } }
                        }
                    }
                }
            }
            h4 { "Stages" }
            ol { class: "workflow-overview-stages",
                for stage in stages.iter() {
                    li { key: "{stage.id}", class: "workflow-stage-{stage.kind(&root).class_name()}",
                        Action { purpose: ActionPurpose::Select, variant: ActionVariant::Row,
                            class: "workflow-overview-stage",
                            title: "Show {stage.content.title} on the graph",
                            onclick: { let id = stage.id.clone(); move |_| focus_stage.set(Some(id.clone())) },
                            span { class: "workflow-stage-mark", "aria-hidden": "true", StageKindGlyph { kind: stage.kind(&root) } }
                            span { class: "workflow-overview-stage-copy",
                                strong { "{stage.content.title}" }
                                if !stage.content.summary.is_empty() {
                                    small { StageText { text: stage.content.summary.clone(), kind: stage.content.summary_kind } }
                                }
                            }
                        }
                    }
                }
            }
            p { class: "workflow-inspector-note", "Select a stage on the graph or in this list to see what it reads, requires, and produces." }
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

/// A summary or fact value, coloured token by token when it is an expression.
#[component]
fn StageText(text: String, kind: SummaryKind) -> Element {
    match kind {
        SummaryKind::Expression => rsx! { ExpressionHighlight { text } },
        SummaryKind::Prose => rsx! { PathText { text } },
    }
}

#[component]
pub(super) fn WorkflowGraphInspector(
    selection: Option<WorkflowGraphDetails>,
    /// The current draft's plan, for the overview shown before any selection.
    report: Option<SpiralPlanReport>,
    focus_stage: Signal<Option<String>>,
) -> Element {
    rsx! {
        section { class: "workflow-stage-inspector", "aria-label": "Stage details",
            match selection {
                Some(WorkflowGraphDetails { selection: GraphSelection::Stage { stage, inputs }, report, root }) => rsx! {
                    WorkflowStageInspector { key: "{stage.id}", stage, inputs, report, root }
                },
                Some(WorkflowGraphDetails { selection: GraphSelection::Edge(details), root, .. }) => rsx! {
                    div { class: "workflow-inspector-body", key: "{details.source.id}:{details.target_title}",
                        header { class: "workflow-inspector-heading",
                            span { class: "workflow-stage-mark workflow-stage-{details.source.kind(&root).class_name()}", "aria-hidden": "true",
                                StageKindGlyph { kind: details.source.kind(&root) }
                            }
                            div {
                                h3 { "{details.source.content.title} → {details.target_title}" }
                                small { "Connection" }
                            }
                        }
                        match details.source.output() {
                            StageOutput::Relation(output) => rsx! {
                                h4 { "Rows carried" }
                                PlanOutputSchema { output: output.clone(), added: Vec::new() }
                            },
                            StageOutput::Expression { expression, dtype } => rsx! {
                                h4 { "Value carried" }
                                dl { class: "workflow-facts",
                                    if let Some(dtype) = dtype {
                                        div { dt { "Type" } dd { ExpressionHighlight { text: dtype.clone() } } }
                                    }
                                    div { dt { "Call" } dd { SqlHighlight { sql: expression.clone() } } }
                                }
                            },
                        }
                    }
                },
                None => match report {
                    Some(report) => rsx! { WorkflowOverview { report, focus_stage } },
                    None => rsx! {
                        div { class: "workflow-panel-empty is-quiet",
                            p { "Stage details appear here once the graph exists: what each stage reads, requires, and produces." }
                        }
                    },
                },
            }
        }
    }
}

#[component]
fn WorkflowStageInspector(
    stage: WorkflowStage,
    inputs: Vec<StageInput>,
    report: SpiralPlanReport,
    root: String,
) -> Element {
    let kind = stage.kind(&root);
    let footer = stage.footer(&root);
    let facts = stage.facts();
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
    let is_join = stage.is_join() && inputs.len() == 2;
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
            }
            if !stage.content.summary.is_empty() {
                p { class: "workflow-inspector-summary",
                    StageText { text: stage.content.summary.clone(), kind: stage.content.summary_kind }
                }
            }
            if let Some(expression) = &stage.content.expression {
                if stage.content.function.is_none() && !stage.reads_source() {
                    div { class: "workflow-inspector-rule",
                        ExpressionHighlight { text: expression.clone() }
                    }
                }
            }
            if !inputs.is_empty() {
                h4 { if inputs.len() == 1 { "Reads from" } else { "Combines" } }
                dl { class: "workflow-facts",
                    for (index, input) in inputs.iter().enumerate() {
                        div { key: "{index}",
                            dt {
                                if is_join { if index == 0 { "Left" } else { "Right" } }
                                else if inputs.len() == 1 { "Stage" }
                                else { "Input {index + 1}" }
                            }
                            dd { "{input.title}" }
                        }
                    }
                }
            }
            if !facts.is_empty() {
                h4 { "Details" }
                dl { class: "workflow-facts",
                    for (index, fact) in facts.iter().enumerate() {
                        div { key: "{index}",
                            dt { "{fact.label}" }
                            dd { StageText { text: fact.value.clone(), kind: fact.kind } }
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
            match stage.output() {
                StageOutput::Relation(output) => rsx! {
                    h4 {
                        if produced.is_empty() { "Columns produced" }
                        else if added.is_empty() { "Columns produced · {produced.len()}" }
                        else if added.len() == 1 { "Columns produced · {produced.len()} · 1 new" }
                        else { "Columns produced · {produced.len()} · {added.len()} new" }
                    }
                    PlanOutputSchema { output: output.clone(), added: added.clone() }
                },
                StageOutput::Expression { dtype: Some(dtype), .. } => rsx! {
                    h4 { "Produces" }
                    p { class: "workflow-inspector-note", "One value per row of type " ExpressionHighlight { text: dtype.clone() } }
                },
                StageOutput::Expression { dtype: None, .. } => rsx! {},
            }
            details { class: "workflow-inspector-disclosure workflow-inspector-native",
                summary {
                    if stage.operators.len() == 1 { "Native operator" } else { "Native operators · {stage.operators.len()}" }
                }
                for node in &stage.operators {
                    PlanOperatorDetails { key: "{node.id()}", node: node.clone() }
                }
            }
            details { class: "workflow-inspector-disclosure",
                summary { "Full plan JSON" }
                pre { "{serde_json::to_string_pretty(&report).unwrap_or_default()}" }
            }
        }
    }
}

#[component]
fn PlanInitialViewport(overflowing: Signal<bool>) -> Element {
    let core = use_flow();
    let mut previous_layout = use_signal(|| None::<(Rect, Rect)>);
    let mut automatic_viewport = use_signal(|| None::<Viewport>);
    let mut user_controlled = use_signal(|| false);
    let mut overflowing = overflowing;
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
        // The controls own the bottom 40px; the rest of the pane is canvas.
        let safe = Rect::new(
            24.0,
            20.0,
            (container.width - 48.0).max(1.0),
            (container.height - 68.0).max(1.0),
        );
        // Fit the whole graph while it stays readable. A pipeline wider than
        // the pane is going to be panned anyway, so it keeps its cards at the
        // largest size the pane's height allows, starts at its sources, and
        // gets the minimap: legible stages beat a complete but unreadable one.
        let fit_zoom = (safe.width / bounds.width).min(safe.height / bounds.height);
        let fits = fit_zoom >= MIN_READABLE_ZOOM;
        if *overflowing.peek() != !fits {
            overflowing.set(!fits);
        }
        let viewport = if fits {
            Viewport::fit(Some(bounds), safe, MIN_READABLE_ZOOM, 1.0)
        } else {
            let zoom = (safe.height / bounds.height).clamp(MIN_READABLE_ZOOM, 1.0);
            let source_x = geometry
                .iter()
                .map(|node| node.rect.x)
                .fold(f64::INFINITY, f64::min);
            Viewport::new(
                safe.x - source_x * zoom,
                container.height / 2.0 - bounds.center().y * zoom,
                zoom,
            )
        };
        automatic_viewport.set(Some(viewport));
        core.set_viewport(viewport, 0);
    });
    rsx! {}
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
            div { class: "workflow-stage-heading",
                span { class: "workflow-stage-mark", "aria-hidden": "true", StageKindGlyph { kind } }
                strong { class: "workflow-stage-title", title: "{stage.content.title}", "{stage.content.title}" }
            }
            p { class: "workflow-stage-summary", title: "{stage.content.summary}",
                if stage.content.summary.is_empty() { "{footer}" }
                else { StageText { text: stage.content.summary.clone(), kind: stage.content.summary_kind } }
            }
            footer { class: "workflow-stage-footer", title: "{kind.label()} · {footer}",
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
            details { class: "workflow-inspector-disclosure",
                summary { "Full output metadata" }
                pre { "{serde_json::to_string_pretty(node.output()).unwrap_or_default()}" }
            }
            if let Some(execution) = node.execution() {
                details { class: "workflow-inspector-disclosure",
                    summary { "Execution identity" }
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
