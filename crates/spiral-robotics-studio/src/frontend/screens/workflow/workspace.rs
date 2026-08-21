//! Four-panel Workflow activity: relation lineage, inspector, assistant, and results.

use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Flow, Handle, HandleKind, NodeViewCtx, use_flow_handle,
};
use spiral_robotics_panels::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_ui::{Action, ActionPurpose, ActionVariant};

use crate::frontend::components::icons::{
    FitViewIcon, ModelNodeIcon, QueryNodeIcon, ResultNodeIcon, TableNodeIcon,
};
use crate::frontend::screens::workflow::state::{
    ASSISTANT_PANEL_ID, CANVAS_PANEL_ID, NODE_PANEL_ID, RESULTS_PANEL_ID, WorkflowNodeData,
    WorkflowNodeKind, workflow_controller,
};

const WORKFLOW_LAYOUT_SCOPE: &str = "workflow-relation-canvas-v1";

#[component]
pub(crate) fn WorkflowWorkspace() -> Element {
    let controller = workflow_controller();
    let panels = vec![
        Panel::new(
            ASSISTANT_PANEL_ID,
            "Assistant",
            "workflow-left",
            rsx! { WorkflowAssistant {} },
        )
        .with_class("workflow-assistant-editor"),
        Panel::new(
            NODE_PANEL_ID,
            "Node",
            "workflow-left",
            rsx! { WorkflowNodeInspector {} },
        )
        .with_class("workflow-node-editor"),
        Panel::new(
            CANVAS_PANEL_ID,
            "Workflow",
            "workflow-canvas",
            rsx! { WorkflowCanvas {} },
        )
        .with_class("workflow-canvas-editor"),
        Panel::new(
            RESULTS_PANEL_ID,
            "Results",
            "workflow-results",
            rsx! { WorkflowResults {} },
        )
        .with_class("workflow-results-editor"),
    ];

    rsx! {
        PanelWorkspace {
            panels,
            initial_layout: default_workflow_layout(),
            layout_scope: Some(WORKFLOW_LAYOUT_SCOPE.to_owned()),
            active_panel: controller.active_panel(),
            on_layout_change: move |_layout: PanelLayout| {},
            on_panel_activate: move |id: PanelId| controller.activate_panel(id),
            on_resize: move |_| {},
        }
    }
}

pub(crate) fn default_workflow_layout() -> PanelLayout {
    let right = LayoutNode::split(
        "workflow-right-rows",
        SplitAxis::Vertical,
        0.62,
        LayoutNode::tile("workflow-canvas", [CANVAS_PANEL_ID]),
        LayoutNode::tile("workflow-results", [RESULTS_PANEL_ID]),
    );
    PanelLayout::new(LayoutNode::split(
        "workflow-columns",
        SplitAxis::Horizontal,
        0.30,
        LayoutNode::tile("workflow-left", [ASSISTANT_PANEL_ID, NODE_PANEL_ID]),
        right,
    ))
}

#[component]
fn WorkflowAssistant() -> Element {
    rsx! {
        section { class: "workflow-assistant", "aria-label": "Workflow assistant scaffold",
            div { class: "workflow-assistant-scroll",
                header { class: "workflow-assistant-intro",
                    h2 { "Ask about Spiral relations" }
                    p {
                        "The assistant will inspect relations, construct lineage, and explain each run. Agent turns are not connected in this scaffold."
                    }
                }
                section { class: "workflow-example-prompt",
                    h3 { "Example prompt" }
                    p { "Find moments where the gripper holds the cube while the arm is still." }
                }
                section { class: "workflow-assistant-plan",
                    h3 { "This draft would" }
                    ol {
                        li {
                            span { "1" }
                            div {
                                strong { "Read two Spiral tables" }
                                p { "Camera frames and joint state remain separate relations." }
                            }
                        }
                        li {
                            span { "2" }
                            div {
                                strong { "Materialize model scores" }
                                p { "SigLIP owns the derived workflow.siglip_scores relation." }
                            }
                        }
                        li {
                            span { "3" }
                            div {
                                strong { "Join and render matches" }
                                p { "Read-only Spiral SQL feeds a media-aware result view." }
                            }
                        }
                    }
                }
            }
            footer { class: "workflow-assistant-footer",
                span { "Assistant input arrives with workflow execution." }
            }
        }
    }
}

