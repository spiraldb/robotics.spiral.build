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
    DataIcon, JoinStageIcon, MediaStageIcon, ModelStageIcon, QueryNodeIcon, ResultNodeIcon,
};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant, CloseIcon};

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
                    div { class: "panel-empty", role: "alert",
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

/// Where the sheet sits, as inline style: left-aligned with the selected
/// cards, below them when the pane has room, above them when only that side
/// has room, and over them from the top of a canvas too short for either,
/// always inside the canvas. `anchor` and `container` are both in screen
/// pixels. Every placement names `top` and `bottom` (one of them `auto`):
/// the style attribute is diffed property by property, so a declaration
/// that merely disappears stays applied, and a sheet that moved from above
/// to below would keep both edges and collapse to a line.
fn popover_placement(anchor: Rect, container: Rect) -> String {
    const WIDTH: f64 = 320.0;
    const GAP: f64 = 8.0;
    const COMFORTABLE: f64 = 240.0;
    const MIN_HEIGHT: f64 = 120.0;
    let left = anchor.x.min(container.width - GAP - WIDTH).max(GAP);
    let below_top = (anchor.max_y() + GAP).max(GAP);
    let below_space = container.height - GAP - below_top;
    let above_bottom = (container.height - anchor.y + GAP).max(GAP);
    let above_space = container.height - GAP - above_bottom;
    if below_space >= COMFORTABLE || (below_space >= above_space && below_space >= MIN_HEIGHT) {
        format!("left:{left:.0}px;top:{below_top:.0}px;bottom:auto;max-height:{below_space:.0}px")
    } else if above_space >= MIN_HEIGHT {
        format!(
            "left:{left:.0}px;top:auto;bottom:{above_bottom:.0}px;max-height:{above_space:.0}px"
        )
    } else {
        let space = (container.height - 2.0 * GAP).max(GAP);
        format!("left:{left:.0}px;top:{GAP:.0}px;bottom:auto;max-height:{space:.0}px")
    }
}

/// Closing the sheet is deselecting: the canvas and the sheet always agree
/// on what is selected.
fn deselect(mut nodes: Signal<Vec<Node<StageNode>>>, mut edges: Signal<Vec<Edge>>) {
    nodes.with_mut(|nodes| {
        for node in nodes.iter_mut() {
            node.selected = false;
        }
    });
    edges.with_mut(|edges| {
        for edge in edges.iter_mut() {
            edge.selected = false;
        }
    });
}

#[component]
fn WorkflowPlanGraph(graph: WorkflowGraph, active: bool) -> Element {
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
    let card_root = root.clone();
    let stage_count = graph.stages.len();
    rsx! {
        div { class: "workflow-plan-graph",
            div { class: "workflow-flow-wrap",
                // Escape deselects from wherever focus sits on the canvas:
                // the pane, the selected card, or the sheet itself.
                onkeydown: move |event| {
                    if event.key() == Key::Escape && selected.peek().is_some() {
                        deselect(nodes, edges);
                    }
                },
                Flow { id: "workflow-native-plan", class: "workflow-flow",
                    nodes, edges, handle: flow, fit_view: false, fit_view_padding: 0.1,
                    min_zoom: 0.1, max_zoom: 1.5, nodes_draggable: false, delete_key: false,
                    is_valid_connection: move |_| false,
                    node_view: move |ctx: NodeViewCtx<StageNode>| rsx! { PlanStageCard { ctx, root: card_root.clone() } },
                    Background { variant: BackgroundVariant::Dots, gap: 22.0, size: 0.7 }
                    PlanInitialViewport { overflowing }
                    PlanCanvasControls { stage_count }
                    if overflowing() { MiniMap { width: 112.0, height: 64.0 } }
                    if let Some(selection) = selected() {
                        StagePopover { selection, root: root.clone(), on_close: move |_| deselect(nodes, edges) }
                    }
                }
            }
        }
    }
}

/// Zoom and fit live on the canvas itself, quiet until needed.
#[component]
fn PlanCanvasControls(stage_count: usize) -> Element {
    rsx! {
        crate::components::canvas_controls::CanvasControls {
            label: format!("Graph view of {stage_count} stages"),
            fit_label: "Fit the whole graph",
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

/// A summary or fact value, coloured token by token when it is an expression.
#[component]
fn StageText(text: String, kind: SummaryKind) -> Element {
    match kind {
        SummaryKind::Expression => rsx! { ExpressionHighlight { text } },
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
    let placement = popover_placement(anchor, container);
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
                Action { purpose: ActionPurpose::Close, variant: ActionVariant::Icon,
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
                        ExpressionHighlight { text: expression.clone() }
                    }
                }
            }
            if !facts.is_empty() {
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
            details { class: "workflow-inspector-disclosure workflow-inspector-native",
                summary {
                    if stage.operators.len() == 1 { "Native operator" } else { "Native operators · {stage.operators.len()}" }
                }
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
                Action { purpose: ActionPurpose::Close, variant: ActionVariant::Icon,
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
                    dl { class: "workflow-facts",
                        if let Some(dtype) = dtype {
                            div { dt { "Type" } dd { ExpressionHighlight { text: dtype.clone() } } }
                        }
                        div { dt { "Call" } dd { SqlHighlight { sql: expression.clone() } } }
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
    fn sheet_sits_below_above_or_over_the_card_and_always_names_both_edges() {
        let container = Rect {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 400.0,
        };
        let card = |y: f64| Rect {
            x: 100.0,
            y,
            width: 168.0,
            height: 69.0,
        };
        // Room below: aligned with the card, the sheet starts under it.
        let below = popover_placement(card(20.0), container);
        assert_eq!(below, "left:100px;top:97px;bottom:auto;max-height:295px");
        // Room only above: the sheet ends over the card.
        let above = popover_placement(card(300.0), container);
        assert_eq!(above, "left:100px;top:auto;bottom:108px;max-height:284px");
        // A canvas too short for either side: the sheet covers from the top
        // and never leaves the canvas.
        let short = Rect {
            width: 1000.0,
            height: 180.0,
            ..container
        };
        let over = popover_placement(card(60.0), short);
        assert_eq!(over, "left:100px;top:8px;bottom:auto;max-height:164px");
        // The right edge clamps the sheet inside the canvas.
        let clamped = popover_placement(
            Rect {
                x: 900.0,
                ..card(20.0)
            },
            container,
        );
        assert!(clamped.starts_with("left:672px;"));
        for placement in [below, above, over, clamped] {
            assert!(placement.contains("top:") && placement.contains("bottom:"));
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
