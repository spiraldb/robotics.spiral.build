//! The curation graph on a Dioxus Flow canvas: the dataset on top, the nodes it
//! feeds below it, laid out from the payload's edges every render.
//! The controller is the only state; the canvas keeps no second copy.
use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Edge, EdgeKind, Flow, Handle, HandleKind, MarkerKind, Node,
    NodeViewCtx, Point, Side, WorldLayer, use_flow,
};
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, DeleteIcon, InlineConfirmation, Menu, Notice, NoticeTone,
};

use super::super::document::CurationGraph;
use super::super::population::PopulationReader;
use super::super::state::{CurationController, NodeActivity};
use super::canvas_controls::CanvasControls;
use super::flow_canvas::{CardGraph, FlowFit, FlowMargins, ReadableViewport};
use super::node_card::{ChildMenu, NodeCard, NodeFace, OutputFlag};
use crate::components::icons::{
    DataIcon, FilmFrameIcon, FunnelIcon, OverlapIcon, SignalStreamIcon, SparkIcon,
};

/// A node's size, so the layout can place it before it mounts: the graph
/// pane is a fifth of the window, two branches side by side have to stay
/// readable in it, and a long name takes a second line rather than the card taking more width.
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
const FIT: ReadableViewport = ReadableViewport::new(FlowMargins::new(14.0, 18.0, 44.0), 0.66);

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

/// The nodes and edges the flow draws, derived from the controller.
///
/// The flow crate owns these as signals it may write — a pane click clears
/// its own idea of the selection — so they are the controller's truth drawn
/// again: after every change the canvas makes itself, in the same frame, and
/// in an effect for every change that arrives from elsewhere.
#[derive(Clone, Copy, PartialEq)]
struct CanvasDrawing {
    nodes: Signal<Vec<Node<NodeCard>>>,
    edges: Signal<Vec<Edge>>,
}

impl CanvasDrawing {
    fn use_drawing(controller: CurationController) -> Self {
        let drawing = use_hook(|| {
            let flow = Self::flow(controller);
            Self {
                nodes: Signal::new(flow.nodes),
                edges: Signal::new(flow.edges),
            }
        });
        use_effect(move || drawing.redraw(controller));
        drawing
    }
    fn flow(controller: CurationController) -> GraphFlow {
        controller.with_reading(|payload, reader| {
            GraphFlow::new(
                payload,
                reader,
                controller.selected_node().as_deref(),
                &controller.activity(),
            )
        })
    }
    /// Draw the graph as the controller holds it now.
    fn redraw(mut self, controller: CurationController) {
        let flow = Self::flow(controller);
        self.nodes.set(flow.nodes);
        self.edges.set(flow.edges);
    }
}

/// What a node card reaches for: the graph it edits, the one + menu that may
/// be open, and where a refused edit is reported.
#[derive(Clone, Copy, PartialEq)]
struct CanvasHand {
    controller: CurationController,
    drawing: CanvasDrawing,
    /// The one node whose + menu is open, and the one whose removal is being
    /// confirmed. Both are popovers, and a popover belongs to the canvas
    /// rather than to the card it hangs from — see `CanvasPopovers`.
    open_menu: Signal<Option<String>>,
    confirming: Signal<Option<String>>,
    problem: Signal<Option<String>>,
}

