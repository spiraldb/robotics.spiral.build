//! What every node canvas in Studio does with a graph of cards: rank it, park
//! the viewport where it can be read, and hang a detail sheet off a selection.
//!
//! Workflow's query plan and Curation's graph both draw the same card on the
//! same `dioxus_flow` canvas. They differ in
//! what a card says, never in how the canvas behaves, and that behaviour is
//! here: [`CardGraph`] places the cards and hands back their reading order,
//! [`FlowFit`] decides what the pane shows, and [`PopoverPlacement`] puts the
//! sheet somewhere it fits.
//!
//! [`CanvasControls`] — the zoom and fit group at the bottom left — is
//! re-exported here so a canvas reaches for one module, not two.

use dioxus::prelude::*;
use dioxus_flow::{
    Edge, LayoutDirection, LayoutNode, LayoutOptions, Node, Point, Rect, Viewport, compute_layout,
    use_flow,
};

pub(crate) use super::canvas_controls::CanvasControls;

/// How a graph of cards is arranged: which way its ranks run, and how much air
/// sits between the cards within a rank and between the ranks themselves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CardGraph {
    direction: LayoutDirection,
    node_gap: f64,
    rank_gap: f64,
}

impl CardGraph {
    /// Ranks running left to right: sources at the left, the output at the
    /// right, the way a pipeline reads.
    pub(crate) const fn left_to_right(node_gap: f64, rank_gap: f64) -> Self {
        Self {
            direction: LayoutDirection::LeftToRight,
            node_gap,
            rank_gap,
        }
    }

    /// Ranks running top to bottom: the root above, what it feeds below.
    pub(crate) const fn top_to_bottom(node_gap: f64, rank_gap: f64) -> Self {
        Self {
            direction: LayoutDirection::TopToBottom,
            node_gap,
            rank_gap,
        }
    }

    /// Position `nodes` along the ranks `edges` imply, and answer with each
    /// node's reading rank, in the order the nodes were given: `order[i]` is
    /// where `nodes[i]` comes in the reading. Entrance motion follows that rank,
    /// so the graph arrives the way the data flows.
    ///
    /// The nodes must already carry their sizes; only their positions change.
    pub(crate) fn rank_layout<T>(self, nodes: &mut [Node<T>], edges: &[Edge]) -> Vec<usize> {
        let geometry = nodes
            .iter()
            .map(|node| LayoutNode {
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
                .direction(self.direction)
                .gaps(self.node_gap, self.rank_gap),
        );
        for node in nodes.iter_mut() {
            if let Some(position) = positions.get(&node.id) {
                node.position = *position;
            }
        }
        self.reading_order(nodes)
    }

    fn reading_order<T>(self, nodes: &[Node<T>]) -> Vec<usize> {
        let mut placed = (0..nodes.len()).collect::<Vec<_>>();
        placed.sort_by(|left, right| {
            let left = self.reading_key(nodes[*left].position);
            let right = self.reading_key(nodes[*right].position);
            left.0.total_cmp(&right.0).then(left.1.total_cmp(&right.1))
        });
        let mut order = vec![0; nodes.len()];
        for (rank, node) in placed.into_iter().enumerate() {
            order[node] = rank;
        }
        order
    }

    /// Reading advances along the ranks first and across them second, so a
    /// horizontal graph reads left to right and a vertical one top to bottom.
    const fn reading_key(self, at: Point) -> (f64, f64) {
        match self.direction {
            LayoutDirection::LeftToRight | LayoutDirection::RightToLeft => (at.x, at.y),
            LayoutDirection::TopToBottom | LayoutDirection::BottomToTop => (at.y, at.x),
        }
    }
}

/// The room a canvas keeps clear of its own furniture: the hairline at its
/// edges, and the strip at the bottom where the zoom controls sit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlowMargins {
    sides: f64,
    top: f64,
    bottom: f64,
}

impl FlowMargins {
    pub(crate) const fn new(sides: f64, top: f64, bottom: f64) -> Self {
        Self { sides, top, bottom }
    }

