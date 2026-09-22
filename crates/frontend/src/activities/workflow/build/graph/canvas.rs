//! The Build graph on a Dioxus Flow canvas: the dataset on top, measurements
//! and explorations below it, laid out from the payload's edges every render.
//! The controller is the only state; the canvas keeps no second copy.
use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Edge, EdgeKind, Flow, Handle, HandleKind, LayoutDirection,
    LayoutOptions, MarkerKind, Node, NodeViewCtx, Point, Side, compute_layout,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, DeleteIcon, InlineConfirmation, Notice, NoticeTone,
};

use super::super::document::BuildPayload;
use super::super::state::BuildController;
use super::node_card::{BuildChildMenu, BuildNodeCard, BuildNodeFace};
use crate::components::canvas_controls::CanvasControls;
use crate::components::icons::{
    DataIcon, JoinStageIcon, MediaStageIcon, ModelStageIcon, QueryNodeIcon,
};

/// The Studio stage card's size, so a Build node reads like a Workflow stage
/// and the layout can place it before it mounts.
const NODE_WIDTH: f64 = 224.0;
const NODE_HEIGHT: f64 = 92.0;
/// Room under a card for its + and the menu it opens.
const RANK_GAP: f64 = 76.0;
const BRANCH_GAP: f64 = 28.0;

/// One quiet line per node id while something runs. A run in flight is not in
/// the payload, so the measurement views own these lines and hand them to the
/// canvas; the slot stays empty until they do.
pub(in crate::activities::workflow) type BuildRunLines = BTreeMap<String, String>;

/// The nodes and edges one payload draws as.
#[derive(Clone)]
struct BuildFlow {
    nodes: Vec<Node<BuildNodeCard>>,
    edges: Vec<Edge>,
}

impl BuildFlow {
    fn new(payload: &BuildPayload, selected: Option<&str>) -> Self {
        let mut nodes = BuildNodeCard::all(payload)
            .into_iter()
            .map(|card| {
                let id = card.id().to_owned();
                let selected = selected == Some(id.as_str());
                let mut node = Node::with_data(id, card.label(), Point::ZERO, card)
                    .size((NODE_WIDTH, NODE_HEIGHT))
                    .sides(Side::Top, Side::Bottom)
                    .draggable(false);
                // The controller owns what is selected, so the canvas never
                // selects a node behind its back: a click reports, and the
                // controller decides.
                node.selectable = false;
                node.selected = selected;
                node
            })
            .collect::<Vec<_>>();
        let edges = payload
            .edges()
            .iter()
            .map(|edge| {
                Edge::new(edge.from(), edge.to())
                    .id(format!("{}:{}", edge.from(), edge.to()))
                    .kind(EdgeKind::SmoothStep)
                    .marker_end(MarkerKind::None)
                    .source_handle("children")
                    .target_handle("parent")
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
        let positions = compute_layout(
            &geometry,
            &connections,
            &LayoutOptions::default()
                .direction(LayoutDirection::TopToBottom)
                .gaps(BRANCH_GAP, RANK_GAP),
        );
        for node in &mut nodes {
            if let Some(position) = positions.get(&node.id) {
                node.position = *position;
            }
        }
        // Reading order follows the layout: top to bottom, then left to right.
        let mut order = nodes
            .iter()
            .map(|node| (node.position.y, node.position.x, node.id.clone()))
            .collect::<Vec<_>>();
        order.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.total_cmp(&right.1)));
        for (rank, (_, _, id)) in order.into_iter().enumerate() {
            if let Some(node) = nodes.iter_mut().find(|node| node.id == id) {
                node.data = node.data.clone().with_order(rank);
            }
        }
        Self { nodes, edges }
    }
}

/// What a node card reaches for: the graph it edits, the one + menu that may
/// be open, and where a refused edit is reported.
#[derive(Clone, Copy, PartialEq)]
struct BuildCanvasHand {
    controller: BuildController,
    open_menu: Signal<Option<String>>,
    problem: Signal<Option<String>>,
}

impl BuildCanvasHand {
    fn use_hand(controller: BuildController) -> Self {
        Self {
            controller,
            open_menu: use_signal(|| None),
            problem: use_signal(|| None),
        }
    }
    fn menu_open(self, node: &str) -> bool {
        self.open_menu.read().as_deref() == Some(node)
    }
    /// Why the last edit was refused, if it was.
    fn problem(self) -> Option<String> {
        self.problem.read().clone()
    }
    fn toggle_menu(mut self, node: &str) {
        let open = self.menu_open(node);
        self.open_menu.set((!open).then(|| node.to_owned()));
    }
    fn close_menu(mut self) {
        self.open_menu.set(None);
    }
    fn select(mut self, node: String) {
        self.open_menu.set(None);
        self.controller.select_node(Some(node));
    }
    /// Add the chosen child and select it, or say why the graph refused.
    fn choose(mut self, menu: &BuildChildMenu, index: usize) {
        self.open_menu.set(None);
        self.problem.set(menu.choose(self.controller, index).err());
    }
    fn remove(mut self, node: &str) {
        let node = node.to_owned();
        self.problem.set(
            self.controller
                .edit(|payload| payload.remove_node(&node))
                .err(),
        );
    }
}

