//! Four-panel relation Workflow editor, execution canvas, and real results.

use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Flow, Handle, HandleKind, NodeViewCtx, use_flow_handle,
};
use spiral_robotics_contracts::workflow::{WorkflowNodeConfig, WorkflowNodeKind, WorkflowRunPhase};
use spiral_robotics_panels::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_ui::{Action, ActionPurpose, ActionVariant};

use crate::frontend::app::state::studio;
use crate::frontend::components::icons::{
    FitViewIcon, ModelNodeIcon, QueryNodeIcon, ResultNodeIcon, TableNodeIcon,
};
use crate::frontend::screens::data::workbench::{QueryState, QueryViewModel, render_query_result};
use crate::frontend::screens::workflow::state::{
    ASSISTANT_PANEL_ID, CANVAS_PANEL_ID, NODE_PANEL_ID, RESULTS_PANEL_ID, WorkflowCommand,
    WorkflowNodeData, workflow_controller,
};

const WORKFLOW_LAYOUT_SCOPE: &str = "workflow-relation-canvas-v2";

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
        section { class: "workflow-assistant", "aria-label": "Workflow assistant unavailable",
            div { class: "workflow-assistant-scroll",
                header { class: "workflow-assistant-intro",
                    h2 { "Assistant is not connected" }
                    p {
                        "Use the Node inspector to choose Spiral relations and edit read-only SQL. Agent turns arrive in a later stage."
                    }
                }
                section { class: "workflow-assistant-plan",
                    h3 { "Available now" }
                    ol {
                        li {
                            span { "1" }
                            div {
                                strong { "Choose a Spiral table" }
                                p { "Workflow reads relations already materialized in the active project." }
                            }
                        }
                        li {
                            span { "2" }
                            div {
                                strong { "Apply read-only SQL" }
                                p { "Exact node placeholders are resolved to server-owned qualified names." }
                            }
                        }
                        li {
                            span { "3" }
                            div {
                                strong { "Run and inspect results" }
                                p { "The backend publishes run state and a bounded media-aware payload." }
                            }
                        }
                    }
                }
            }
            footer { class: "workflow-assistant-footer",
                span { "Assistant input is unavailable in Stage 2." }
            }
        }
    }
}

