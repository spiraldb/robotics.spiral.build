//! Select: keep part of what reaches the node, by the measure of the node
//! above it, and show exactly what is kept and what is left out.
//!
//! The workspace reads top to bottom as the decision does: the rule, in the
//! words of the node it follows; how much it keeps; where the threshold falls
//! among the values it judges; where in the episodes the kept parts lie; and
//! then the kept (or left-out) parts one at a time beside their recording.
//! A Select nothing follows is an output, and publishes from here.
mod listing;
mod publish;
mod rule;
mod strip;

use super::document::{CurationNode, CurationNodeKind, Selection, SelectionBasis};
use super::episode_map::{EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, SegmentPaint};
use super::episode_tabs::{OpenEpisode, OpenEpisodes};
use super::population::{EpisodeSample, Population, Slice};
use super::review::{Counted, NodeBar, NodeReach};
use super::state::CurationController;
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use listing::{EpisodeListing, ListedSide, MomentListing, SubtaskListing};
use publish::PublishControl;
use rule::{SelectNode, SelectRule};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{ActionPurpose, EmptyState, ModeButton, ModeGroup, Notice, NoticeTone};
use strip::{AxisUnit, StripDot, Threshold, ThresholdScale, ThresholdStrip};

/// Which side of the rule the index lists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Showing {
    Kept,
    LeftOut,
}

/// Everything one Select shows, read once per render from the graph and the
/// sample it is interpreted over.
#[derive(Clone, PartialEq)]
struct SelectView {
    node: String,
    selection: Selection,
    /// The node the rule is stated in terms of.
    measure: CurationNode,
    input: Population,
    output: Population,
    sample: EpisodeSample,
}

impl SelectView {
    fn of(controller: CurationController, node: &str) -> Option<Self> {
        let sample = controller.sample()?;
        let (input, output) = controller.populations(node)?;
        controller.with_payload(|payload| {
            let CurationNodeKind::Select(selection) = payload.node(node)?.kind() else {
                return None;
            };
            Some(Self {
                node: node.to_owned(),
                selection: selection.clone(),
                measure: payload.measure_above(node)?.clone(),
                input,
                output,
                sample,
            })
        })
    }
    /// Whether the rule keeps each item that reaches it, in input order.
    fn kept(&self) -> Vec<bool> {
        match (&self.input, &self.output) {
            (Population::Episodes(input), Population::Episodes(output)) => {
                input.iter().map(|id| output.contains(id)).collect()
            }
            (Population::Subtasks(input), Population::Subtasks(output)) => {
                input.iter().map(|row| output.contains(row)).collect()
            }
            // Moments come one per frame, too many to look each up in what
            // the rule kept; the rule itself says.
            (Population::Moments(input), Population::Moments(_)) => input
                .iter()
                .map(|row| self.selection.keeps_moment(row.progress_rate()))
                .collect(),
            _ => Vec::new(),
        }
    }
    /// One slice per item that reaches the rule, in input order: moments one
    /// frame at a time, as they were judged.
    fn item_slices(&self) -> Vec<Slice> {
        self.input.items(&self.sample)
    }
    fn kept_seconds(&self) -> f64 {
        self.output
            .slices(&self.sample)
            .iter()
            .map(Slice::seconds)
            .sum()
    }
    fn input_seconds(&self) -> f64 {
        self.input
            .slices(&self.sample)
            .iter()
            .map(Slice::seconds)
            .sum()
    }
    /// How much recording the rule keeps, which is what training is finally
    /// given, of what reaches it. The counts are the Kept and Left out
    /// segments over the index.
    fn tally(&self) -> String {
        format!(
            "Keeps {} of {}",
            self.kept_seconds().clock_label(),
            self.input_seconds().clock_label(),
        )
    }
    /// How many of the episodes that reach the rule keep anything, which is
    /// what the map under the strip draws.
    fn kept_in(&self) -> String {
        let reached = self.input.episodes();
        let kept = self.output.episodes();
        format!(
            "Kept in {} of {}",
            kept.len(),
            Counted::episodes(reached.len())
        )
    }
    /// How the rule's value reads on its axis.
    fn unit(&self) -> AxisUnit {
        match self.selection.basis() {
            SelectionBasis::Similarity => AxisUnit::Percent,
            SelectionBasis::Progress => AxisUnit::ProgressRate,
            SelectionBasis::EpisodeFacts | SelectionBasis::Categories => AxisUnit::Plain,
        }
    }
    /// Where one item of the input plays: its episode, and its stretch unless
    /// the item is the episode whole.
    fn place(&self, item: usize) -> Option<(EpisodeId, Option<(f64, f64)>)> {
        match &self.input {
            Population::Episodes(episodes) => episodes.get(item).map(|id| (id.clone(), None)),
            Population::Subtasks(rows) => rows.get(item).map(|row| {
                (
                    row.episode().clone(),
                    Some((row.start_time(), row.end_time())),
                )
            }),
            Population::Moments(rows) => rows.get(item).map(|row| {
                (
                    row.episode().clone(),
                    Some((row.start_time(), row.end_time())),
                )
            }),
        }
    }
    /// The item a tab is playing: its episode whole, or the one of its items
    /// that starts where the tab's stretch does.
    fn item_in(&self, tab: &OpenEpisode) -> Option<usize> {
        match &self.input {
            Population::Episodes(episodes) => tab
                .interval()
                .is_none()
                .then(|| episodes.iter().position(|id| id == tab.episode()))
                .flatten(),
            Population::Subtasks(rows) => rows
                .iter()
                .position(|row| tab.holds(row.episode(), row.start_time())),
            Population::Moments(rows) => rows
                .iter()
                .position(|row| tab.holds(row.episode(), row.start_time())),
        }
    }
    /// Open the episode `item` belongs to, playing the item.
    fn open(&self, tabs: OpenEpisodes, item: usize) {
        if let Some((episode, interval)) = self.place(item) {
            tabs.open(&episode, interval);
        }
    }
    /// What an empty side of the rule says in place of its list.
    fn empty_side(&self, listed_kept: bool) -> &'static str {
        match (&self.selection, listed_kept) {
            (Selection::Categories { names }, true) if names.is_empty() => {
                "Choose the categories to keep above."
            }
            (_, true) => "Nothing passes this rule. Loosen it above to keep something.",
            (_, false) => "Nothing is left out: everything that reaches this rule passes it.",
        }
    }
    /// Why nothing can be judged yet, when the search or grouping above has
    /// not read any episode.
    fn unmeasured(&self) -> Option<String> {
        let judging = matches!(
            self.measure.kind(),
            CurationNodeKind::FindSimilarSubtasks(_) | CurationNodeKind::GroupSimilarSubtasks(_)
        );
        (judging && self.input.len() == 0).then(|| match self.selection.basis() {
            SelectionBasis::Similarity => {
                "The search above has not answered its question yet. Run it, then choose how close a subtask must be.".to_owned()
            }
            _ => "Group the subtasks above first, then choose the categories to keep.".to_owned(),
        })
    }
}