#[component]
fn WorkflowNodeInspector() -> Element {
    let controller = workflow_controller();
    let selected = controller.selected_node_data();
    rsx! {
        section { class: "workflow-inspector", "aria-label": "Selected workflow node",
            if let Some((title, data)) = selected {
                header { class: "workflow-inspector-heading",
                    div { class: "workflow-inspector-kind",
                        {node_kind_icon(data.kind)}
                        span { "{data.kind.label()}" }
                    }
                    h2 { "{title}" }
                    p { "{data.description}" }
                }
                dl { class: "workflow-inspector-facts",
                    div {
                        dt { "State" }
                        dd { class: "{data.state.class_name()}", "{data.state.label()}" }
                    }
                    if !data.inputs.is_empty() {
                        div {
                            dt { "Reads" }
                            dd { class: "workflow-inspector-relations",
                                for input in &data.inputs { code { "{input}" } }
                            }
                        }
                    }
                    if data.kind == WorkflowNodeKind::Table {
                        div {
                            dt { "Relation" }
                            dd { code { "{title}" } }
                        }
                    }
                    if let Some(output) = &data.output_relation {
                        div {
                            dt { "Writes" }
                            dd { code { "{output}" } }
                        }
                    }
                    div {
                        dt { "Fields" }
                        dd { class: "workflow-inspector-fields",
                            for field in &data.fields { code { "{field}" } }
                        }
                    }
                }
                if let Some(sql) = &data.sql {
                    section { class: "workflow-inspector-sql",
                        h3 { "Read-only SQL" }
                        pre { code { "{sql}" } }
                    }
                }
                if data.kind == WorkflowNodeKind::Table {
                    p { class: "workflow-inspector-boundary",
                        "Import provenance belongs in Data. Workflow nodes always name Spiral relations."
                    }
                }
            } else {
                div { class: "workflow-panel-empty",
                    strong { "No node selected" }
                    p { "Choose a node on the canvas to inspect its relation contract." }
                    span { "Keyboard: Tab to a node, then press Enter or Space." }
                }
            }
        }
    }
}

#[component]
fn WorkflowCanvas() -> Element {
    let controller = workflow_controller();
    let nodes = controller.nodes();
    let edges = controller.edges();
    let flow = use_flow_handle::<WorkflowNodeData>();

    // dioxus-flow owns keyboard selection. Mirror that selection into the
    // activity controller so Enter/Space and pointer selection converge on the
    // same inspector state without selecting anything on initial render.
    use_effect(move || {
        let selected = nodes
            .read()
            .iter()
            .find(|node| node.selected)
            .map(|node| node.id.clone());
        match (selected, controller.selected_node()) {
            (Some(next), current) if current.as_deref() != Some(next.as_str()) => {
                controller.select_node(next);
            }
            (None, Some(_)) => controller.clear_selection(),
            _ => {}
        }
    });

    rsx! {
        section { class: "workflow-canvas", "aria-label": "Illustrative relation workflow",
            header { class: "workflow-canvas-toolbar",
                div {
                    strong { "Static draft" }
                    span { "5 nodes · 4 connections" }
                }
                Action {
                    purpose: ActionPurpose::Fit,
                    variant: ActionVariant::Outline,
                    class: "workflow-fit-action",
                    title: "Fit the example graph in the canvas",
                    onclick: move |_| flow.fit_view(300),
                    FitViewIcon {}
                    "Fit graph"
                }
            }
            div { class: "workflow-flow-wrap",
                Flow {
                    id: "workflow-example-graph",
                    class: "workflow-flow",
                    nodes,
                    edges,
                    handle: flow,
                    fit_view: true,
                    fit_view_padding: 0.10,
                    min_zoom: 0.45,
                    max_zoom: 1.8,
                    drag_threshold: 4.0,
                    delete_key: false,
                    is_valid_connection: move |_| false,
                    on_node_click: move |id: String| controller.select_node(id),
                    on_pane_click: move |_| controller.clear_selection(),
                    node_view: move |ctx: NodeViewCtx<WorkflowNodeData>| rsx! {
                        WorkflowNodeCard { ctx }
                    },
                    Background { variant: BackgroundVariant::Dots, gap: 20.0, size: 0.7 }
                }
            }
        }
    }
}

