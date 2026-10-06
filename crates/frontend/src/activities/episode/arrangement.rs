//! Where an inspected episode's panels sit in the room a surface gives it,
//! until the operator arranges them otherwise.
//!
//! Two bands share one clock: the recording above, the timeline below. The
//! recording's band holds its cameras, the leading one first, and beside
//! them one tile of its robot's Scene with Plots as the tab behind it. The
//! timeline — where streams are chosen — is never hidden behind another
//! panel.
//!
//! The cameras take the width their pictures can use at the band's height,
//! in whichever grid leaves the smallest picture largest and spends the least
//! on camera mat; the Scene takes the rest. Once the grid is chosen its
//! column gives way to a little over half the room while every picture stays
//! readable, since the robot and its plots matter more than pixel-perfect
//! pictures and a plot's time axis needs the width. A camera is never shrunk below a
//! readable size to make it fit: when the room runs out, the cameras that do
//! not fit join the last camera's tile as tabs, still full size one click
//! away. A room too narrow for a column beside the cameras sends the Scene
//! and Plots to that tile too.
//!
//! Every share is rounded to a step, so a room that grows or shrinks a little
//! keeps the same arrangement — and with it whatever the operator dragged.
use dioxus_workbench::{LayoutNode, PanelId, PanelLayout, SplitAxis};
use std::iter;

/// The least of the height the timeline keeps: its transport, its ruler and
/// a handful of rows.
const LEAST_TIMELINE_SHARE: f64 = 0.3;
/// The least it keeps when the surface brings rows of its own — a node's
/// progress, its subtasks — because those rows are what the surface is
/// about, and the pictures give way first. Rows brought beside the recording
/// instead take what they need, up to this.
const LEAST_EVIDENCE_TIMELINE_SHARE: f64 = 0.46;
/// The timeline around rows brought beside the recording: its transport, its
/// ruler, and the one row the recorded streams fold into under them.
const TIMELINE_CHROME: f64 = 40.0 + 36.0 + 24.0;
/// The least width the column beside the cameras is worth: the Scene's view
/// controls and a plot's time axis still read.
const LEAST_SIDE_WIDTH: f64 = 280.0;
/// The share of the room's width the column beside the cameras keeps when
/// the cameras' pictures stay readable without it, however large the band
/// would let them grow. A whole number of steps, so a room at the limit does
/// not flip between two shares.
const FAIR_SIDE_SHARE: f64 = 0.46;
/// Shares move in steps of this size, so a small change of room is no change.
const SHARE_STEP: f64 = 0.02;
/// The narrowest picture a camera is drawn at: about 160 × 120, where an arm,
/// a gripper and what it holds still read.
const SMALLEST_PICTURE_WIDTH: f64 = 160.0;
/// The workbench's tab bar, above the content of a tile that holds more than
/// one panel. A lone panel's tile has none: its content fills the tile.
const TAB_BAR: f64 = 32.0;
/// The shape recorded cameras usually have. It decides which grid gives the
/// pictures the most area, not how any picture is drawn.
const PICTURE_ASPECT: f64 = 4.0 / 3.0;

/// A width and a height in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Size {
    width: f64,
    height: f64,
}

impl Size {
    pub(super) fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// One camera's tile in a grid, and whether it also holds the cameras that
/// did not fit — the one tile whose tab bar takes from its picture.
#[derive(Clone, Copy, Debug)]
struct ViewTile {
    size: Size,
    tabbed: bool,
}

impl ViewTile {
    /// The width of a picture of the usual camera shape drawn as large as
    /// this tile allows.
    fn picture_width(self) -> f64 {
        let header = if self.tabbed { TAB_BAR } else { 0.0 };
        self.size
            .width
            .min((self.size.height - header).max(0.0) * PICTURE_ASPECT)
    }
    fn readable(self) -> bool {
        self.picture_width() >= SMALLEST_PICTURE_WIDTH
    }
    fn picture_area(self) -> f64 {
        self.picture_width().powi(2) / PICTURE_ASPECT
    }
    fn area(self) -> f64 {
        self.size.width * self.size.height
    }
}

/// What the timeline holds, and so how much of the room it claims.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TimelineClaim {
    /// The recorded streams, a handful of rows at least.
    Streams,
    /// Rows the surface is about, above the streams folded behind one row.
    Evidence,
    /// Rows this tall brought beside the recording, above the streams
    /// folded behind one row.
    Rows(f64),
}