#[component]
fn WorkflowNodeInspector() -> Element {
    let controller = workflow_controller();
    let selected = controller.selected_node_data();
    let relations = controller.relations();
    let active = controller.snapshot().phase.active();
    rsx! {
        section { class: "workflow-inspector", "aria-label": "Selected workflow node",
            if let Some((node_id, title, data)) = selected {
                header { class: "workflow-inspector-heading",
                    div { class: "workflow-inspector-kind",
                        {node_kind_icon(data.kind)}
                        span { "{data.kind_label()}" }
                    }
                    h2 { "{title}" }
                    p { "{data.description}" }
                }
                if let Some(error) = &data.error {
                    div { class: "workflow-inspector-error", role: "alert",
                        strong { "{data.phase_label()}" }
                        p { "{error}" }
                    }
                }
                dl { class: "workflow-inspector-facts",
                    div {
                        dt { "State" }
                        dd { class: "{data.phase_class()}", "{data.phase_label()} · {data.detail}" }
                    }
                    if let Some(relation) = &data.relation {
                        div {
                            dt { "Relation" }
                            dd { code { "{relation}" } }
                        }
                    }
                    if let Some(output) = &data.output_relation {
                        div {
                            dt { "Writes" }
                            dd { code { "{output}" } }
                        }
                    }
                    if !data.fields.is_empty() {
                        div {
                            dt { "Fields" }
                            dd { class: "workflow-inspector-fields",
                                for field in &data.fields { code { "{field}" } }
                            }
                        }
                    }
                }
                if data.kind == WorkflowNodeKind::Table {
                    section { class: "workflow-inspector-editor",
                        label {
                            span { class: "label", "Spiral relation" }
                            select {
                                class: "field",
                                disabled: active,
                                value: data.relation.clone().unwrap_or_default(),
                                onchange: {
                                    let node_id = node_id.clone();
                                    move |event| controller.set_table_relation(&node_id, event.value())
                                },
                                option { value: "", "Choose a qualified relation…" }
                                for relation in &relations {
                                    option {
                                        value: "{relation.name}",
                                        selected: data.relation.as_deref() == Some(relation.name.as_str()),
                                        "{relation.name}"
                                    }
                                }
                            }
                        }
                        if relations.is_empty() {
                            p { class: "workflow-inspector-help", role: "status",
                                "No catalog relations are available. Configure an active Spiral project or materialize data in Data first."
                            }
                        }
                        if let Some(limitation) = controller.relation_limitation() {
                            p { class: "workflow-inspector-help", "{limitation}" }
                        }
                    }
                    p { class: "workflow-inspector-boundary",
                        "Import provenance belongs in Data. Workflow always begins from qualified Spiral relations."
                    }
                } else if data.kind == WorkflowNodeKind::Query {
                    section { class: "workflow-inspector-editor workflow-inspector-query-editor",
                        label {
                            span { class: "label", "Read-only SQL" }
                            textarea {
                                class: "query-input workflow-query-input",
                                spellcheck: "false",
                                disabled: active,
                                value: data.sql.clone().unwrap_or_default(),
                                oninput: {
                                    let node_id = node_id.clone();
                                    move |event| controller.set_query_sql(&node_id, event.value())
                                },
                            }
                        }
                        p { class: "workflow-inspector-help",
                            "Reference each connected Table with an exact placeholder such as "
                            code { "{{node:table}}" }
                            "."
                        }
                    }
                } else if data.kind == WorkflowNodeKind::Model {
                    p { class: "workflow-inspector-boundary workflow-inspector-unavailable",
                        "Model execution is reserved for Stage 3. Remove Model nodes to run this workflow now."
                    }
                }
                {node_validation_issues(&node_id)}
            } else {
                div { class: "workflow-panel-empty",
                    strong { "No node selected" }
                    p { "Choose a node on the canvas to inspect or edit its relation contract." }
                    span { "Keyboard: Tab, then Enter or Space" }
                }
            }
        }
    }
}

fn node_validation_issues(node_id: &str) -> Element {
    let validation = workflow_controller().validation();
    let issues = validation
        .errors
        .into_iter()
        .filter(|issue| issue.node_id.as_deref().is_none_or(|id| id == node_id))
        .collect::<Vec<_>>();
    rsx! {
        if !issues.is_empty() {
            section { class: "workflow-validation", role: "alert",
                for issue in issues {
                    div {
                        strong { "{issue.message}" }
                        p { "{issue.recovery}" }
                    }
                }
            }
        }
    }
}

