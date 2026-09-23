//! Where an inspected episode's panels sit in the room a surface gives it.
//!
//! Three bands share one clock, top to bottom: the recording's views — its
//! cameras, the leading one first, then its robot — then Plots, then the
//! timeline. Plots and the timeline span the same width and give their names
//! the same column, so one vertical line through both is one instant.
//!
//! The views take only the height their pictures can use, in whichever grid
//! leaves the smallest picture largest and spends the least on camera mat. A
//! view is never shrunk below a readable size to make it fit: when the room
//! runs out, the views that do not fit join the last tile as tabs, still full
//! size one click away. A room too short for a Plots band sends Plots to that
//! tile too, so the timeline — where streams are chosen — is never hidden
//! behind a plot of them.
//!
//! Every share is rounded to a step, so a room that grows or shrinks a little
//! keeps the same arrangement — and with it whatever the operator dragged.
use dioxus_workbench::{LayoutNode, PanelId, PanelLayout, SplitAxis};
use std::iter;

/// The least of the height the timeline keeps: its transport, its ruler and
/// a handful of rows.
const LEAST_TIMELINE_SHARE: f64 = 0.3;
/// The least height a Plots band is worth: its tab, its ruler, a stream's
/// heading and one lane.
const LEAST_PLOTS_BAND: f64 = 200.0;
/// Shares move in steps of this size, so a small change of room is no change.
const SHARE_STEP: f64 = 0.02;
/// The smallest tile a view is drawn in, tab bar included: a picture about
/// 225 px wide, where an arm, a gripper and what it holds still read.
const SMALLEST_VIEW: Size = Size {
    width: 256.0,
    height: 200.0,
};
/// The workbench's tab bar above every tile's content.
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
    fn fits(self, smallest: Self) -> bool {
        self.width >= smallest.width && self.height >= smallest.height
    }
    /// The area of a picture of the usual camera shape drawn as large as
    /// this tile allows below its tab bar.
    fn picture_area(self) -> f64 {
        let width = self
            .width
            .min((self.height - TAB_BAR).max(0.0) * PICTURE_ASPECT);
        width * width / PICTURE_ASPECT
    }
}

/// The panels of one inspected episode and where they go.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EpisodeArrangement {
    /// Cameras, the leading one first, then the robot.
    views: Vec<PanelId>,
}

impl EpisodeArrangement {
    pub(super) const PLOTS: &str = "plots";
    pub(super) const TIMELINE: &str = "timeline";
    pub(super) const STAGE: &str = "stage";
    const CAMERA_PREFIX: &str = "camera:";

    pub(super) fn new(views: Vec<PanelId>) -> Self {
        Self { views }
    }
    /// The panel one recorded camera opens in.
    pub(super) fn camera(key: &str) -> PanelId {
        PanelId::new(format!("{}{key}", Self::CAMERA_PREFIX))
    }
    /// The stream key of a camera panel; `None` for every other panel.
    pub(super) fn camera_key(panel: &PanelId) -> Option<&str> {
        panel.as_str().strip_prefix(Self::CAMERA_PREFIX)
    }