#[component]
fn WorkflowNodeCard(ctx: NodeViewCtx<WorkflowNodeData>) -> Element {
    let node = &ctx.node;
    let data = &node.data;
    let kind = data.kind;
    let fields = data.fields.join(" · ");
    let selected_class = if node.selected {
        "workflow-node-selected"
    } else {
        ""
    };
    let dragging_class = if ctx.dragging {
        "workflow-node-dragging"
    } else {
        ""
    };

    rsx! {
        article {
            class: "workflow-node-card {kind.class_name()} {selected_class} {dragging_class}",
            if kind == WorkflowNodeKind::Query {
                Handle {
                    kind: HandleKind::Target,
                    position: node.target_side,
                    id: "scores",
                    offset: 0.36,
                }
                Handle {
                    kind: HandleKind::Target,
                    position: node.target_side,
                    id: "joint-state",
                    offset: 0.72,
                }
            } else if kind != WorkflowNodeKind::Table {
                Handle { kind: HandleKind::Target, position: node.target_side }
            }
            header { class: "workflow-node-heading",
                {node_kind_icon(kind)}
                span { "{kind.label()}" }
            }
            div { class: "workflow-node-body",
                h3 { title: "{node.label}", "{node.label}" }
                p { class: "workflow-node-fields", title: "{fields}", "{fields}" }
                if let Some(output) = &data.output_relation {
                    p { class: "workflow-node-relation", "writes ", code { "{output}" } }
                }
                if let Some(sql) = &data.sql {
                    code { class: "workflow-node-sql", "{sql}" }
                }
            }
            footer { class: "workflow-node-footer",
                span { class: "workflow-node-status-dot {data.state.class_name()}", "aria-hidden": "true" }
                span { "{data.state.label()}" }
            }
            if kind != WorkflowNodeKind::Result {
                Handle { kind: HandleKind::Source, position: node.source_side }
            }
        }
    }
}

fn node_kind_icon(kind: WorkflowNodeKind) -> Element {
    match kind {
        WorkflowNodeKind::Table => rsx! { TableNodeIcon {} },
        WorkflowNodeKind::Model => rsx! { ModelNodeIcon {} },
        WorkflowNodeKind::Query => rsx! { QueryNodeIcon {} },
        WorkflowNodeKind::Result => rsx! { ResultNodeIcon {} },
    }
}

#[component]
fn WorkflowResults() -> Element {
    rsx! {
        section { class: "workflow-results", "aria-label": "Workflow results scaffold",
            div { class: "workflow-results-column-preview", "aria-hidden": "true",
                span { "Preview" }
                span { "Episode" }
                span { "Time" }
                span { "Score" }
            }
            div { class: "workflow-panel-empty",
                ResultNodeIcon {}
                strong { "No workflow results" }
                p {
                    "Execution is not connected in this scaffold. Matching rows and media previews will appear here after a Result node completes."
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_tiles<'a>(node: &'a LayoutNode, out: &mut Vec<(&'a str, Vec<&'a str>)>) {
        match node {
            LayoutNode::Tile(tile) => out.push((
                tile.id.as_str(),
                tile.panels.iter().map(PanelId::as_str).collect(),
            )),
            LayoutNode::Split { first, second, .. } => {
                collect_tiles(first, out);
                collect_tiles(second, out);
            }
        }
    }

    #[test]
    fn workflow_registers_the_approved_four_panel_composition() {
        let layout = default_workflow_layout();
        let mut tiles = Vec::new();
        collect_tiles(&layout.root, &mut tiles);

        assert_eq!(
            tiles,
            vec![
                ("workflow-left", vec![ASSISTANT_PANEL_ID, NODE_PANEL_ID]),
                ("workflow-canvas", vec![CANVAS_PANEL_ID]),
                ("workflow-results", vec![RESULTS_PANEL_ID]),
            ]
        );
        match &layout.root {
            LayoutNode::Split { axis, ratio, .. } => {
                assert_eq!(*axis, SplitAxis::Horizontal);
                assert!((*ratio - 0.30).abs() < f64::EPSILON);
            }
            LayoutNode::Tile(_) => panic!("workflow root must be a split"),
        }
    }
}