#[component]
fn WorkflowCanvas() -> Element {
    let controller = workflow_controller();
    let studio = studio();
    let nodes = controller.nodes();
    let edges = controller.edges();
    let flow = use_flow_handle::<WorkflowNodeData>();
    let mut canvas_was_active = use_signal(|| false);
    let spec = controller.spec();
    let snapshot = controller.snapshot();
    let run_reason = controller.run_disabled_reason();
    let apply_reason = controller.apply_disabled_reason();
    let command = controller.command();
    let active = snapshot.phase.active();
    let state_load_failed = controller.load_error().is_some();
    let state_label = if controller.dirty() {
        "Modified"
    } else {
        run_phase_label(snapshot.phase)
    };

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

    use_effect(move || {
        let canvas_active = controller
            .active_panel()
            .as_ref()
            .is_some_and(|panel| panel.as_str() == CANVAS_PANEL_ID);
        let was_active = *canvas_was_active.peek();
        if canvas_active != was_active {
            canvas_was_active.set(canvas_active);
        }
        if canvas_active && !was_active {
            spawn(async move {
                // PanelWorkspace updates mobile visibility from the same
                // activation. Wait one frame so dioxus-flow measures the now
                // visible canvas rather than its former hidden bounds.
                gloo_timers::future::TimeoutFuture::new(32).await;
                flow.fit_view(300);
            });
        }
    });

    rsx! {
        section { class: "workflow-canvas", "aria-label": "Spiral relation workflow",
            header { class: "workflow-canvas-toolbar",
                div {
                    strong { class: if controller.dirty() { "workflow-toolbar-modified" } else { "workflow-toolbar-saved" }, "{state_label}" }
                    span { "{spec.nodes.len()} nodes · {spec.edges.len()} connections · r{spec.revision}" }
                }
                Action {
                    purpose: ActionPurpose::Fit,
                    variant: ActionVariant::Outline,
                    class: "workflow-fit-action",
                    title: "Fit the workflow graph in the canvas",
                    onclick: move |_| flow.fit_view(300),
                    FitViewIcon {}
                    "Fit"
                }
                if controller.revision_conflict() {
                    Action {
                        purpose: ActionPurpose::Sync,
                        variant: ActionVariant::Outline,
                        title: "Discard local edits and reload the newer saved Workflow",
                        onclick: move |_| controller.reload_saved(),
                        "Reload saved"
                    }
                }
                Action {
                    purpose: ActionPurpose::Apply,
                    variant: ActionVariant::Outline,
                    disabled: apply_reason.is_some(),
                    aria_busy: command == Some(WorkflowCommand::Applying),
                    title: apply_reason.clone().unwrap_or_else(|| "Apply graph changes".to_owned()),
                    onclick: move |_| controller.apply(studio),
                    if command == Some(WorkflowCommand::Applying) { "Applying…" } else { "Apply" }
                }
                if active {
                    Action {
                        purpose: ActionPurpose::Cancel,
                        variant: ActionVariant::Outline,
                        disabled: command.is_some() || snapshot.phase == WorkflowRunPhase::Cancelling,
                        aria_busy: command == Some(WorkflowCommand::Cancelling),
                        title: "Cancel after the current Spiral operation",
                        onclick: move |_| controller.cancel(studio),
                        if snapshot.phase == WorkflowRunPhase::Cancelling || command == Some(WorkflowCommand::Cancelling) { "Cancelling…" } else { "Cancel" }
                    }
                } else {
                    Action {
                        purpose: ActionPurpose::Start,
                        variant: ActionVariant::Solid,
                        disabled: run_reason.is_some(),
                        aria_busy: command == Some(WorkflowCommand::Starting),
                        title: run_reason.clone().unwrap_or_else(|| "Run this saved workflow".to_owned()),
                        onclick: move |_| controller.run(studio),
                        if command == Some(WorkflowCommand::Starting) { "Starting…" } else { "Run workflow" }
                    }
                }
            }
            if let Some(reason) = run_reason {
                p {
                    class: if state_load_failed { "workflow-canvas-status workflow-canvas-status-error" } else { "workflow-canvas-status" },
                    role: if state_load_failed { "alert" } else { "status" },
                    "{reason}"
                }
            }
            div { class: "workflow-flow-wrap",
                Flow {
                    id: "workflow-relation-graph",
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
    let fields = if data.fields.is_empty() {
        match data.kind {
            WorkflowNodeKind::Table => "Choose a relation".to_owned(),
            WorkflowNodeKind::Model => "Stage 3 capability".to_owned(),
            WorkflowNodeKind::Query => "Read-only SQL".to_owned(),
            WorkflowNodeKind::Result => "Terminal payload".to_owned(),
        }
    } else {
        data.fields.join(" · ")
    };
    let selected_class = node
        .selected
        .then_some("workflow-node-selected")
        .unwrap_or("");
    let dragging_class = ctx
        .dragging
        .then_some("workflow-node-dragging")
        .unwrap_or("");

    rsx! {
        article {
            class: "workflow-node-card {data.kind_class()} {selected_class} {dragging_class}",
            title: data.error.clone().unwrap_or_default(),
            if data.kind != WorkflowNodeKind::Table {
                Handle { kind: HandleKind::Target, position: node.target_side }
            }
            header { class: "workflow-node-heading",
                {node_kind_icon(data.kind)}
                span { "{data.kind_label()}" }
            }
            div { class: "workflow-node-body",
                h3 { title: "{node.label}", "{node.label}" }
                p { class: "workflow-node-fields", title: "{fields}", "{fields}" }
                if let Some(relation) = &data.relation {
                    p { class: "workflow-node-relation", "reads ", code { "{relation}" } }
                }
                if let Some(output) = &data.output_relation {
                    p { class: "workflow-node-relation", "writes ", code { "{output}" } }
                }
                if let Some(sql) = &data.sql {
                    code { class: "workflow-node-sql", "{sql}" }
                }
            }
            footer { class: "workflow-node-footer",
                span { class: "workflow-node-status-dot {data.phase_class()}", "aria-hidden": "true" }
                span { "{data.phase_label()} · {data.detail}" }
            }
            if data.kind != WorkflowNodeKind::Result {
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
    let controller = workflow_controller();
    let snapshot = controller.snapshot();
    let query = controller.spec().nodes.into_iter().find_map(|node| {
        let WorkflowNodeConfig::Query { sql, max_rows } = node.config else {
            return None;
        };
        Some((sql, max_rows))
    });
    let (sql, max_rows) = query.unwrap_or_else(|| ("Read-only Workflow query".to_owned(), 100));
    let result_model = snapshot.result.clone().map(|result| QueryViewModel {
        sql: sql.clone(),
        max_rows,
        state: QueryState::Ready(result),
    });
    let transient_model = if result_model.is_none() {
        match snapshot.phase {
            WorkflowRunPhase::Validating
            | WorkflowRunPhase::Running
            | WorkflowRunPhase::Cancelling => Some(QueryViewModel {
                sql: sql.clone(),
                max_rows,
                state: QueryState::Running,
            }),
            WorkflowRunPhase::Failed | WorkflowRunPhase::Interrupted => {
                snapshot.error.clone().map(|error| QueryViewModel {
                    sql: sql.clone(),
                    max_rows,
                    state: QueryState::Failed(error),
                })
            }
            _ => None,
        }
    } else {
        None
    };

    rsx! {
        section { class: "workflow-results", "aria-label": "Workflow results",
            header { class: "workflow-results-meta",
                span { "{run_phase_label(snapshot.phase)}" }
                if let Some(result) = &snapshot.result {
                    span { class: "font-mono tabular-nums", "{result.rows.len()} rows · {result.elapsed_ms} ms" }
                    if result.truncated { span { class: "chip chip-caution", "Truncated" } }
                }
            }
            if snapshot.result_stale {
                div { class: "workflow-results-stale", role: "status",
                    strong { "Previous run" }
                    span { "These rows are stale until the current saved graph completes." }
                }
            }
            if snapshot.result.is_some() && snapshot.error.is_some() {
                div { class: "workflow-results-error", role: "alert", "{snapshot.error.clone().unwrap_or_default()}" }
            }
            div { class: "workflow-result-body",
                if let Some(model) = result_model.or(transient_model) {
                    {render_query_result(&model)}
                } else {
                    div { class: "workflow-panel-empty",
                        ResultNodeIcon {}
                        strong { "No workflow results" }
                        p { "Choose a Table relation, Apply the graph, then run its read-only Query." }
                    }
                }
            }
        }
    }
}

fn run_phase_label(phase: WorkflowRunPhase) -> &'static str {
    match phase {
        WorkflowRunPhase::Idle => "Saved",
        WorkflowRunPhase::Validating => "Validating",
        WorkflowRunPhase::Running => "Running",
        WorkflowRunPhase::Cancelling => "Cancelling",
        WorkflowRunPhase::Completed => "Complete",
        WorkflowRunPhase::Failed => "Failed",
        WorkflowRunPhase::Cancelled => "Cancelled",
        WorkflowRunPhase::Interrupted => "Interrupted",
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

    #[test]
    fn interaction_language_keeps_run_authority_out_of_assistant() {
        let source = include_str!("workspace.rs");
        let source = source.split("#[cfg(test)]").next().unwrap();
        assert!(source.contains("Assistant is not connected"));
        assert_eq!(source.matches("Run workflow").count(), 1);
        assert!(source.contains("Choose a qualified relation"));
        assert!(!source.contains("Hugging Face"));
        assert!(!source.contains("MCAP"));
    }
}