/// The graph pane: the canvas, its zoom and fit controls, and whatever the
/// document or the last edit needs to say.
#[component]
pub(in crate::activities::workflow) fn BuildGraphCanvas(
    controller: BuildController,
    /// A quiet line per node while a measurement runs, from the view that
    /// runs it. Empty until a measurement fills it.
    #[props(default)]
    running: BuildRunLines,
) -> Element {
    let hand = BuildCanvasHand::use_hand(controller);
    let initial = use_hook(|| {
        controller
            .with_payload(|payload| BuildFlow::new(payload, controller.selected_node().as_deref()))
    });
    let mut nodes = use_signal(|| initial.nodes);
    let mut edges = use_signal(|| initial.edges);
    // The payload and the selection are the truth; the canvas follows them.
    use_effect(move || {
        let flow = controller
            .with_payload(|payload| BuildFlow::new(payload, controller.selected_node().as_deref()));
        nodes.set(flow.nodes);
        edges.set(flow.edges);
    });
    let count = nodes.read().len();
    let message = hand.problem().or_else(|| controller.error());
    rsx! {
        section { class: "workflow-flow-wrap build-canvas-graph", "aria-label": "Build graph",
            if let Some(message) = message {
                Notice { tone: NoticeTone::Danger, class: "inline-error build-canvas-notice", "{message}" }
            }
            Flow { id: "build-graph-flow", class: "workflow-flow build-canvas-flow",
                nodes, edges, fit_view: true, fit_view_padding: 0.14,
                min_zoom: 0.4, max_zoom: 1.25, nodes_draggable: false, delete_key: false,
                // Two fingers pan as they do on the Workflow graph; a pinch
                // (or ctrl/⌘ + wheel) and the controls zoom.
                pan_on_scroll: true, zoom_on_scroll: false,
                is_valid_connection: move |_| false,
                on_pane_click: move |_| hand.close_menu(),
                on_node_click: move |id: String| hand.select(id),
                node_view: move |ctx: NodeViewCtx<BuildNodeCard>| {
                    let status = running.get(ctx.node.id.as_str()).cloned();
                    rsx! { BuildNodeCardView { ctx, hand, status } }
                },
                Background { variant: BackgroundVariant::Dots, gap: 22.0, size: 0.7 }
                BuildCanvasFit { count }
                CanvasControls {
                    label: format!("Build graph of {count} nodes"),
                    fit_label: "Fit the whole graph",
                }
            }
        }
    }
}

/// Fit the graph whenever it gains or loses a node; between those moments the
/// user's own pan and zoom are left alone.
#[component]
fn BuildCanvasFit(count: usize) -> Element {
    let core = dioxus_flow::use_flow();
    use_effect(use_reactive((&count,), move |_| core.fit_view(240)));
    rsx! {}
}

#[component]
pub(super) fn BuildNodeGlyph(face: BuildNodeFace) -> Element {
    match face {
        BuildNodeFace::Dataset => rsx! { DataIcon {} },
        BuildNodeFace::Annotation => rsx! { ModelStageIcon {} },
        BuildNodeFace::Progress => rsx! { QueryNodeIcon {} },
        BuildNodeFace::Search => rsx! { MediaStageIcon {} },
        BuildNodeFace::Grouping => rsx! { JoinStageIcon {} },
    }
}

/// The + that opens a node's child menu: a hairline cross, never a label.
#[component]
fn AddGlyph() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M8 4.5v7M4.5 8h7" }
        }
    }
}