/// The panels of one inspected episode and where they go.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EpisodeArrangement {
    /// Cameras, the leading one first, then the robot's Scene if it has one.
    views: Vec<PanelId>,
    timeline: TimelineClaim,
}

impl EpisodeArrangement {
    pub(super) const PLOTS: &str = "plots";
    pub(super) const TIMELINE: &str = "timeline";
    pub(super) const STAGE: &str = "stage";
    const CAMERA_PREFIX: &str = "camera:";

    pub(super) fn new(views: Vec<PanelId>) -> Self {
        Self {
            views,
            timeline: TimelineClaim::Streams,
        }
    }
    /// The arrangement for a surface whose own timeline rows are what it is
    /// about.
    pub(super) fn led_by_evidence(mut self) -> Self {
        self.timeline = TimelineClaim::Evidence;
        self
    }
    /// The arrangement for timeline rows `height` tall brought beside the
    /// recording, which leads: the timeline takes what they need and the
    /// pictures the rest.
    pub(super) fn beside_rows(mut self, height: f64) -> Self {
        self.timeline = TimelineClaim::Rows(height);
        self
    }
    /// The share of `room`'s height the timeline takes, on a step.
    fn timeline_share(&self, room: Size) -> f64 {
        match self.timeline {
            TimelineClaim::Streams => LEAST_TIMELINE_SHARE,
            TimelineClaim::Evidence => LEAST_EVIDENCE_TIMELINE_SHARE,
            TimelineClaim::Rows(height) => {
                let needed = (TIMELINE_CHROME + height) / room.height.max(1.0);
                ((needed / SHARE_STEP).ceil() * SHARE_STEP).min(LEAST_EVIDENCE_TIMELINE_SHARE)
            }
        }
    }
    /// The panel one recorded camera opens in.
    pub(super) fn camera(key: &str) -> PanelId {
        PanelId::new(format!("{}{key}", Self::CAMERA_PREFIX))
    }
    /// The stream key of a camera panel; `None` for every other panel.
    pub(super) fn camera_key(panel: &PanelId) -> Option<&str> {
        panel.as_str().strip_prefix(Self::CAMERA_PREFIX)
    }

    /// The designed arrangement for `room`.
    pub(super) fn layout(&self, room: Size) -> PanelLayout {
        let timeline = LayoutNode::tile("episode-timeline", [Self::TIMELINE]);
        let band_share = Self::step(1.0 - self.timeline_share(room));
        let (cameras, side): (Vec<PanelId>, Vec<PanelId>) = self
            .views
            .iter()
            .cloned()
            .partition(|view| Self::camera_key(view).is_some());
        let side = side
            .into_iter()
            .chain(iter::once(PanelId::new(Self::PLOTS)))
            .collect::<Vec<_>>();
        if cameras.is_empty() {
            // Nothing to watch: the Scene and Plots take the pictures' band.
            return Self::rows(band_share, LayoutNode::tile("episode-side", side), timeline);
        }
        let band = Size::new(room.width, room.height * band_share);
        let widest = room.width - LEAST_SIDE_WIDTH;
        if widest < SMALLEST_PICTURE_WIDTH {
            let plan = ViewPlan::for_room(cameras.len(), band, TieBreak::LeastMat);
            return Self::rows(band_share, plan.node(&cameras, &side), timeline);
        }
        let plan = ViewPlan::for_room(
            cameras.len(),
            Size::new(widest, band.height),
            TieBreak::LeastWidth,
        );
        let width = plan.width_beside_side(room.width, band.height, cameras.len());
        Self::rows(
            band_share,
            LayoutNode::split(
                "episode-views",
                SplitAxis::Horizontal,
                Self::step(width / room.width),
                plan.node(&cameras, &[]),
                LayoutNode::tile("episode-side", side),
            ),
            timeline,
        )
    }
    fn rows(upper_share: f64, upper: LayoutNode, lower: LayoutNode) -> PanelLayout {
        PanelLayout::new(LayoutNode::split(
            "episode-rows",
            SplitAxis::Vertical,
            Self::step(upper_share),
            upper,
            lower,
        ))
    }
    fn step(share: f64) -> f64 {
        (share / SHARE_STEP).round() * SHARE_STEP
    }
}