    /// The part of `container` a graph may occupy.
    fn safe(self, container: Rect) -> Rect {
        Rect::new(
            self.sides,
            self.top,
            (container.width - 2.0 * self.sides).max(1.0),
            (container.height - self.top - self.bottom).max(1.0),
        )
    }
}

/// Where a canvas parks for one graph.
///
/// The whole graph, when it stays readable — a card shrunk past `min_zoom` is a
/// card nobody can read, and a graph wider than the pane is going to be panned
/// anyway. Past that point the canvas keeps its cards at the largest size the
/// pane's height allows and starts at the first rank, so the operator begins
/// where the data does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ReadableViewport {
    margins: FlowMargins,
    min_zoom: f64,
    overflow: OverflowZoom,
}

/// The zoom a graph too big to show whole parks at.
#[derive(Clone, Copy, Debug, PartialEq)]
enum OverflowZoom {
    /// As large as the pane's height allows: a pipeline running off to the
    /// right is read one stage at a time anyway.
    FitHeight,
    /// As small as a card stays readable: a graph branching sideways shows
    /// as many of its branches as it can.
    Smallest,
}

impl ReadableViewport {
    pub(crate) const fn new(margins: FlowMargins, min_zoom: f64) -> Self {
        Self {
            margins,
            min_zoom,
            overflow: OverflowZoom::FitHeight,
        }
    }

    /// Park an overflowing graph at the smallest readable zoom rather than the
    /// largest its height allows — the right answer for a tree read downwards,
    /// whose overflow is its branches side by side.
    pub(crate) const fn showing_most(mut self) -> Self {
        self.overflow = OverflowZoom::Smallest;
        self
    }

    /// Where to park for a graph of `bounds` inside `container`, and whether the
    /// graph runs off the pane at that zoom — which is when a canvas earns its
    /// minimap.
    fn park(self, bounds: Rect, container: Rect) -> (Viewport, bool) {
        let safe = self.margins.safe(container);
        let whole = (safe.width / bounds.width).min(safe.height / bounds.height);
        if whole >= self.min_zoom {
            return (Viewport::fit(Some(bounds), safe, self.min_zoom, 1.0), false);
        }
        let zoom = match self.overflow {
            OverflowZoom::FitHeight => (safe.height / bounds.height).clamp(self.min_zoom, 1.0),
            OverflowZoom::Smallest => self.min_zoom,
        };
        let parked = Viewport::new(
            safe.x - bounds.x * zoom,
            safe.y + (safe.height - bounds.height * zoom) / 2.0 - bounds.y * zoom,
            zoom,
        );
        (parked, true)
    }
}

/// A pan that brings one card into the pane.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CardReveal {
    graph: Rect,
    pane_width: f64,
    /// How far a revealed card stays from the pane's edge: the fit's own side
    /// margin, so a card the fit already placed is never "revealed" by a pan.
    margin: f64,
}

impl CardReveal {
    const fn new(graph: Rect, pane_width: f64, margin: f64) -> Self {
        Self {
            graph,
            pane_width,
            margin,
        }
    }

    /// `base` panned along the ranks just far enough to bring `card` inside the
    /// pane — `base` itself when it already is — and never far enough to slide
    /// the graph's first or last card away from the pane's edge.
    fn of(self, base: Viewport, card: Rect) -> Viewport {
        // Rounding in the fit can leave a card a hair past its own margin.
        let margin = self.margin - 0.5;
        let left = base.x + card.x * base.zoom;
        let right = base.x + card.max_x() * base.zoom;
        if left >= margin && right <= self.pane_width - margin {
            return base;
        }
        let centred = self.pane_width / 2.0 - card.center().x * base.zoom;
        let first = self.margin - self.graph.x * base.zoom;
        let last = self.pane_width - self.margin - self.graph.max_x() * base.zoom;
        let x = if last > first {
            first
        } else {
            centred.clamp(last, first)
        };
        Viewport::new(x, base.y, base.zoom)
    }
}

