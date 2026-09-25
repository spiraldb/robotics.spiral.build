//! The curation graph on a Dioxus Flow canvas: the dataset on top, the nodes it
//! feeds below it, laid out from the payload's edges every render.
//! The controller is the only state; the canvas keeps no second copy.
use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Edge, EdgeKind, Flow, Handle, HandleKind, MarkerKind, Node,
    NodeViewCtx, Point, Side, WorldLayer, use_flow,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, DeleteIcon, InlineConfirmation, Menu, Notice, NoticeTone,
};

use super::super::document::CurationGraph;
use super::super::population::PopulationReader;
use super::super::state::{CurationController, NodeActivity};
use super::node_card::{ChildMenu, NodeCard, NodeFace, OutputFlag};
use crate::components::flow_canvas::{
    CanvasControls, CardGraph, FlowFit, FlowMargins, ReadableViewport,
};
use crate::components::icons::{
    DataIcon, JoinStageIcon, MediaStageIcon, ModelStageIcon, QueryNodeIcon, SignalStreamIcon,
};

/// A node's size, so the layout can place it before it mounts: the graph
/// pane is a fifth of the window, two branches side by side have to stay
/// readable in it, and the longest name ("Identify unproductive moments")
/// takes a second line rather than the card taking more width.
const NODE_WIDTH: f64 = 192.0;
const NODE_HEIGHT: f64 = 100.0;
/// How far under a card's bottom edge its popovers hang, clear of the +.
const MENU_DROP: f64 = 18.0;

/// The dataset above what it feeds, with room under each card for its + and
/// no more: the menu the + opens floats over what is below.
const GRAPH: CardGraph = CardGraph::top_to_bottom(20.0, 40.0);

/// Where the pane parks: the whole graph, clear of the canvas's own hairline,
/// for as long as a card stays large enough to read. Past that the graph is
/// panned rather than shrunk into cards nobody can read.
const FIT: ReadableViewport =
    ReadableViewport::new(FlowMargins::new(14.0, 18.0, 44.0), 0.66).showing_most();

/// The nodes and edges one payload draws as.
#[derive(Clone)]
struct GraphFlow {
    nodes: Vec<Node<NodeCard>>,
    edges: Vec<Edge>,
}