/// How the views fill the space above the timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ViewGrid {
    /// Rows of `columns` tiles. When the views do not fill every row, the
    /// first row is the short one, so the leading view is the wider.
    Rows { columns: usize },
    /// The leading view in a column of its own, the rest in rows of
    /// `columns` beside it.
    Leading { columns: usize },
}

impl ViewGrid {
    /// Every grid that could hold `shown` views.
    fn candidates(shown: usize) -> impl Iterator<Item = Self> {
        let rows = (1..=shown).map(|columns| Self::Rows { columns });
        // Beside a leading column the rest need two rows at least, or the
        // grid is one row of every view, which `Rows` already offers.
        let leading = (1..=shown.saturating_sub(2)).map(|columns| Self::Leading { columns });
        rows.chain(leading)
    }
    /// The size of each of `shown` tiles, leading first.
    fn tiles(self, shown: usize, room: Size) -> Vec<Size> {
        match self {
            Self::Rows { columns } => Self::rows(shown, columns, room),
            Self::Leading { columns } => {
                let lead = Size::new(room.width / (columns + 1) as f64, room.height);
                let rest = Size::new(room.width - lead.width, room.height);
                iter::once(lead)
                    .chain(Self::rows(shown - 1, columns, rest))
                    .collect()
            }
        }
    }
    /// The width at which every shown picture fills its tile's height: the
    /// longest row's worth, since a short row's pictures are the larger.
    fn width_to_fill(self, shown: usize, height: f64) -> f64 {
        let picture_width = |rows: usize| height / rows as f64 * PICTURE_ASPECT;
        match self {
            Self::Rows { columns } => columns as f64 * picture_width(shown.div_ceil(columns)),
            // The leading column is one of `columns + 1` equal shares, and
            // its full-height picture is the widest in the grid.
            Self::Leading { columns } => (columns + 1) as f64 * picture_width(1),
        }
    }
    /// The height at which every shown picture fills its tile's width.
    fn height_to_fill(self, shown: usize, width: f64) -> f64 {
        let rows = |count: usize, columns: usize, width: f64| {
            Self::row_lengths(count, columns)
                .iter()
                .map(|&length| width / length as f64 / PICTURE_ASPECT)
                .sum::<f64>()
        };
        match self {
            Self::Rows { columns } => rows(shown, columns, width),
            Self::Leading { columns } => {
                let lead = width / (columns + 1) as f64;
                (lead / PICTURE_ASPECT).max(rows(shown - 1, columns, width - lead))
            }
        }
    }
    fn rows(count: usize, columns: usize, room: Size) -> Vec<Size> {
        let lengths = Self::row_lengths(count, columns);
        let height = room.height / lengths.len() as f64;
        lengths
            .iter()
            .flat_map(|&length| {
                iter::repeat_n(Size::new(room.width / length as f64, height), length)
            })
            .collect()
    }
    /// How many tiles each row holds, the short row first.
    fn row_lengths(count: usize, columns: usize) -> Vec<usize> {
        let rows = count.div_ceil(columns);
        let first = count - (rows - 1) * columns;
        iter::once(first)
            .chain(iter::repeat_n(columns, rows - 1))
            .collect()
    }
}

/// A grid, how many views it shows as tiles of their own — the rest are tabs
/// of the last tile — and the height it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewPlan {
    grid: ViewGrid,
    shown: usize,
    height: f64,
}

