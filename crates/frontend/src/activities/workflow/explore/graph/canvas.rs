//! The exploration graph on a Dioxus Flow canvas: the dataset on top, measurements
//! and explorations below it, laid out from the payload's edges every render.
//! The controller is the only state; the canvas keeps no second copy.
use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Edge, EdgeKind, Flow, Handle, HandleKind, MarkerKind, Node,
    NodeViewCtx, Point, Side, WorldLayer,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, DeleteIcon, InlineConfirmation, Notice, NoticeTone,
};

use super::super::document::ExplorationPayload;
use super::super::state::ExplorationController;
use super::node_card::{ExploreChildMenu, ExploreNodeCard, ExploreNodeFace};
use crate::components::flow_canvas::{
    CanvasControls, CardGraph, FlowFit, FlowMargins, ReadableViewport,
};
use crate::components::icons::{DataIcon, JoinStageIcon, MediaStageIcon, ModelStageIcon};

/// The Studio stage card's size, so a graph node reads like a Workflow stage
/// and the layout can place it before it mounts.
const NODE_WIDTH: f64 = 224.0;
const NODE_HEIGHT: f64 = 92.0;
/// How far under a card's bottom edge its popovers hang, clear of the +.
const MENU_DROP: f64 = 22.0;

/// The dataset above what it feeds, with room under each card for its + and
/// the menu it opens.
const GRAPH: CardGraph = CardGraph::top_to_bottom(28.0, 76.0);

/// Where the pane parks: the whole graph, clear of the canvas's own hairline,
/// for as long as a card stays large enough to read.
const FIT: ReadableViewport = ReadableViewport::new(FlowMargins::new(24.0, 24.0, 24.0), 0.4);

/// The nodes and edges one payload draws as.
#[derive(Clone)]
struct ExploreFlow {
    nodes: Vec<Node<ExploreNodeCard>>,
    edges: Vec<Edge>,
}

impl ExploreFlow {
    fn new(payload: &ExplorationPayload, selected: Option<&str>) -> Self {
        let mut nodes = ExploreNodeCard::all(payload)
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
        // Entrance motion follows the reading, so the graph arrives the way
        // the data flows.
        let reading = GRAPH.rank_layout(&mut nodes, &edges);
        for (node, rank) in nodes.iter_mut().zip(reading) {
            node.data = node.data.clone().with_order(rank);
        }
        Self { nodes, edges }
    }
}

/// What a node card reaches for: the graph it edits, the one + menu that may
/// be open, and where a refused edit is reported.
#[derive(Clone, Copy, PartialEq)]
struct ExploreCanvasHand {
    controller: ExplorationController,
    /// The one node whose + menu is open, and the one whose removal is being
    /// confirmed. Both are popovers, and a popover belongs to the canvas
    /// rather than to the card it hangs from — see `ExploreCanvasPopovers`.
    open_menu: Signal<Option<String>>,
    confirming: Signal<Option<String>>,
    problem: Signal<Option<String>>,
}