/// One node: what it is, what it holds, what may follow it, and — while it
/// runs — one quiet line from the view that runs it.
#[component]
fn BuildNodeCardView(
    ctx: NodeViewCtx<BuildNodeCard>,
    hand: BuildCanvasHand,
    #[props(default)] status: Option<String>,
) -> Element {
    let card = ctx.node.data.clone();
    let mut removing = use_signal(|| false);
    let id = card.id().to_owned();
    let menu = card.menu().cloned();
    let menu_open = hand.menu_open(&id);
    let remove_action = format!("build-canvas-remove-{id}");
    rsx! {
        div {
            class: "workflow-stage-card build-canvas-node {card.face().class_name()}",
            class: if ctx.node.selected { "is-selected" },
            class: if !card.executable() { "is-planned" },
            style: "--stage-order: {card.order()}",
            div { class: "workflow-stage-heading",
                span { class: "workflow-stage-mark", "aria-hidden": "true",
                    BuildNodeGlyph { face: card.face() }
                }
                strong { class: "workflow-stage-title", "{card.label()}" }
                if card.removable() {
                    Button {
                        id: "{remove_action}",
                        purpose: ActionPurpose::Remove,
                        variant: ButtonVariant::Icon,
                        class: "build-canvas-node-remove",
                        aria_label: "Remove {card.label()}",
                        title: "Remove this node and everything after it",
                        onclick: move |_| removing.set(true),
                        DeleteIcon {}
                    }
                }
            }
            p { class: "workflow-stage-summary", title: "{card.summary()}", "{card.summary()}" }
            if let Some(status) = status {
                footer { class: "workflow-stage-footer build-canvas-node-status", role: "status",
                    span { class: "build-canvas-node-dot", "aria-hidden": "true" }
                    span { "{status}" }
                }
            }
            if card.removable() {
                Handle { id: "parent".to_owned(), kind: HandleKind::Target, position: Side::Top }
            }
            if let Some(menu) = menu {
                Handle { id: "children".to_owned(), kind: HandleKind::Source, position: Side::Bottom }
                // The press stops here: opening the menu is not selecting the
                // node, so pressing + again closes what it opened.
                span { class: "build-canvas-add", onpointerdown: move |event| event.stop_propagation(),
                    Button {
                        purpose: ActionPurpose::Create,
                        variant: ButtonVariant::Icon,
                        class: "build-canvas-add-action",
                        aria_haspopup: "menu",
                        aria_expanded: menu_open,
                        aria_label: "Add a step after {card.label()}",
                        title: "Add a step",
                        onclick: {
                            let id = id.clone();
                            move |_| hand.toggle_menu(&id)
                        },
                        AddGlyph {}
                    }
                }
                if menu_open {
                    div {
                        class: "build-canvas-menu",
                        role: "menu",
                        "aria-label": "Add a step after {card.label()}",
                        onpointerdown: move |event| event.stop_propagation(),
                        onkeydown: move |event: KeyboardEvent| {
                            if event.key() == Key::Escape {
                                event.prevent_default();
                                hand.close_menu();
                            }
                        },
                        for (index, label) in menu.labels().into_iter().enumerate() {
                            Button {
                                key: "{label}",
                                purpose: ActionPurpose::Create,
                                variant: ButtonVariant::Row,
                                role: "menuitem",
                                onclick: {
                                    let menu = menu.clone();
                                    move |_| hand.choose(&menu, index)
                                },
                                "{label}"
                            }
                        }
                    }
                }
            }
            if removing() {
                div { class: "build-canvas-remove", onpointerdown: move |event| event.stop_propagation(),
                    InlineConfirmation {
                        id: "{remove_action}-confirmation",
                        copy: ConfirmationCopy::new(
                            format!("Remove {} and everything after it?", card.label()),
                            "Remove",
                            "Keep",
                        ),
                        response: ConfirmationResponse::new(
                            ActionPurpose::Remove,
                            {
                                let id = id.clone();
                                move |_| {
                                    removing.set(false);
                                    hand.remove(&id);
                                }
                            },
                            move |_| removing.set(false),
                        ),
                        dismissal: ConfirmationDismissal::new(remove_action.clone()),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::document::{
        BuildNodeKind, DATASET_NODE_ID, SubtaskAnnotation, SubtaskSearch,
    };
    use super::*;

    #[test]
    fn the_graph_is_laid_out_downwards_with_branches_side_by_side() {
        let mut payload = BuildPayload::new(None);
        let annotate = payload
            .add_child(
                DATASET_NODE_ID,
                BuildNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        let search = payload
            .add_child(
                &annotate,
                BuildNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )
            .unwrap();
        let group = payload
            .add_child(&annotate, BuildNodeKind::GroupSimilarSubtasks)
            .unwrap();
        let flow = BuildFlow::new(&payload, Some(&search));
        let node = |id: &str| {
            flow.nodes
                .iter()
                .find(|node| node.id == id)
                .expect("every payload node is drawn")
                .clone()
        };
        assert_eq!(flow.nodes.len(), 4);
        assert_eq!(flow.edges.len(), 3);
        assert!(node(DATASET_NODE_ID).position.y < node(&annotate).position.y);
        assert!(node(&annotate).position.y < node(&search).position.y);
        assert_eq!(node(&search).position.y, node(&group).position.y);
        assert_ne!(node(&search).position.x, node(&group).position.x);
        assert!(node(&search).selected && !node(&group).selected);
        assert!(flow.nodes.iter().all(|node| !node.draggable));
        assert_eq!(node(DATASET_NODE_ID).data.order(), 0);
        assert_eq!(node(&annotate).data.order(), 1);
        let edge = flow
            .edges
            .iter()
            .find(|edge| edge.target == annotate)
            .unwrap();
        assert_eq!(edge.source, DATASET_NODE_ID);
        assert_eq!(edge.source_handle.as_deref(), Some("children"));
        assert_eq!(edge.target_handle.as_deref(), Some("parent"));
    }
}