/// What the episode map draws for a Select: one segment per item, except
/// that moments — one per frame, thousands to an episode — are drawn as the
/// runs they make on one side of the rule, each opening at its first moment.
struct MapSegments {
    slices: Vec<Slice>,
    first_items: Vec<usize>,
    kept: Vec<bool>,
}

impl MapSegments {
    fn of(view: &SelectView, kept: &[bool]) -> Self {
        let items = view.item_slices();
        let mut segments = Self {
            slices: Vec::new(),
            first_items: Vec::new(),
            kept: Vec::new(),
        };
        let joins_runs = matches!(view.input, Population::Moments(_));
        for (item, slice) in items.into_iter().enumerate() {
            let side = kept.get(item).copied().unwrap_or(false);
            match (segments.slices.last_mut(), segments.kept.last()) {
                (Some(last), Some(last_side))
                    if joins_runs && *last_side == side && last.continues_into(&slice) =>
                {
                    last.extend_to(&slice);
                }
                _ => {
                    segments.slices.push(slice);
                    segments.first_items.push(item);
                    segments.kept.push(side);
                }
            }
        }
        segments
    }
    /// The segment an item is drawn in.
    fn segment_of(&self, item: usize) -> usize {
        self.first_items
            .partition_point(|first| *first <= item)
            .saturating_sub(1)
    }
}