impl ExploreCanvasHand {
    fn use_hand(controller: ExplorationController) -> Self {
        Self {
            controller,
            open_menu: use_signal(|| None),
            confirming: use_signal(|| None),
            problem: use_signal(|| None),
        }
    }
    /// The id the removal confirmation returns focus to when it closes.
    fn remove_action(node: &str) -> String {
        format!("explore-canvas-remove-{node}")
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
        self.confirming.set(None);
    }
    fn confirm_removal(mut self, node: &str) {
        self.open_menu.set(None);
        self.confirming.set(Some(node.to_owned()));
    }
    fn select(mut self, node: String) {
        self.open_menu.set(None);
        self.confirming.set(None);
        self.controller.select_node(Some(node));
    }
    /// Add the chosen child and select it, or say why the graph refused.
    fn choose(mut self, menu: &ExploreChildMenu, index: usize) {
        self.open_menu.set(None);
        self.problem.set(menu.choose(self.controller, index).err());
    }
    fn remove(mut self, node: &str) {
        let node = node.to_owned();
        self.confirming.set(None);
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
pub(in crate::activities::workflow) fn ExploreGraphCanvas(
    controller: ExplorationController,
) -> Element {
    let hand = ExploreCanvasHand::use_hand(controller);
    let initial = use_hook(|| {
        controller.with_payload(|payload| {
            ExploreFlow::new(payload, controller.selected_node().as_deref())
        })
    });
    let mut nodes = use_signal(|| initial.nodes);
    let mut edges = use_signal(|| initial.edges);
    // The payload and the selection are the truth; the canvas follows them.
    use_effect(move || {
        let flow = controller.with_payload(|payload| {
            ExploreFlow::new(payload, controller.selected_node().as_deref())
        });
        nodes.set(flow.nodes);
        edges.set(flow.edges);
    });
    let count = nodes.read().len();
    // Explore draws no minimap; the fit reports an overflowing graph all the
    // same, and parking at the first rank is the answer either way.
    let overflowing = use_signal(|| false);
    let running = controller.activity();
    let message = hand.problem().or_else(|| controller.error());
    rsx! {
        section { class: "workflow-flow-wrap explore-canvas-graph", "aria-label": "Exploration graph",
            if let Some(message) = message {
                Notice { tone: NoticeTone::Danger, class: "inline-error explore-canvas-notice", "{message}" }
            }
            Flow { id: "explore-graph-flow", class: "workflow-flow explore-canvas-flow",
                nodes, edges, fit_view: false, fit_view_padding: 0.14,
                min_zoom: 0.4, max_zoom: 1.0, nodes_draggable: false, delete_key: false,
                // Two fingers pan as they do on the Workflow graph; a pinch
                // (or ctrl/⌘ + wheel) and the controls zoom.
                pan_on_scroll: true, zoom_on_scroll: false,
                is_valid_connection: move |_| false,
                on_pane_click: move |_| hand.close_menu(),
                on_node_click: move |id: String| hand.select(id),
                node_view: move |ctx: NodeViewCtx<ExploreNodeCard>| {
                    let status = running.get(ctx.node.id.as_str()).cloned();
                    rsx! { ExploreNodeCardView { ctx, hand, status } }
                },
                Background { variant: BackgroundVariant::Dots, gap: 22.0, size: 0.7 }
                ExploreCanvasPopovers { nodes, hand }
                FlowFit {
                    fit: FIT,
                    overflowing,
                    reveal: controller.selected_node().unwrap_or_default(),
                }
                CanvasControls {
                    label: format!("Exploration graph of {count} nodes"),
                    fit_label: "Fit the whole graph",
                }
            }
        }
    }
}

#[component]
pub(super) fn ExploreNodeGlyph(face: ExploreNodeFace) -> Element {
    match face {
        ExploreNodeFace::Dataset => rsx! { DataIcon {} },
        ExploreNodeFace::Annotation => rsx! { ModelStageIcon {} },
        ExploreNodeFace::Search => rsx! { MediaStageIcon {} },
        ExploreNodeFace::Grouping => rsx! { JoinStageIcon {} },
    }
}

/// One tab stop per node, and it is the card.
///
/// `dioxus_flow` gives every node wrapper `tabindex="0"` so its own keyboard
/// gestures can reach it — selecting, and nudging a node with the arrows. This
/// graph allows neither: the controller owns the selection and the layout owns
/// the positions, so that stop is a focus ring on an element that answers no
/// key. The card beside it is the real button, so the wrapper stands down and
/// a graph of four nodes costs four stops rather than eight.
struct ExploreCardStop;

impl ExploreCardStop {
    fn take_over(event: Event<MountedData>) {
        let mounted = event.data();
        let Some(wrapper) = mounted
            .downcast::<web_sys::Element>()
            .and_then(|card| card.parent_element())
        else {
            return;
        };
        let _ = wrapper.set_attribute("tabindex", "-1");
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
///
/// What it does not hold is either of its popovers. A node is drawn inside a
/// world tile, and a tile carries `content-visibility: auto` so the engine can
/// skip the graph it cannot see; the paint containment that comes with it
/// clips to the tile's own box. The menu hanging under a card was therefore
/// laid out, hit-testable and simply never drawn below that edge — a live
/// control nobody could see. Both popovers are rendered by
/// `ExploreCanvasPopovers`, in the world layer the crate provides for exactly
/// this.
#[component]
fn ExploreNodeCardView(
    ctx: NodeViewCtx<ExploreNodeCard>,
    hand: ExploreCanvasHand,
    #[props(default)] status: Option<String>,
) -> Element {
    let card = ctx.node.data.clone();
    let id = card.id().to_owned();
    let menu_open = hand.menu_open(&id);
    let activated = id.clone();
    rsx! {
        div {
            class: "workflow-stage-card explore-canvas-node {card.face().class_name()}",
            class: if ctx.node.selected { "is-selected" },
            style: "--stage-order: {card.order()}",
            // Selecting a node is the way into every workspace in Explore, and
            // the flow crate's own key handling reaches it only for a node it
            // is allowed to select — which this one is not, because the
            // controller owns the selection. So the card is the button.
            role: "button",
            tabindex: "0",
            "aria-current": ctx.node.selected.then_some("true"),
            onmounted: ExploreCardStop::take_over,
            onkeydown: move |event: KeyboardEvent| {
                match event.key() {
                    Key::Enter => {}
                    Key::Character(key) if key == " " => {}
                    _ => return,
                }
                event.prevent_default();
                event.stop_propagation();
                hand.select(activated.clone());
            },
            div { class: "workflow-stage-heading",
                span { class: "workflow-stage-mark", "aria-hidden": "true",
                    ExploreNodeGlyph { face: card.face() }
                }
                strong { class: "workflow-stage-title", "{card.label()}" }
            }
            p { class: "workflow-stage-summary", "{card.summary()}" }
            if let Some(status) = status {
                footer { class: "workflow-stage-footer explore-canvas-node-status", role: "status",
                    span { class: "explore-canvas-node-dot", "aria-hidden": "true" }
                    span { "{status}" }
                }
            }
            if card.removable() {
                Handle { id: "parent".to_owned(), kind: HandleKind::Target, position: Side::Top }
            }
            if card.menu().is_some() {
                Handle { id: "children".to_owned(), kind: HandleKind::Source, position: Side::Bottom }
                // The press stops here: opening the menu is not selecting the
                // node, so pressing + again closes what it opened.
                span { class: "explore-canvas-add", onpointerdown: move |event| event.stop_propagation(),
                    Button {
                        purpose: ActionPurpose::Create,
                        variant: ButtonVariant::Icon,
                        class: "explore-canvas-add-action",
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
            }
            // After the +, because a keyboard user should reach what builds
            // the graph before what deletes a branch of it. The card places it
            // back in its top corner.
            if card.removable() {
                Button {
                    id: "{ExploreCanvasHand::remove_action(&id)}",
                    purpose: ActionPurpose::Remove,
                    variant: ButtonVariant::Icon,
                    class: "explore-canvas-node-remove",
                    aria_label: "Remove {card.label()}",
                    title: "Remove this node and everything after it",
                    onclick: {
                        let id = id.clone();
                        move |_| hand.confirm_removal(&id)
                    },
                    DeleteIcon {}
                }
            }
        }
    }
}

/// The two popovers a node can open, drawn above the graph rather than inside
/// it: the + menu and the confirmation that removes a branch.
///
/// `WorldLayer` shares the viewport transform without being a tile, so a
/// popover stays fixed to its card through pan and zoom and is never clipped.
/// Only one node has either open at a time, so this draws at most one of each.
#[component]
fn ExploreCanvasPopovers(
    nodes: Signal<Vec<Node<ExploreNodeCard>>>,
    hand: ExploreCanvasHand,
) -> Element {
    let open = hand.open_menu.read().clone();
    let confirming = hand.confirming.read().clone();
    if open.is_none() && confirming.is_none() {
        return rsx! {};
    }
    let placed = |wanted: &Option<String>| {
        let wanted = wanted.as_deref()?;
        nodes
            .read()
            .iter()
            .find(|node| node.id == wanted)
            .map(|node| (node.data.clone(), node.position))
    };
    rsx! {
        WorldLayer { class: "explore-canvas-overlay".to_owned(),
            if let Some((card, at)) = placed(&open) {
                if let Some(menu) = card.menu().cloned() {
                    div {
                        class: "explore-canvas-menu",
                        role: "menu",
                        "aria-label": "Add a step after {card.label()}",
                        style: "left:{at.x + NODE_WIDTH / 2.0}px;top:{at.y + NODE_HEIGHT + MENU_DROP}px",
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
            if let Some((card, at)) = placed(&confirming) {
                div {
                    class: "explore-canvas-remove",
                    style: "left:{at.x}px;top:{at.y + NODE_HEIGHT + MENU_DROP}px",
                    onpointerdown: move |event| event.stop_propagation(),
                    InlineConfirmation {
                        id: "{ExploreCanvasHand::remove_action(card.id())}-confirmation",
                        copy: ConfirmationCopy::new(
                            format!("Remove {} and everything after it?", card.label()),
                            "Remove",
                            "Keep",
                        ),
                        response: ConfirmationResponse::new(
                            ActionPurpose::Remove,
                            {
                                let id = card.id().to_owned();
                                move |_| hand.remove(&id)
                            },
                            move |_| hand.close_menu(),
                        ),
                        dismissal: ConfirmationDismissal::new(
                            ExploreCanvasHand::remove_action(card.id()),
                        ),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::document::{
        DATASET_NODE_ID, ExplorationNodeKind, SubtaskAnnotation, SubtaskSearch,
    };
    use super::*;

    #[test]
    fn the_graph_is_laid_out_downwards_with_branches_side_by_side() {
        let mut payload = ExplorationPayload::new(None);
        let annotate = payload
            .add_child(
                DATASET_NODE_ID,
                ExplorationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        let search = payload
            .add_child(
                &annotate,
                ExplorationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )
            .unwrap();
        let group = payload
            .add_child(&annotate, ExplorationNodeKind::GroupSimilarSubtasks)
            .unwrap();
        let flow = ExploreFlow::new(&payload, Some(&search));
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
