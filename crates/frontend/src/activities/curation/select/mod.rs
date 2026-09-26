//! Select: keep part of what reaches the node by what the node above it
//! measured, and show exactly what is kept and what is left out.
//!
//! The review is any rule's, not only a Select's: Score progress keeps its
//! moments by a rule of its own and reads them here too. It reads top to
//! bottom as the decision does: the rule, in the words of what it keeps by;
//! how much it keeps; where each threshold falls among the values it judges;
//! where in the episodes the kept parts lie; and then the kept (or left-out)
//! parts one at a time beside their recording.
mod listing;
mod rule;

pub(super) use rule::RuleSentence;

use super::document::{Comparison, CurationNode, CurationNodeKind, Selection};
use super::episode_map::{EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, SegmentPaint};
use super::episode_tabs::{OpenEpisode, OpenEpisodes};
use super::population::{EpisodeSample, Population, Slice};
use super::review::{Counted, NodeBar, NodeReach};
use super::state::CurationController;
use super::threshold::{AxisUnit, StripDot, Threshold, ThresholdStrip};
use crate::format::ClockPresentation as _;
use crate::components::icons::SpinnerIcon;
use dioxus::prelude::*;
use listing::{EpisodeListing, ListedSide, MomentListing, SubtaskListing};
use rule::RuleEdit;
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
                selection: current.keeps()?.clone(),
                measure: measure.clone(),
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
    /// condition alone leaves the episode out, or Score progress's least
    /// progress rate. A rule of categories has no axis.
    pub(in crate::activities::curation) fn strips(&self) -> Vec<RuleStrip> {
        match (&self.selection, &self.input) {
            (Selection::Episodes { conditions }, Population::Episodes(episodes)) => {
                let episodes = episodes
                    .iter()
                    .filter_map(|id| self.sample.episode(id))
                    .collect::<Vec<_>>();
                conditions
                    .iter()
                    .enumerate()
                    .map(|(index, condition)| {
                        let values = episodes
                            .iter()
                            .map(|episode| condition.fact().of(episode))
                            .collect::<Vec<_>>();
                        let passes = episodes
                            .iter()
                            .map(|episode| condition.holds(episode))
                            .collect::<Vec<_>>();
                        let dots = StripDot::of(&values, &passes);
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
            // Find similar slides its cutoff over its own ranking.
            _ => Vec::new(),
        }
    }
    /// The rule with `knob` slid to `value`.
    fn slid(&self, knob: RuleKnob, value: f64) -> Selection {
        match (knob, self.selection.clone()) {
            (RuleKnob::Condition(index), Selection::Episodes { mut conditions }) => {
                if let Some(condition) = conditions.get_mut(index) {
                    *condition = condition.at(value);
                }
                Selection::Episodes { conditions }
            }
            (_, rule) => rule.with_minimum(value),
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
            _ if self.input.len() == 0 => match self.measure.kind() {
                CurationNodeKind::ScoreProgress(_) => {
                    "No episode is scored yet. Run scores them on this machine."
                }
                CurationNodeKind::GroupSimilarSubtasks(_) => "Group the subtasks above first.",
                _ => "Nothing reaches this rule yet.",
            },
            (Selection::Categories { names }, true) if names.is_empty() => {
                "Choose the categories to keep above."
            }
            (_, true) => "Nothing passes this rule. Loosen it above to keep something.",
            (_, false) => "Nothing is left out: everything that reaches this rule passes it.",
        }
    }
    /// Why nothing can be judged yet, when the grouping above has not read
    /// any episode.
    fn unmeasured(&self) -> Option<String> {
        (matches!(
            self.measure.kind(),
            CurationNodeKind::GroupSimilarSubtasks(_)
        ) && self.input.len() == 0)
            .then(|| {
                "Group the subtasks above first, then choose the categories to keep.".to_owned()
            })
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

/// The workspace shown for a selected Select node: its rule on the node's
/// bar, what it keeps, and the review of both sides.
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
    let thresholds = strips.iter().map(RuleStrip::threshold).collect::<Vec<_>>();
    rsx! {
        section { class: "curation-select-workspace", "aria-label": "Select",
            NodeBar {
                lead: rsx! { RuleSentence { controller, view: view.clone(), thresholds } },
                note: view.unmeasured(),
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                // The bar ends with what the rule gives training.
                actions: rsx! {
                    p { class: "curation-threshold-tally", role: "status", "{view.tally()}" }
                },
            }
            RuleReview { controller, view, strips }
        }
    }
}

/// Where a rule's thresholds fall and where what it keeps lies, over one side
/// of it listed beside the recording. The episodes open beside the index,
/// and the item playing in the tab in front is the chosen one, so the map,
/// the strips, and either side of the index agree on it.
#[component]
pub(in crate::activities::curation) fn RuleReview(
    controller: CurationController,
    view: RuleView,
    strips: Vec<RuleStrip>,
) -> Element {
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
    let edit = RuleEdit::new(view.node.clone(), controller);
    let segments = MapSegments::of(&view, &kept);
    // Several conditions each slide their own strip, so each says which.
    let captioned = strips.len() > 1;
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
        if !strips.is_empty() || !matches!(view.input, Population::Episodes(_)) {
            div { class: "curation-select-overview",
                for strip in strips {
                    div { key: "{strip.label}-{strip.knob:?}", class: "curation-select-overview-strip",
                        if captioned {
                            p { class: "curation-threshold-caption", "{strip.label}" }
                        }
                        ThresholdStrip {
                            dots: strip.dots.clone(),
                            threshold: strip.threshold,
                            chosen,
                            label: format!("{} threshold", strip.label),
                            on_select: {
                                let mut open = open.clone();
                                move |item| open(item)
                            },
                            on_threshold: {
                                let (edit, view) = (edit.clone(), view.clone());
                                move |value| edit.set(view.slid(strip.knob, value))
                            },
                        }
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