impl GraphFlow {
    fn new(
        payload: &CurationGraph,
        reader: Option<&PopulationReader<'_>>,
        selected: Option<&str>,
        activity: &NodeActivity,
    ) -> Self {
        let mut nodes = NodeCard::all(payload, reader)
            .into_iter()
            .map(|card| {
                let status = activity.get(card.id()).cloned();
                let card = card.running(status.as_ref());
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
struct CanvasHand {
    controller: CurationController,
    /// The one node whose + menu is open, and the one whose removal is being
    /// confirmed. Both are popovers, and a popover belongs to the canvas
    /// rather than to the card it hangs from — see `CanvasPopovers`.
    open_menu: Signal<Option<String>>,
    confirming: Signal<Option<String>>,
    problem: Signal<Option<String>>,
}

impl CanvasHand {
    fn use_hand(controller: CurationController) -> Self {
        Self {
            controller,
            open_menu: use_signal(|| None),
            confirming: use_signal(|| None),
            problem: use_signal(|| None),
        }
    }
    /// The id the removal confirmation returns focus to when it closes.
    fn remove_action(node: &str) -> String {
        format!("curation-canvas-remove-{node}")
    }
    /// The id the + menu returns focus to when Escape closes it.
    fn add_action(node: &str) -> String {
        format!("curation-canvas-add-{node}")
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
    fn choose(mut self, menu: &ChildMenu, index: usize) {
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
pub(in crate::activities::curation) fn CurationGraphCanvas(
    controller: CurationController,
) -> Element {
    let hand = CanvasHand::use_hand(controller);
    let initial = use_hook(|| {
        controller.with_reading(|payload, reader| {
            GraphFlow::new(
                payload,
                reader,
                controller.selected_node().as_deref(),
                &controller.activity(),
            )
        })
    });
    let mut nodes = use_signal(|| initial.nodes);
    let mut edges = use_signal(|| initial.edges);
    // The payload, the selection, and what is running are the truth; the
    // canvas follows them.
    use_effect(move || {
        let flow = controller.with_reading(|payload, reader| {
            GraphFlow::new(
                payload,
                reader,
                controller.selected_node().as_deref(),
                &controller.activity(),
            )
        });
        nodes.set(flow.nodes);
        edges.set(flow.edges);
    });
    let count = nodes.read().len();
    // Curation draws no minimap; the fit reports an overflowing graph all the
    // same, and parking at the first rank is the answer either way.
    let overflowing = use_signal(|| false);
    let message = hand.problem().or_else(|| controller.error());
    rsx! {
        section { class: "workflow-flow-wrap curation-canvas-graph", "aria-label": "Curation graph",
            if let Some(message) = message {
                Notice { tone: NoticeTone::Danger, class: "inline-error curation-canvas-notice", "{message}" }
            }
            Flow { id: "curation-graph-flow", class: "workflow-flow curation-canvas-flow",
                nodes, edges, fit_view: false, fit_view_padding: 0.14,
                min_zoom: 0.4, max_zoom: 1.0, nodes_draggable: false, delete_key: false,
                // Two fingers pan; a pinch (or ctrl/⌘ + wheel) and the controls
                // zoom.
                pan_on_scroll: true, zoom_on_scroll: false,
                is_valid_connection: move |_| false,
                on_pane_click: move |_| hand.close_menu(),
                on_node_click: move |id: String| hand.select(id),
                node_view: move |ctx: NodeViewCtx<NodeCard>| rsx! { NodeCardView { ctx, hand } },
                Background { variant: BackgroundVariant::Dots, gap: 22.0, size: 0.7 }
                CanvasPopovers { nodes, hand }
                FlowFit {
                    fit: FIT,
                    overflowing,
                    reveal: controller.selected_node().unwrap_or_default(),
                }
                CanvasControls {
                    label: format!("Curation graph of {count} nodes"),
                    fit_label: "Fit the whole graph",
                }
            }
        }
    }
}

#[component]
pub(in crate::activities::curation) fn NodeGlyph(face: NodeFace) -> Element {
    match face {
        NodeFace::Dataset => rsx! { DataIcon {} },
        NodeFace::Annotation => rsx! { ModelStageIcon {} },
        NodeFace::Search => rsx! { MediaStageIcon {} },
        NodeFace::Grouping => rsx! { JoinStageIcon {} },
        NodeFace::Detection => rsx! { SignalStreamIcon {} },
        NodeFace::Select => rsx! { QueryNodeIcon {} },
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
struct CardStop;

impl CardStop {
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
/// runs — one quiet line from the view that runs it in place of what it holds.
///
/// What it does not hold is either of its popovers. A node is drawn inside a
/// world tile, and a tile carries `content-visibility: auto` so the engine can
/// skip the graph it cannot see; the paint containment that comes with it
/// clips to the tile's own box. The menu hanging under a card was therefore
/// laid out, hit-testable and simply never drawn below that edge — a live
/// control nobody could see. Both popovers are rendered by
/// `CanvasPopovers`, in the world layer the crate provides for exactly
/// this.
#[component]
fn NodeCardView(ctx: NodeViewCtx<NodeCard>, hand: CanvasHand) -> Element {
    let card = ctx.node.data.clone();
    let id = card.id().to_owned();
    let menu_open = hand.menu_open(&id);
    let activated = id.clone();
    rsx! {
        div {
            class: "workflow-stage-card curation-canvas-node {card.face().class_name()}",
            class: if ctx.node.selected { "is-selected" },
            style: "--stage-order: {card.order()}",
            // Selecting a node is the way into every workspace in Curation, and
            // the flow crate's own key handling reaches it only for a node it
            // is allowed to select — which this one is not, because the
            // controller owns the selection. So the card is the button.
            role: "button",
            tabindex: "0",
            "aria-current": ctx.node.selected.then_some("true"),
            onmounted: CardStop::take_over,
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
            // An output wears its flag on the card's edge rather than in the
            // heading, so the name keeps the heading's width and the flag
            // never sits under the card's remove control.
            match card.output() {
                Some(OutputFlag::Unpublished) => rsx! {
                    span {
                        class: "workflow-stage-output-flag curation-canvas-output",
                        title: "One of this curation's outputs: publish it from its workspace",
                        "Output"
                    }
                },
                Some(OutputFlag::Published(published)) if published.is_current(hand.controller.saved_revision()) => rsx! {
                    span {
                        class: "workflow-stage-output-flag curation-canvas-output",
                        title: "Published as the catalog view {published.view()}",
                        "Published"
                    }
                },
                Some(OutputFlag::Published(published)) => rsx! {
                    span {
                        class: "workflow-stage-output-flag curation-canvas-output is-stale",
                        title: "The saved curation has changed since {published.view()} was published; publish it again from its workspace",
                        "Out of date"
                    }
                },
                None => rsx! {},
            }
            div { class: "workflow-stage-heading",
                span { class: "workflow-stage-mark", "aria-hidden": "true",
                    NodeGlyph { face: card.face() }
                }
                strong { class: "workflow-stage-title", "{card.label()}" }
            }
            // While it runs, what the node is doing replaces what it holds:
            // the run line carries the live count, so the card says it once.
            if card.is_running() {
                footer { class: "workflow-stage-footer curation-canvas-node-status",
                    span { class: "curation-canvas-node-dot", "aria-hidden": "true" }
                    span { "{card.summary()}" }
                }
            } else {
                p { class: "workflow-stage-summary", "{card.summary()}" }
            }
            if card.removable() {
                Handle { id: "parent".to_owned(), kind: HandleKind::Target, position: Side::Top }
            }
            if card.menu().is_some() {
                Handle { id: "children".to_owned(), kind: HandleKind::Source, position: Side::Bottom }
                // The press stops here: opening the menu is not selecting the
                // node, so pressing + again closes what it opened.
                span { class: "curation-canvas-add", onpointerdown: move |event| event.stop_propagation(),
                    Button {
                        id: "{CanvasHand::add_action(&id)}",
                        purpose: ActionPurpose::Create,
                        variant: ButtonVariant::Icon,
                        class: "curation-canvas-add-action",
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
                    id: "{CanvasHand::remove_action(&id)}",
                    purpose: ActionPurpose::Remove,
                    variant: ButtonVariant::Icon,
                    class: "curation-canvas-node-remove",
                    aria_label: "Remove {card.label()}",
                    title: "Remove this step and everything after it",
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
/// Each is scaled back by the zoom: it hangs from a card in the graph, but
/// it is read at the size of every other menu in Studio.
#[component]
fn CanvasPopovers(nodes: Signal<Vec<Node<NodeCard>>>, hand: CanvasHand) -> Element {
    let zoom = use_flow().viewport.read().zoom;
    let open = hand.open_menu.read().clone();
    let confirming = hand.confirming.read().clone();
    if open.is_none() && confirming.is_none() {
        return rsx! {};
    }
    let screen = format!("--screen-scale: {:.4}", 1.0 / zoom.max(0.05));
    let placed = |wanted: &Option<String>| {
        let wanted = wanted.as_deref()?;
        nodes
            .read()
            .iter()
            .find(|node| node.id == wanted)
            .map(|node| (node.data.clone(), node.position))
    };
    rsx! {
        WorldLayer { class: "curation-canvas-overlay".to_owned(),
            if let Some((card, at)) = placed(&open) {
                if let Some(menu) = card.menu().cloned() {
                    div {
                        class: "curation-canvas-menu",
                        style: "left:{at.x + NODE_WIDTH / 2.0}px;top:{at.y + NODE_HEIGHT + MENU_DROP}px;{screen}",
                        onpointerdown: move |event| event.stop_propagation(),
                        Menu {
                            label: format!("Add a step after {}", card.label()),
                            opener: CanvasHand::add_action(card.id()),
                            on_close: move |()| hand.close_menu(),
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
            }
            if let Some((card, at)) = placed(&confirming) {
                div {
                    class: "curation-canvas-remove",
                    style: "left:{at.x}px;top:{at.y + NODE_HEIGHT + MENU_DROP}px;{screen}",
                    onpointerdown: move |event| event.stop_propagation(),
                    InlineConfirmation {
                        id: "{CanvasHand::remove_action(card.id())}-confirmation",
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
                            CanvasHand::remove_action(card.id()),
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
        CurationNodeKind, DATASET_NODE_ID, SubtaskAnnotation, SubtaskSearch,
    };
    use super::*;

    #[test]
    fn the_graph_is_laid_out_downwards_with_branches_side_by_side() {
        let mut payload = CurationGraph::new(None);
        let annotate = payload
            .add_child(
                DATASET_NODE_ID,
                CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        let search = payload
            .add_child(
                &annotate,
                CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )
            .unwrap();
        let group = payload
            .add_child(
                &annotate,
                CurationNodeKind::GroupSimilarSubtasks(Default::default()),
            )
            .unwrap();
        let flow = GraphFlow::new(&payload, None, Some(&search), &NodeActivity::new());
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