/// Keeps a canvas showing something worth looking at.
///
/// Mount it inside a `Flow`. It refits while the graph and the pane settle —
/// dock layout arrives after the canvas mounts — and stops the moment the
/// operator pans or zooms: from then on their view is kept through a resize,
/// and the canvas moves only to reveal a card they selected somewhere else.
#[component]
pub(crate) fn FlowFit(
    fit: ReadableViewport,
    /// Set while the graph cannot be shown whole and readable. A canvas answers
    /// that by drawing its minimap.
    overflowing: Signal<bool>,
    /// The id of the card to keep in view as the selection moves. An empty id
    /// names no card, which is how a canvas with nothing to reveal says so.
    reveal: ReadSignal<String>,
) -> Element {
    let core = use_flow();
    let mut previous_layout = use_signal(|| None::<(Rect, Rect)>);
    let mut automatic = use_signal(|| None::<Viewport>);
    let mut revealed = use_signal(String::new);
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
            .reduce(|left, right| left.union(&right))
        else {
            return;
        };
        let wanted = reveal();
        let moved = *revealed.peek() != wanted;
        let previous = *previous_layout.peek();
        if previous == Some((container, bounds)) && !moved {
            return;
        }
        let current = *core.viewport.peek();
        let settled = automatic.peek().is_some();
        if automatic.peek().is_some_and(|last| last != current) {
            user_controlled.set(true);
        }
        previous_layout.set(Some((container, bounds)));
        if moved {
            revealed.set(wanted.clone());
        }
        let (parked, cut_off) = fit.park(bounds, container);
        if *overflowing.peek() != cut_off {
            overflowing.set(cut_off);
        }
        let base = if *user_controlled.peek() {
            match previous {
                Some((before, _)) => Viewport::new(
                    current.x + (container.width - before.width) / 2.0,
                    current.y + (container.height - before.height) / 2.0,
                    current.zoom,
                ),
                None => current,
            }
        } else {
            parked
        };
        let target = geometry
            .iter()
            .find(|node| node.id == wanted)
            .map_or(base, |node| {
                CardReveal::new(bounds, container.width, fit.margins.sides).of(base, node.rect)
            });
        automatic.set(Some(target));
        if target != current {
            core.set_viewport(target, if settled && moved { 200 } else { 0 });
        }
    });
    rsx! {}
}

/// Where a detail sheet sits over a canvas, as inline style.
pub(crate) struct PopoverPlacement;

impl PopoverPlacement {
    const WIDTH: f64 = 320.0;
    const GAP: f64 = 8.0;
    /// Room enough that the sheet does not have to be scrolled to be read.
    const COMFORTABLE: f64 = 240.0;
    const MIN_HEIGHT: f64 = 120.0;