impl ViewPlan {
    /// The plan that shows the most views at a readable size, and among
    /// those the one whose smallest picture is largest, `tie` deciding
    /// between equals. A room too small for any grid still gets one tile,
    /// with every view a tab in it.
    fn for_room(count: usize, tallest: Size, tie: TieBreak) -> Self {
        (2..=count)
            .rev()
            .find_map(|shown| {
                ViewGrid::candidates(shown)
                    .map(|grid| Self::fitted(grid, shown, tallest))
                    .filter_map(|plan| {
                        let tiles = plan.tiles(tallest.width, count);
                        let width = plan.width_to_fill(tallest.height).min(tallest.width);
                        tiles
                            .iter()
                            .all(|tile| tile.readable())
                            .then(|| (plan, PictureScore::of(&tiles, width)))
                    })
                    .reduce(|best, next| {
                        if next.1.beats(best.1, tie) {
                            next
                        } else {
                            best
                        }
                    })
                    .map(|(plan, _)| plan)
            })
            .unwrap_or_else(|| Self::fitted(ViewGrid::Rows { columns: 1 }, 1, tallest))
    }
    /// `grid` at the height its pictures can use, and no taller than the room.
    fn fitted(grid: ViewGrid, shown: usize, tallest: Size) -> Self {
        Self {
            grid,
            shown,
            height: grid
                .height_to_fill(shown, tallest.width)
                .min(tallest.height),
        }
    }
    /// The shown tiles of a grid `width` wide; with fewer shown than
    /// `count`, the last one holds the rest as tabs.
    fn tiles(self, width: f64, count: usize) -> Vec<ViewTile> {
        let sizes = self.grid.tiles(self.shown, Size::new(width, self.height));
        let last = sizes.len() - 1;
        sizes
            .into_iter()
            .enumerate()
            .map(|(place, size)| ViewTile {
                size,
                tabbed: place == last && self.shown < count,
            })
            .collect()
    }
    fn width_to_fill(self, height: f64) -> f64 {
        self.grid.width_to_fill(self.shown, height)
    }
    /// The width the grid takes of a room `room` wide beside the Scene and
    /// Plots: what its pictures can use at `height`, leaving the column its
    /// least width, and no more than leaves it its fair share as long as each
    /// of the `count` cameras is still drawn readable.
    fn width_beside_side(self, room: f64, height: f64, count: usize) -> f64 {
        let wanted = self.width_to_fill(height).min(room - LEAST_SIDE_WIDTH);
        let fair = room * (1.0 - FAIR_SIDE_SHARE);
        let readable_at_fair = self.tiles(fair, count).iter().all(|tile| tile.readable());
        if wanted > fair && readable_at_fair {
            fair
        } else {
            wanted
        }
    }
    /// The views as tiles; the last tile also holds the views that did not
    /// fit, and `extra`, as tabs.
    fn node(self, views: &[PanelId], extra: &[PanelId]) -> LayoutNode {
        let (shown, hidden) = views.split_at(self.shown);
        let mut tiles = shown
            .iter()
            .enumerate()
            .map(|(place, view)| {
                let mut tabs = vec![view.clone()];
                if place + 1 == self.shown {
                    tabs.extend(hidden.iter().cloned());
                    tabs.extend(extra.iter().cloned());
                }
                LayoutNode::tile(format!("episode-view-{place}"), tabs)
            })
            .collect::<Vec<_>>();
        match self.grid {
            ViewGrid::Rows { columns } => Self::rows(tiles, columns),
            ViewGrid::Leading { columns } => {
                let lead = tiles.remove(0);
                LayoutNode::split(
                    "episode-leading",
                    SplitAxis::Horizontal,
                    1.0 / (columns + 1) as f64,
                    lead,
                    Self::rows(tiles, columns),
                )
            }
        }
    }
    fn rows(mut tiles: Vec<LayoutNode>, columns: usize) -> LayoutNode {
        let rows = ViewGrid::row_lengths(tiles.len(), columns)
            .into_iter()
            .enumerate()
            .map(|(row, length)| {
                let rest = tiles.split_off(length);
                let row_tiles = std::mem::replace(&mut tiles, rest);
                Self::even(
                    SplitAxis::Horizontal,
                    &format!("episode-row-{row}"),
                    row_tiles,
                )
            })
            .collect();
        Self::even(SplitAxis::Vertical, "episode-view-rows", rows)
    }
    /// `nodes` side by side along `axis`, each an equal share.
    fn even(axis: SplitAxis, id: &str, mut nodes: Vec<LayoutNode>) -> LayoutNode {
        let count = nodes.len();
        let first = nodes.remove(0);
        if nodes.is_empty() {
            return first;
        }
        LayoutNode::split(
            format!("{id}-{count}"),
            axis,
            1.0 / count as f64,
            first,
            Self::even(axis, id, nodes),
        )
    }
}

/// What decides between grids whose smallest pictures are as large.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TieBreak {
    /// The grid has its room to itself: the one with less camera mat.
    LeastMat,
    /// A column beside the grid gets whatever width it leaves: the
    /// narrowest.
    LeastWidth,
}