/// The workspace shown for a selected Select node.
#[component]
pub(super) fn SelectWorkspace(controller: CurationController, node: String) -> Element {
    let mut showing = use_signal(|| Showing::Kept);
    // The episodes open beside the index. The item playing in the tab in
    // front is the chosen one, so the map, the strip, and either side of the
    // index agree on it.
    let tabs = OpenEpisodes::use_open();
    let Some(view) = SelectView::of(controller, &node) else {
        return rsx! {
            section { class: "curation-select-workspace",
                match controller.sample_error() {
                    Some(error) => rsx! {
                        Notice { tone: NoticeTone::Danger, "The episodes could not be read: {error}" }
                    },
                    None => rsx! { EmptyState { p { "Reading episodes…" } } },
                }
            }
        };
    };
    let spans = EpisodeSpans::of(&view.sample);
    let kept = view.kept();
    let listed_kept = showing() == Showing::Kept;
    let chosen = tabs.front().and_then(|tab| view.item_in(&tab));
    let open = {
        let kept = kept.clone();
        let view = view.clone();
        move |item: usize| {
            view.open(tabs, item);
            showing.set(if kept.get(item).copied().unwrap_or(true) {
                Showing::Kept
            } else {
                Showing::LeftOut
            });
        }
    };
    let dots = StripDot::for_view(&view, &kept);
    // One axis for the strip and the slider on it, so the thumb is always
    // where the threshold line stands.
    let threshold = view.selection.threshold().map(|value| {
        Threshold::new(
            value,
            ThresholdScale::of(dots.as_deref().unwrap_or_default(), value, view.unit()),
            view.selection.keeps_at_least(),
        )
    });
    let rule = SelectNode::new(view.node.clone(), controller);
    let segments = MapSegments::of(&view, &kept);
    let is_output = controller.with_payload(|payload| {
        payload
            .outputs()
            .iter()
            .any(|output| output.id() == node.as_str())
    });
    let selection = view.selection.clone();
    // Switching sides plays that side's first row, so the recording beside
    // the list is always one of the rows it lists.
    let kept_side = ListedSide::new(view.clone(), kept.clone(), true);
    let left_side = ListedSide::new(view.clone(), kept.clone(), false);
    let (kept_rows, left_rows) = (kept_side.rows(), left_side.rows());
    let sides = rsx! {
        ModeGroup { label: "Which side of the rule to list".to_owned(), class: "curation-select-sides".to_owned(),
            ModeButton {
                purpose: ActionPurpose::Select,
                checked: listed_kept,
                onclick: move |_| {
                    if showing() != Showing::Kept {
                        showing.set(Showing::Kept);
                        if let Some((episode, span)) = kept_side.opening() {
                            tabs.open(&episode, span);
                        }
                    }
                },
                "Kept {kept_rows}"
            }
            ModeButton {
                purpose: ActionPurpose::Select,
                checked: !listed_kept,
                onclick: move |_| {
                    if showing() != Showing::LeftOut {
                        showing.set(Showing::LeftOut);
                        if let Some((episode, span)) = left_side.opening() {
                            tabs.open(&episode, span);
                        }
                    }
                },
                "Left out {left_rows}"
            }
        }
    };
    let side = ListedSide::new(view.clone(), kept.clone(), listed_kept);
    rsx! {
        section { class: "curation-select-workspace", "aria-label": "Select",
            NodeBar {
                lead: rsx! { SelectRule { controller, view: view.clone(), threshold } },
                note: view.unmeasured(),
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                // Whether this is an output is the card's flag in the graph;
                // the bar ends with what the rule gives training, and an
                // output with where it is published.
                actions: rsx! {
                    p { class: "curation-select-tally", role: "status", "{view.tally()}" }
                    if is_output {
                        PublishControl { controller, node: node.clone() }
                    }
                },
            }
            if dots.is_some() || !matches!(view.input, Population::Episodes(_)) {
                div { class: "curation-select-overview",
                    if let (Some(dots), Some(threshold)) = (dots, threshold) {
                        ThresholdStrip {
                            dots,
                            threshold,
                            chosen,
                            on_select: {
                                let mut open = open.clone();
                                move |item| open(item)
                            },
                            on_threshold: move |value| rule.set(selection.clone().with_threshold(value)),
                        }
                    }
                    // Where the kept parts lie inside their episodes. A rule over
                    // whole episodes keeps each one end to end or not at all, so
                    // its map would be a column of full bars saying what the
                    // counts already say.
                    if !matches!(view.input, Population::Episodes(_)) {
                        EpisodeMap {
                            lanes: EpisodeLanes::of(&view.input.episodes(), &segments.slices, &spans),
                            // Kept in ink, left out faded; the Kept and Left out
                            // segments over the index say the same in words.
                            paint: MapPaint::new(
                                segments.kept.iter().map(|kept| SegmentPaint::Kept(*kept)).collect(),
                                chosen.map(|item| segments.segment_of(item)),
                            ),
                            summary: view.kept_in(),
                            on_select: {
                                let mut open = open.clone();
                                let firsts = segments.first_items.clone();
                                move |segment: usize| {
                                    if let Some(item) = firsts.get(segment) {
                                        open(*item);
                                    }
                                }
                            },
                        }
                    }
                }
            }
            match &view.input {
                Population::Episodes(_) => rsx! {
                    EpisodeListing { controller, side, tabs, sides }
                },
                Population::Subtasks(_) => rsx! {
                    SubtaskListing { controller, side, tabs, sides }
                },
                Population::Moments(_) => rsx! {
                    MomentListing { controller, side, tabs, sides }
                },
            }
        }
    }
}