impl CanvasHand {
    fn use_hand(controller: CurationController, drawing: CanvasDrawing) -> Self {
        Self {
            controller,
            drawing,
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
    /// A press on the empty pane closes what is open, and draws the
    /// selection again over the one the flow crate cleared.
    fn press_pane(self) {
        self.close_menu();
        self.drawing.redraw(self.controller);
    }
    fn confirm_removal(mut self, node: &str) {
        self.open_menu.set(None);
        self.confirming.set(Some(node.to_owned()));
    }
    fn select(mut self, node: String) {
        self.open_menu.set(None);
        self.confirming.set(None);
        self.controller.select_node(Some(node));
        self.drawing.redraw(self.controller);
    }
    /// Add the chosen child, select it, and carry focus to its card, since
    /// the menu that added it is gone; or say why the graph refused.
    fn choose(mut self, menu: &ChildMenu, index: usize) {
        self.open_menu.set(None);
        let outcome = menu.choose(self.controller, index);
        if let Ok(added) = &outcome {
            self.drawing.redraw(self.controller);
            BrowserFocus::after_render(Self::card_id(added));
        }
        self.problem.set(outcome.err());
    }
    /// Delete `node` and every step after it, and carry on from the step it
    /// hung from: the selection moves to that parent when it was in the
    /// deleted branch, and focus follows it, since the control that was
    /// pressed is gone.
    fn remove(mut self, node: &str) {
        let node = node.to_owned();
        let parent = self
            .controller
            .with_payload(|payload| payload.parent_of(&node).map(str::to_owned));
        let selected = self.controller.selected_node();
        self.confirming.set(None);
        let outcome = self.controller.edit(|payload| payload.remove_node(&node));
        if outcome.is_ok() {
            let selection_gone = selected.is_some_and(|selected| {
                self.controller
                    .with_payload(|payload| payload.node(&selected).is_none())
            });
            if let Some(parent) = parent.filter(|_| selection_gone) {
                self.controller.select_node(Some(parent.clone()));
                BrowserFocus::after_render(Self::card_id(&parent));
            }
            self.drawing.redraw(self.controller);
        }
        self.problem.set(outcome.err());
    }
    /// The id of the control a card is chosen and focused by.
    fn card_id(node: &str) -> String {
        format!("curation-canvas-card-{node}")
    }
    /// What deleting `node` asks: its name, and how many later steps go with
    /// it, since those are on the page under the question.
    fn removal_question(self, node: &str, label: &str) -> String {
        let after = self
            .controller
            .with_payload(|payload| payload.branch_of(node).len().saturating_sub(1));
        match after {
            0 => format!("Delete {label}?"),
            1 => format!("Delete {label} and the step after it?"),
            steps => format!("Delete {label} and the {steps} steps after it?"),
        }
    }
}

/// The graph pane: the canvas, its zoom and fit controls, and whatever the
/// document or the last edit needs to say.
#[component]
pub(in crate::activities::curation) fn CurationGraphCanvas(
    controller: CurationController,
) -> Element {
    let drawing = CanvasDrawing::use_drawing(controller);
    let hand = CanvasHand::use_hand(controller, drawing);
    let CanvasDrawing { nodes, edges } = drawing;
    let count = nodes.read().len();
    // Curation draws no minimap; the fit reports an overflowing graph all the
    // same, and parking at the first rank is the answer either way.
    let overflowing = use_signal(|| false);
    let message = hand.problem().or_else(|| controller.error());
    rsx! {
        section { class: "curation-canvas-graph", "aria-label": "Curation graph",
            if let Some(message) = message {
                Notice { tone: NoticeTone::Danger, class: "inline-error curation-canvas-notice", "{message}" }
            }
            Flow { id: "curation-graph-flow", class: "curation-canvas-flow",
                nodes, edges, fit_view: false, fit_view_padding: 0.14,
                min_zoom: 0.4, max_zoom: 1.0, nodes_draggable: false, delete_key: false,
                // Two fingers pan; a pinch (or ctrl/⌘ + wheel) and the controls
                // zoom.
                pan_on_scroll: true, zoom_on_scroll: false,
                is_valid_connection: move |_| false,
                on_pane_click: move |_| hand.press_pane(),
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
        NodeFace::Annotation => rsx! { SparkIcon {} },
        NodeFace::Search => rsx! { FilmFrameIcon {} },
        NodeFace::Grouping => rsx! { OverlapIcon {} },
        NodeFace::Scoring => rsx! { SignalStreamIcon {} },
        NodeFace::Select => rsx! { FunnelIcon {} },
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
            class: "curation-canvas-node {card.face().class_name()}",
            class: if ctx.node.selected { "is-selected" },
            style: "--stage-order: {card.order()}",
            onmounted: CardStop::take_over,
            // Selecting a node is the way into every workspace in Curation, and
            // the flow crate's own key handling reaches it only for a node it
            // is allowed to select — which this one is not, because the
            // controller owns the selection. The card cannot be that button
            // itself: its + and trash would be buttons inside a button, which
            // nobody reaches with a screen reader. So one button is laid over
            // the whole card, and the card's two actions sit above it.
            button {
                r#type: "button",
                id: "{CanvasHand::card_id(&id)}",
                class: "curation-canvas-node-open",
                "aria-label": "{card.label()}: {card.summary()}",
                "aria-current": ctx.node.selected.then_some("true"),
                onclick: move |_| hand.select(activated.clone()),
            }
            // An output wears its flag on the card's edge rather than in the
            // heading, so the name keeps the heading's width and the flag
            // never sits under the card's remove control.
            match card.output() {
                Some(OutputFlag::Unpublished) => rsx! {
                    span {
                        class: "curation-canvas-output",
                        title: "One of this curation's outputs: publish it from its workspace",
                        "Output"
                    }
                },
                Some(OutputFlag::Published(published)) if published.is_current(hand.controller.saved_revision()) => rsx! {
                    span {
                        class: "curation-canvas-output",
                        title: "Published as the catalog view {published.view()}",
                        "Published"
                    }
                },
                Some(OutputFlag::Published(published)) => rsx! {
                    span {
                        class: "curation-canvas-output is-stale",
                        title: "The saved curation has changed since {published.view()} was published; publish it again from its workspace",
                        "Out of date"
                    }
                },
                None => rsx! {},
            }
            div { class: "curation-canvas-heading",
                span { class: "curation-canvas-mark", "aria-hidden": "true",
                    NodeGlyph { face: card.face() }
                }
                strong { class: "curation-canvas-title", "{card.label()}" }
            }
            // While it runs, what the node is doing replaces what it holds:
            // the run line carries the live count, so the card says it once.
            if card.is_running() {
                footer { class: "curation-canvas-footer curation-canvas-node-status",
                    span { class: "curation-canvas-node-dot", "aria-hidden": "true" }
                    span { "{card.summary()}" }
                }
            } else {
                p { class: "curation-canvas-summary", "{card.summary()}" }
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
            // The press stops here too: asking to delete a step is not
            // opening it, so the workspace is not rebuilt for a node that is
            // about to go.
            if card.removable() {
                span { class: "curation-canvas-node-remove-slot", onpointerdown: move |event| event.stop_propagation(),
                    Button {
                        id: "{CanvasHand::remove_action(&id)}",
                        purpose: ActionPurpose::Remove,
                        variant: ButtonVariant::Icon,
                        class: "curation-canvas-node-remove",
                        aria_label: "Delete {card.label()}",
                        title: "Delete this step and every step after it",
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
            // Over the card it deletes, in the card's own box: beside what it
            // is about, and never over the steps it will delete with it.
            if let Some((card, at)) = placed(&confirming) {
                div {
                    class: "curation-canvas-remove",
                    style: "left:{at.x}px;top:{at.y}px;width:{NODE_WIDTH}px;min-height:{NODE_HEIGHT}px",
                    onpointerdown: move |event| event.stop_propagation(),
                    InlineConfirmation {
                        id: "{CanvasHand::remove_action(card.id())}-confirmation",
                        copy: ConfirmationCopy::new(
                            hand.removal_question(card.id(), card.label()),
                            "Delete",
                            "Keep",
                        ),
                        response: ConfirmationResponse::new(
                            ActionPurpose::Delete,
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