    pub(super) fn layout(&self, room: Size) -> PanelLayout {
        let timeline = LayoutNode::tile("episode-timeline", [Self::TIMELINE]);
        let plots = LayoutNode::tile("episode-plots", [Self::PLOTS]);
        if self.views.is_empty() {
            // Nothing to watch: the plots take the space the pictures would.
            return Self::rows(0.5, plots, timeline);
        }
        let least_timeline = room.height * LEAST_TIMELINE_SHARE;
        let tallest = Size::new(room.width, room.height - least_timeline);
        let plan = ViewPlan::for_room(self.views.len(), tallest);
        let rest = room.height - plan.height;
        let views_share = plan.height / room.height;
        if rest - least_timeline < LEAST_PLOTS_BAND {
            return Self::rows(
                views_share,
                plan.node(&self.views, Some(Self::PLOTS)),
                timeline,
            );
        }
        // Plots and the timeline split what the pictures leave evenly, and
        // the timeline never keeps less than its least.
        let plots_height = (rest / 2.0)
            .min(rest - least_timeline)
            .max(LEAST_PLOTS_BAND);
        Self::rows(
            views_share,
            plan.node(&self.views, None),
            LayoutNode::split(
                "episode-plots-timeline",
                SplitAxis::Vertical,
                Self::step(plots_height / rest),
                plots,
                timeline,
            ),
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
    /// The height at which every shown picture fills its tile's width.
    fn height_to_fill(self, shown: usize, width: f64) -> f64 {
        let rows = |count: usize, columns: usize, width: f64| {
            Self::row_lengths(count, columns)
                .iter()
                .map(|&length| width / length as f64 / PICTURE_ASPECT + TAB_BAR)
                .sum::<f64>()
        };
        match self {
            Self::Rows { columns } => rows(shown, columns, width),
            Self::Leading { columns } => {
                let lead = width / (columns + 1) as f64;
                (lead / PICTURE_ASPECT + TAB_BAR).max(rows(shown - 1, columns, width - lead))
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
    /// those the one whose smallest picture is largest. A room too small for
    /// any grid still gets one tile, with every view a tab in it.
    fn for_room(count: usize, tallest: Size) -> Self {
        (2..=count)
            .rev()
            .find_map(|shown| {
                ViewGrid::candidates(shown)
                    .map(|grid| Self::fitted(grid, shown, tallest))
                    .filter_map(|plan| {
                        let tiles = plan.tiles(tallest.width);
                        tiles
                            .iter()
                            .all(|tile| tile.fits(SMALLEST_VIEW))
                            .then(|| (plan, PictureScore::of(&tiles)))
                    })
                    .reduce(|best, next| if next.1.beats(best.1) { next } else { best })
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
    fn tiles(self, width: f64) -> Vec<Size> {
        self.grid.tiles(self.shown, Size::new(width, self.height))
    }
    /// The views as tiles; the last tile also holds the views that did not
    /// fit, and `extra`, as tabs.
    fn node(self, views: &[PanelId], extra: Option<&str>) -> LayoutNode {
        let (shown, hidden) = views.split_at(self.shown);
        let mut tiles = shown
            .iter()
            .enumerate()
            .map(|(place, view)| {
                let mut tabs = vec![view.clone()];
                if place + 1 == self.shown {
                    tabs.extend(hidden.iter().cloned());
                    tabs.extend(extra.map(PanelId::new));
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

/// How well a grid serves its pictures: the smallest one first, then how
/// much of the grid is picture rather than camera mat.
#[derive(Clone, Copy, Debug)]
struct PictureScore {
    smallest: f64,
    filled: f64,
}

impl PictureScore {
    fn of(tiles: &[Size]) -> Self {
        let areas = tiles.iter().map(|tile| tile.picture_area());
        let room = tiles
            .iter()
            .map(|tile| tile.width * tile.height)
            .sum::<f64>();
        Self {
            smallest: areas.clone().fold(f64::INFINITY, f64::min),
            filled: areas.sum::<f64>() / room,
        }
    }
    /// Smallest pictures within a percent of each other are a tie, which
    /// the grid with less mat breaks.
    fn beats(self, other: Self) -> bool {
        if self.smallest > other.smallest * 1.01 {
            true
        } else if other.smallest > self.smallest * 1.01 {
            false
        } else {
            self.filled > other.filled
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

    #[test]
    fn a_wide_room_sets_the_cameras_across_over_plots_over_the_timeline() {
        let cameras = views(3);
        let layout = EpisodeArrangement::new(cameras.clone()).layout(Size::new(1140.0, 800.0));
        let LayoutNode::Split { ratio, second, .. } = &layout.root else {
            panic!("views over the rest")
        };
        // Three pictures 380 px wide need 317 px of the 800.
        assert_eq!(*ratio, 0.4);
        assert!(matches!(
            second.as_ref(),
            LayoutNode::Split { id, ratio, .. } if id.as_str() == "episode-plots-timeline" && *ratio == 0.5
        ));
        assert_eq!(tiles_of(&layout, &cameras).len(), 3);
        assert_ne!(
            tile_of(&layout, EpisodeArrangement::PLOTS),
            tile_of(&layout, EpisodeArrangement::TIMELINE)
        );
    }

    #[test]
    fn views_that_would_be_too_small_become_tabs_of_the_last_tile_with_plots() {
        let four = views(4);
        let layout = EpisodeArrangement::new(four.clone()).layout(Size::new(600.0, 550.0));
        let last = tile_of(&layout, four[1].as_str());
        assert_ne!(tile_of(&layout, four[0].as_str()), last);
        assert_eq!(tile_of(&layout, four[2].as_str()), last);
        assert_eq!(tile_of(&layout, four[3].as_str()), last);
        assert_eq!(tile_of(&layout, EpisodeArrangement::PLOTS), last);
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
            let layout = EpisodeArrangement::new(views(count)).layout(Size::new(width, height));
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
        let plan = ViewPlan::for_room(4, Size::new(700.0, 630.0));
        assert_eq!(plan.grid, ViewGrid::Rows { columns: 2 });
        assert_eq!(plan.shown, 4);
        let layout = EpisodeArrangement::new(four.clone()).layout(Size::new(700.0, 900.0));
        assert_eq!(tiles_of(&layout, &four).len(), 4);
    }

    #[test]
    fn the_leading_camera_takes_a_column_only_where_its_picture_is_the_larger() {
        // Data at a 1024 px window: a column beats a full-width first row
        // whose picture would sit between two slabs of mat.
        let narrow = ViewPlan::for_room(3, Size::new(760.0, 472.0));
        assert_eq!(narrow.grid, ViewGrid::Leading { columns: 1 });
        // Wide enough for three across, nobody leads.
        let wide = ViewPlan::for_room(3, Size::new(1089.0, 561.0));
        assert_eq!(wide.grid, ViewGrid::Rows { columns: 3 });
    }

    #[test]
    fn an_episode_without_views_gives_the_plots_their_space() {
        let layout = EpisodeArrangement::new(vec![]).layout(Size::new(800.0, 600.0));
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
