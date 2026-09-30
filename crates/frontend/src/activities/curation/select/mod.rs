//! Select: keep the episodes whose recorded facts — duration, message count,
//! channel count, file size — meet every one of its conditions, and show
//! exactly what is kept and what is left out, and by which condition.
//!
//! The review is any rule's, not only a Select's: Score progress keeps its
//! moments by a rule of its own and reads them here too. It reads top to
//! bottom as the decision does: the rule, in the words of what it keeps by;
//! how much it keeps; where each threshold falls among the values it judges;
//! where in the episodes the kept parts lie; and then the kept (or left-out)
//! parts one at a time beside their recording.
mod listing;
mod rule;

pub(super) use rule::ProgressRule;

use super::document::{Comparison, CurationNode, CurationNodeKind, EpisodeCondition, Selection};
use super::episode_map::{EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, SegmentPaint};
use super::episode_tabs::{OpenEpisode, OpenEpisodes};
use super::population::{EpisodeSample, Population, Slice};
use super::review::{Counted, NodeBar, NodeReach};
use super::state::CurationController;
use super::threshold::{AxisUnit, DotSide, StripDot, Threshold, ThresholdStrip};
use crate::components::icons::SpinnerIcon;
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use listing::{EpisodeListing, ListedSide, MomentListing};
use rule::EpisodeConditions;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{ActionPurpose, EmptyState, ModeButton, ModeGroup, Notice, NoticeTone};

/// Which side of the rule the index lists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Showing {
    Kept,
    LeftOut,
}

/// One node's rule, read once per render from the graph and the sample it is
/// interpreted over: what it judges, and what it keeps of that.
#[derive(Clone, PartialEq)]
pub(in crate::activities::curation) struct RuleView {
    node: String,
    controller: CurationController,
    selection: Selection,
    /// The node the rule is stated in terms of: the measure above a Select,
    /// or the node that keeps by a rule of its own.
    measure: CurationNode,
    input: Population,
    output: Population,
    sample: EpisodeSample,
}

/// Which of a rule's thresholds a strip slides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuleKnob {
    /// One condition of a Select of episodes, by its place in the rule.
    Condition(usize),
    /// A measure's least value.
    Minimum,
}

/// One threshold of a rule on its strip: which it is, what it slides along,
/// every judged value, and where it stands.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct RuleStrip {
    knob: RuleKnob,
    label: &'static str,
    dots: Vec<StripDot>,
    threshold: Threshold,
}

impl RuleStrip {
    pub(in crate::activities::curation) fn threshold(&self) -> Threshold {
        self.threshold
    }
}