    /// Left-aligned with the selected cards, below them when the pane has room,
    /// above them when only that side has room, and over them from the top of a
    /// pane too short for either — always inside the pane. `anchor` and
    /// `container` are both in screen pixels.
    ///
    /// Every placement names `top` and `bottom`, one of them `auto`: the style
    /// attribute is diffed property by property, so a declaration that merely
    /// disappears stays applied, and a sheet moving from above to below would
    /// otherwise keep both edges and collapse to a line.
    pub(crate) fn of(anchor: Rect, container: Rect) -> String {
        let left = anchor
            .x
            .min(container.width - Self::GAP - Self::WIDTH)
            .max(Self::GAP);
        let below_top = (anchor.max_y() + Self::GAP).max(Self::GAP);
        let below_space = container.height - Self::GAP - below_top;
        let above_bottom = (container.height - anchor.y + Self::GAP).max(Self::GAP);
        let above_space = container.height - Self::GAP - above_bottom;
        if below_space >= Self::COMFORTABLE
            || (below_space >= above_space && below_space >= Self::MIN_HEIGHT)
        {
            format!(
                "left:{left:.0}px;top:{below_top:.0}px;bottom:auto;max-height:{below_space:.0}px"
            )
        } else if above_space >= Self::MIN_HEIGHT {
            format!(
                "left:{left:.0}px;top:auto;bottom:{above_bottom:.0}px;max-height:{above_space:.0}px"
            )
        } else {
            let space = (container.height - 2.0 * Self::GAP).max(Self::GAP);
            format!(
                "left:{left:.0}px;top:{:.0}px;bottom:auto;max-height:{space:.0}px",
                Self::GAP
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str) -> Node<()> {
        Node::new(id, id, Point::ZERO).size((224.0, 92.0))
    }

    #[test]
    fn ranks_run_along_the_layout_direction_and_so_does_the_reading() {
        let mut nodes = vec![card("source"), card("shape"), card("output")];
        let edges = vec![Edge::new("source", "shape"), Edge::new("shape", "output")];
        let order = CardGraph::left_to_right(28.0, 56.0).rank_layout(&mut nodes, &edges);
        assert_eq!(order, vec![0, 1, 2]);
        assert!(nodes[0].position.x < nodes[1].position.x);
        assert!(nodes[1].position.x < nodes[2].position.x);

        let mut nodes = vec![card("output"), card("source"), card("shape")];
        let order = CardGraph::top_to_bottom(28.0, 76.0).rank_layout(&mut nodes, &edges);
        // The order is reported in the order the nodes were given, so the node
        // the graph reads first is the one whose rank is 0.
        assert_eq!(order, vec![2, 0, 1]);
        assert!(nodes[1].position.y < nodes[2].position.y);
    }

    #[test]
    fn a_graph_too_wide_to_read_whole_keeps_its_cards_and_starts_at_the_first_rank() {
        let fit = ReadableViewport::new(FlowMargins::new(24.0, 20.0, 48.0), 0.7);
        let pane = Rect::new(0.0, 0.0, 800.0, 600.0);

        let (whole, cut_off) = fit.park(Rect::new(0.0, 0.0, 600.0, 300.0), pane);
        assert!(!cut_off);
        assert!(whole.zoom >= 0.7);

        let (parked, cut_off) = fit.park(Rect::new(0.0, 0.0, 4_000.0, 300.0), pane);
        assert!(cut_off);
        assert_eq!(parked.zoom, 1.0);
        // The first rank sits at the safe area's left edge, not off-screen.
        assert_eq!(parked.x, 24.0);

        // A tree read downwards shows as many branches as stay readable.
        let (parked, cut_off) = fit
            .showing_most()
            .park(Rect::new(0.0, 0.0, 4_000.0, 300.0), pane);
        assert!(cut_off);
        assert_eq!(parked.zoom, 0.7);
        assert_eq!(parked.x, 24.0);
    }

    #[test]
    fn revealing_a_card_never_slides_the_ends_of_the_graph_off_the_pane() {
        let graph = Rect::new(0.0, 0.0, 2_000.0, 92.0);
        let reveal = CardReveal::new(graph, 800.0, 24.0);
        let base = Viewport::new(24.0, 0.0, 1.0);

        let visible = reveal.of(base, Rect::new(100.0, 0.0, 224.0, 92.0));
        assert_eq!(visible, base);

        let far = reveal.of(base, Rect::new(1_700.0, 0.0, 224.0, 92.0));
        // The last card ends at the pane's right margin; no further.
        assert_eq!(far.x, 800.0 - 24.0 - 2_000.0);
        assert_eq!(far.zoom, 1.0);
    }

    #[test]
    fn a_sheet_names_both_edges_wherever_it_lands() {
        let container = Rect::new(0.0, 0.0, 900.0, 600.0);
        let roomy = PopoverPlacement::of(Rect::new(40.0, 40.0, 224.0, 92.0), container);
        assert!(roomy.contains("top:140px"), "{roomy}");
        assert!(roomy.contains("bottom:auto"), "{roomy}");

        let low = PopoverPlacement::of(Rect::new(40.0, 500.0, 224.0, 92.0), container);
        assert!(low.contains("top:auto"), "{low}");
        assert!(low.contains("bottom:108px"), "{low}");

        // A pane with room on neither side still places the sheet inside it.
        let cramped = PopoverPlacement::of(
            Rect::new(40.0, 60.0, 224.0, 92.0),
            Rect::new(0.0, 0.0, 900.0, 200.0),
        );
        assert!(cramped.contains("top:8px"), "{cramped}");
        assert!(cramped.contains("max-height:184px"), "{cramped}");
    }
}