/// How well a grid serves its pictures: the smallest one first, then how
/// much of the grid is picture rather than camera mat, and the width it
/// needs for its pictures to fill their height.
#[derive(Clone, Copy, Debug)]
struct PictureScore {
    smallest: f64,
    filled: f64,
    width: f64,
}

impl PictureScore {
    fn of(tiles: &[ViewTile], width: f64) -> Self {
        let areas = tiles.iter().map(|tile| tile.picture_area());
        let room = tiles.iter().map(|tile| tile.area()).sum::<f64>();
        Self {
            smallest: areas.clone().fold(f64::INFINITY, f64::min),
            filled: areas.sum::<f64>() / room,
            width,
        }
    }
    /// Smallest pictures within a percent of each other are a tie, which
    /// `tie` breaks.
    fn beats(self, other: Self, tie: TieBreak) -> bool {
        if self.smallest > other.smallest * 1.01 {
            true
        } else if other.smallest > self.smallest * 1.01 {
            false
        } else {
            match tie {
                TieBreak::LeastMat => self.filled > other.filled,
                TieBreak::LeastWidth => self.width < other.width,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus_workbench::TileId;
    use std::collections::BTreeSet;

    fn views(count: usize) -> Vec<PanelId> {
        (0..count)
            .map(|index| EpisodeArrangement::camera(&format!("/camera-{index}")))
            .collect()
    }

    fn tile_of(layout: &PanelLayout, panel: &str) -> TileId {
        layout
            .tile_for_panel(&PanelId::new(panel))
            .expect("every panel is placed")
    }

    fn tiles_of(layout: &PanelLayout, panels: &[PanelId]) -> BTreeSet<TileId> {
        panels
            .iter()
            .map(|panel| tile_of(layout, panel.as_str()))
            .collect()
    }

    fn with_scene(mut views: Vec<PanelId>) -> Vec<PanelId> {
        views.push(PanelId::new(EpisodeArrangement::STAGE));
        views
    }

    #[test]
    fn the_cameras_sit_beside_the_scene_with_plots_behind_it() {
        let cameras = views(3);
        let layout =
            EpisodeArrangement::new(with_scene(cameras.clone())).layout(Size::new(1140.0, 800.0));
        let LayoutNode::Split { ratio, first, .. } = &layout.root else {
            panic!("the recording over the timeline")
        };
        assert!((ratio - 0.7).abs() < 1e-9, "the recording takes {ratio}");
        assert!(matches!(
            first.as_ref(),
            LayoutNode::Split { id, .. } if id.as_str() == "episode-views"
        ));
        assert_eq!(tiles_of(&layout, &cameras).len(), 3);
        let side = layout
            .tile(&tile_of(&layout, EpisodeArrangement::STAGE))
            .unwrap();
        assert_eq!(
            side.panels,
            [
                PanelId::new(EpisodeArrangement::STAGE),
                PanelId::new(EpisodeArrangement::PLOTS)
            ]
        );
        assert_eq!(
            side.active.as_ref().map(PanelId::as_str),
            Some(EpisodeArrangement::STAGE),
            "the Scene is in front"
        );
    }

    #[test]
    fn three_cameras_set_the_leading_one_over_the_other_two() {
        // A curation node's evidence at a 1440 × 900 window: the leading
        // camera across the top, the other two under it, and the Scene the
        // width the grid leaves.
        let cameras = views(3);
        let layout = EpisodeArrangement::new(with_scene(cameras.clone()))
            .led_by_evidence()
            .layout(Size::new(708.0, 752.0));
        let LayoutNode::Split { first, .. } = &layout.root else {
            panic!("the recording over the timeline")
        };
        let LayoutNode::Split { ratio, .. } = first.as_ref() else {
            panic!("the cameras beside the Scene")
        };
        assert!(
            (0.5..0.62).contains(ratio),
            "the cameras take {ratio} of the width"
        );
        assert_eq!(tiles_of(&layout, &cameras).len(), 3);
        let plan = ViewPlan::for_room(3, Size::new(584.0, 405.0), TieBreak::LeastWidth);
        assert_eq!(plan.grid, ViewGrid::Rows { columns: 2 });
        assert_eq!(plan.shown, 3);
    }

    #[test]
    fn three_cameras_keep_one_over_two_in_every_room_a_surface_gives() {
        // Break down's evidence and Select's, shorter, at a 1440 × 900
        // window, and Data's. A lone camera's tile has no tab bar, so two
        // rows of pictures fit where two rows of tabbed tiles did not.
        for (room, led) in [
            (Size::new(708.0, 752.0), true),
            (Size::new(737.0, 513.0), true),
            (Size::new(1137.0, 782.0), false),
        ] {
            let cameras = views(3);
            let arrangement = EpisodeArrangement::new(with_scene(cameras.clone()));
            let arrangement = if led {
                arrangement.led_by_evidence()
            } else {
                arrangement
            };
            let layout = arrangement.layout(room);
            assert_eq!(tiles_of(&layout, &cameras).len(), 3, "{room:?}");
            let splits = layout.split_ids();
            assert!(
                splits.iter().any(|id| id.as_str() == "episode-row-1-2"),
                "the other two share the second row in {room:?}: {splits:?}"
            );
        }
    }

    #[test]
    fn the_scene_and_plots_keep_near_half_the_room_beside_large_pictures() {
        // A curation node's evidence in a tall window: the pictures could
        // take three fifths of the width, and the plots' time axis would be
        // left a strip. The cameras keep their one-over-two grid and give
        // way, their pictures still readable.
        let room = Size::new(700.0, 632.0);
        let layout = EpisodeArrangement::new(views(3)).layout(room);
        let LayoutNode::Split { first, .. } = &layout.root else {
            panic!("the recording over the timeline")
        };
        let LayoutNode::Split { ratio, .. } = first.as_ref() else {
            panic!("the cameras beside the plots")
        };
        assert!(
            (ratio - (1.0 - FAIR_SIDE_SHARE)).abs() < 1e-9,
            "the cameras take {ratio} of the width"
        );
        assert_eq!(tiles_of(&layout, &views(3)).len(), 3);
        assert!(
            layout
                .split_ids()
                .iter()
                .any(|id| id.as_str() == "episode-row-1-2")
        );
    }

    #[test]
    fn a_narrow_room_sends_the_scene_and_plots_to_the_last_camera() {
        let four = views(4);
        let layout =
            EpisodeArrangement::new(with_scene(four.clone())).layout(Size::new(420.0, 800.0));
        let last = tile_of(&layout, four[3].as_str());
        assert_eq!(tile_of(&layout, EpisodeArrangement::STAGE), last);
        assert_eq!(tile_of(&layout, EpisodeArrangement::PLOTS), last);
        assert_ne!(tile_of(&layout, four[0].as_str()), last);
    }

    #[test]
    fn a_surface_with_rows_of_its_own_gives_the_timeline_more_of_the_room() {
        let cameras = views(2);
        let share = |layout: PanelLayout| match layout.root {
            LayoutNode::Split { ratio, .. } => ratio,
            _ => 1.0,
        };
        let room = Size::new(600.0, 900.0);
        let plain = EpisodeArrangement::new(cameras.clone()).layout(room);
        let led = EpisodeArrangement::new(cameras)
            .led_by_evidence()
            .layout(room);
        assert!(
            share(led) < share(plain),
            "the pictures give way to the evidence"
        );
    }

    #[test]
    fn rows_beside_the_recording_take_what_they_need_and_the_cameras_fill_the_rest() {
        // A curated view's intervals on an episode in Data at a 1440 × 900
        // window: one row of spans over the folded streams.
        let cameras = views(3);
        let room = Size::new(1137.0, 786.0);
        let layout = EpisodeArrangement::new(with_scene(cameras.clone()))
            .beside_rows(44.0)
            .layout(room);
        let LayoutNode::Split { ratio, .. } = &layout.root else {
            panic!("the recording over the timeline")
        };
        assert!((ratio - 0.8).abs() < 1e-9, "the recording takes {ratio}");
        assert!(
            (1.0 - ratio) * room.height >= TIMELINE_CHROME + 44.0,
            "the timeline shows its row whole"
        );
        // Two rows of pictures that fill the band's height: nothing is drawn
        // letterboxed in a band taller than its pictures.
        let band = Size::new(room.width - LEAST_SIDE_WIDTH, room.height * ratio);
        let plan = ViewPlan::for_room(3, band, TieBreak::LeastWidth);
        assert_eq!(plan.grid, ViewGrid::Rows { columns: 2 });
        assert!((plan.height - band.height).abs() < 1e-9);
        assert_eq!(tiles_of(&layout, &cameras).len(), 3);
        // Many rows never take more than a surface led by its rows gives.
        let tall = EpisodeArrangement::new(with_scene(cameras))
            .beside_rows(900.0)
            .timeline_share(room);
        assert!((tall - LEAST_EVIDENCE_TIMELINE_SHARE).abs() < 1e-9);
    }

    #[test]
    fn the_timeline_is_never_hidden_behind_another_panel() {
        for (count, width, height) in [
            (3, 1140.0, 800.0),
            (4, 600.0, 550.0),
            (3, 300.0, 400.0),
            (0, 800.0, 600.0),
            (4, 996.0, 544.0),
        ] {
            let layout =
                EpisodeArrangement::new(with_scene(views(count))).layout(Size::new(width, height));
            let tile = layout
                .tile(&tile_of(&layout, EpisodeArrangement::TIMELINE))
                .unwrap();
            assert_eq!(
                tile.panels,
                [PanelId::new(EpisodeArrangement::TIMELINE)],
                "{count} views in {width}×{height}"
            );
        }
    }

    #[test]
    fn a_cramped_room_keeps_one_full_size_tile_with_the_leading_view_in_front() {
        let three = views(3);
        let layout = EpisodeArrangement::new(three.clone()).layout(Size::new(300.0, 400.0));
        let tile = layout.tile(&tile_of(&layout, three[0].as_str())).unwrap();
        let mut expected = three.clone();
        expected.push(PanelId::new(EpisodeArrangement::PLOTS));
        assert_eq!(tile.panels, expected);
        assert_eq!(tile.active.as_ref(), Some(&three[0]));
    }

    #[test]
    fn four_views_in_a_generous_room_make_two_rows_of_two() {
        let four = views(4);
        let plan = ViewPlan::for_room(4, Size::new(700.0, 630.0), TieBreak::LeastMat);
        assert_eq!(plan.grid, ViewGrid::Rows { columns: 2 });
        assert_eq!(plan.shown, 4);
        let layout = EpisodeArrangement::new(four.clone()).layout(Size::new(1020.0, 900.0));
        assert_eq!(tiles_of(&layout, &four).len(), 4);
    }

    #[test]
    fn equal_pictures_go_to_less_mat_alone_and_to_less_width_beside_the_scene() {
        // Both grids draw the other two cameras as large; a lead column
        // spends less on mat, the one-over-two grid leaves the Scene more.
        let room = Size::new(760.0, 472.0);
        let alone = ViewPlan::for_room(3, room, TieBreak::LeastMat);
        assert_eq!(alone.grid, ViewGrid::Leading { columns: 1 });
        let beside = ViewPlan::for_room(3, room, TieBreak::LeastWidth);
        assert_eq!(beside.grid, ViewGrid::Rows { columns: 2 });
    }

    #[test]
    fn an_episode_without_views_gives_the_plots_their_space() {
        let layout = EpisodeArrangement::new(vec![]).layout(Size::new(800.0, 600.0));
        assert_eq!(layout.tile_count(), 2);
        assert_ne!(
            tile_of(&layout, EpisodeArrangement::PLOTS),
            tile_of(&layout, EpisodeArrangement::TIMELINE)
        );
    }

    #[test]
    fn a_room_that_changes_a_little_keeps_its_arrangement() {
        let arrangement = EpisodeArrangement::new(views(3));
        assert_eq!(
            arrangement.layout(Size::new(1140.0, 800.0)),
            arrangement.layout(Size::new(1146.0, 803.0))
        );
    }

    #[test]
    fn short_rows_lead_and_camera_panels_know_their_stream() {
        assert_eq!(ViewGrid::row_lengths(3, 2), [1, 2]);
        assert_eq!(ViewGrid::row_lengths(4, 2), [2, 2]);
        assert_eq!(ViewGrid::row_lengths(5, 3), [2, 3]);
        let panel = EpisodeArrangement::camera("/top#abc");
        assert_eq!(EpisodeArrangement::camera_key(&panel), Some("/top#abc"));
        assert_eq!(EpisodeArrangement::camera_key(&PanelId::new("plots")), None);
    }
}