impl RuleView {
    pub(in crate::activities::curation) fn of(
        controller: CurationController,
        node: &str,
    ) -> Option<Self> {
        let sample = controller.sample()?;
        let (input, output) = controller.judged(node)?;
        controller.with_payload(|payload| {
            let current = payload.node(node)?;
            let measure = match current.kind() {
                CurationNodeKind::Select => payload.measure_above(node)?,
                _ => current,
            };
            Some(Self {
                node: node.to_owned(),
                controller,
                selection: current.keeps()?.clone(),
                measure: measure.clone(),
                input,
                output,
                sample,
            })
        })
    }
    /// Replace the rule. The document refuses a rule of another basis, so a
    /// slip can never re-point a node at something else.
    fn set(&self, rule: Selection) {
        let _ = self.controller.set_rule(&self.node, rule);
    }
    /// Whether the rule keeps each item that reaches it, in input order.
    fn kept(&self) -> Vec<bool> {
        match (&self.input, &self.output) {
            (Population::Episodes(input), Population::Episodes(output)) => {
                input.iter().map(|id| output.contains(id)).collect()
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
    pub(in crate::activities::curation) fn tally(&self) -> String {
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
    /// Every threshold the rule slides, each with the values it divides: one
    /// per condition of a Select of episodes, whose dots are faded where that
    /// condition leaves the episode out and rings where only another one
    /// does, or Score progress's least progress rate. Find similar slides its
    /// cutoff over its own ranking.
    pub(in crate::activities::curation) fn strips(&self) -> Vec<RuleStrip> {
        match (&self.selection, &self.input) {
            (Selection::Episodes { conditions }, Population::Episodes(episodes)) => {
                let episodes = episodes
                    .iter()
                    .filter_map(|id| self.sample.episode(id))
                    .collect::<Vec<_>>();
                let kept = episodes
                    .iter()
                    .map(|episode| self.selection.keeps_episode(episode))
                    .collect::<Vec<_>>();
                conditions
                    .iter()
                    .enumerate()
                    .map(|(index, condition)| {
                        let values = episodes
                            .iter()
                            .map(|episode| condition.fact().of(episode))
                            .collect::<Vec<_>>();
                        let sides = episodes
                            .iter()
                            .zip(&kept)
                            .map(|(episode, kept)| match (condition.holds(episode), kept) {
                                (false, _) => DotSide::LeftOut,
                                (true, false) => DotSide::LeftOutElsewhere,
                                (true, true) => DotSide::Kept,
                            })
                            .collect::<Vec<_>>();
                        let dots = StripDot::on_sides(&values, &sides);
                        let threshold = Threshold::over(
                            &dots,
                            condition.value(),
                            AxisUnit::Plain,
                            condition.comparison() == Comparison::AtLeast,
                        );
                        RuleStrip {
                            knob: RuleKnob::Condition(index),
                            label: condition.fact().label(),
                            dots,
                            threshold,
                        }
                    })
                    .collect()
            }
            (Selection::Progressing { minimum }, Population::Moments(rows)) => {
                let values = rows
                    .iter()
                    .map(|row| row.progress_rate())
                    .collect::<Vec<_>>();
                let dots = StripDot::of(&values, &self.kept());
                vec![RuleStrip {
                    knob: RuleKnob::Minimum,
                    label: "Progress rate",
                    threshold: Threshold::over(&dots, *minimum, AxisUnit::ProgressRate, true),
                    dots,
                }]
            }
            _ => Vec::new(),
        }
    }
    /// How many of the episodes reaching a Select `condition` leaves out,
    /// whatever the others say.
    fn leaves_out(&self, condition: &EpisodeCondition) -> usize {
        self.input
            .episodes()
            .iter()
            .filter_map(|id| self.sample.episode(id))
            .filter(|episode| !condition.holds(episode))
            .count()
    }
    /// Where one item of the input plays: its episode, and its stretch unless
    /// the item is the episode whole.
    fn place(&self, item: usize) -> Option<(EpisodeId, Option<(f64, f64)>)> {
        match &self.input {
            Population::Episodes(episodes) => episodes.get(item).map(|id| (id.clone(), None)),
            // A Select follows only the dataset, so no rule judges subtasks.
            Population::Subtasks(_) => None,
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
            Population::Subtasks(_) => None,
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
        match listed_kept {
            _ if self.input.len() == 0 => match self.measure.kind() {
                CurationNodeKind::ScoreProgress(_) => "No episode is scored yet. Run scores them.",
                _ => "Nothing reaches this rule yet.",
            },
            true => "Nothing passes this rule. Loosen it above to keep something.",
            false => "Nothing is left out: everything that reaches this rule passes it.",
        }
    }
}

/// What the episode map draws for a rule: one segment per item, except
/// that moments — one per frame, thousands to an episode — are drawn as the
/// runs they make on one side of the rule, each opening at its first moment.
struct MapSegments {
    slices: Vec<Slice>,
    first_items: Vec<usize>,
    kept: Vec<bool>,
}

impl MapSegments {
    fn of(view: &RuleView, kept: &[bool]) -> Self {
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

/// The workspace shown for a selected Select node: what it keeps by on the
/// node's bar, its conditions, and the review of both sides.
#[component]
pub(super) fn SelectWorkspace(controller: CurationController, node: String) -> Element {
    let Some(view) = RuleView::of(controller, &node) else {
        return rsx! {
            section { class: "curation-select-workspace",
                match controller.sample_error() {
                    Some(error) => rsx! {
                        Notice { tone: NoticeTone::Danger, "The episodes could not be read: {error}" }
                    },
                    None => rsx! { EmptyState { icon: rsx! { SpinnerIcon {} }, p { "Loading episodes…" } } },
                }
            }
        };
    };
    let strips = view.strips();
    rsx! {
        section { class: "curation-select-workspace", "aria-label": "Select",
            NodeBar {
                lead: rsx! {
                    p { class: "curation-node-bar-reads",
                        "Keeps the episodes whose recorded facts meet every condition"
                    }
                },
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                // The bar ends with what the rule gives training.
                actions: rsx! {
                    p { class: "curation-threshold-tally", role: "status", "{view.tally()}" }
                },
            }
            RuleReview { view, strips }
        }
    }
}

/// Where a rule's thresholds fall and where what it keeps lies, over one side
/// of it listed beside the recording. The episodes open beside the index,
/// and the item playing in the tab in front is the chosen one, so the map,
/// the strips, and either side of the index agree on it.
#[component]
pub(in crate::activities::curation) fn RuleReview(
    view: RuleView,
    strips: Vec<RuleStrip>,
) -> Element {
    let controller = view.controller;
    let mut showing = use_signal(|| Showing::Kept);
    let tabs = OpenEpisodes::use_open();
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
    let segments = MapSegments::of(&view, &kept);
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
        // A Select's conditions each slide their strip in their own row. A
        // rule over whole episodes keeps each one end to end or not at all,
        // so it draws no map: a column of full bars would say what the
        // counts already say.
        if matches!(view.selection, Selection::Episodes { .. }) {
            EpisodeConditions {
                view: view.clone(),
                strips,
                chosen,
                on_select: {
                    let mut open = open.clone();
                    move |item| open(item)
                },
            }
        } else {
            div { class: "curation-select-overview",
                for strip in strips {
                    ThresholdStrip {
                        key: "{strip.label}-{strip.knob:?}",
                        dots: strip.dots.clone(),
                        threshold: strip.threshold,
                        chosen,
                        label: format!("{} threshold", strip.label),
                        on_select: {
                            let mut open = open.clone();
                            move |item| open(item)
                        },
                        on_threshold: {
                            let view = view.clone();
                            move |value| view.set(view.selection.clone().with_minimum(value))
                        },
                    }
                }
                // Where the kept parts lie inside their episodes.
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
        if matches!(view.input, Population::Moments(_)) {
            MomentListing { controller, side, tabs, sides }
        } else {
            EpisodeListing { controller, side, tabs, sides }
        }
    }
}
