//! Select: keep part of what reaches the node, by the measure of the node
//! above it, and show exactly what is kept and what is left out.
//!
//! The workspace reads top to bottom as the decision does: the rule, in the
//! words of the node it follows; how much it keeps; where the threshold falls
//! among the values it judges; where in the episodes the kept parts lie; and
//! then the kept (or left-out) parts one at a time beside their recording.
mod listing;
mod rule;
mod strip;

use super::document::{CurationNode, CurationNodeKind, ProgressCurve, Selection, SelectionBasis};
use super::episode_map::{EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, SegmentPaint};
use super::episode_tabs::{OpenEpisode, OpenEpisodes};
use super::population::{EpisodeSample, Population, Slice};
use super::state::CurationController;
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use listing::{EpisodeListing, MomentListing, SubtaskListing};
use rule::SelectRule;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{ActionPurpose, EmptyState, ModeButton, ModeGroup, Notice, NoticeTone};
use strip::{StripDot, ThresholdScale, ThresholdStrip};

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
                measure: payload.selection_measure(node)?.clone(),
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
            (
                Population::Subtasks { rows: input, .. },
                Population::Subtasks { rows: output, .. },
            ) => input.iter().map(|row| output.contains(row)).collect(),
            (Population::Moments(input), Population::Moments(output)) => {
                input.iter().map(|row| output.contains(row)).collect()
            }
            _ => Vec::new(),
        }
    }
    /// One slice per item that reaches the rule, in input order: moments one
    /// second at a time, as they were judged.
    fn item_slices(&self) -> Vec<Slice> {
        match &self.input {
            Population::Episodes(episodes) => episodes
                .iter()
                .map(|episode| Slice::of_episode(episode, &self.sample))
                .collect(),
            Population::Subtasks { rows, .. } => rows.iter().map(Slice::of_subtask).collect(),
            Population::Moments(rows) => rows.iter().map(Slice::of_moment).collect(),
        }
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
    /// given. The counts are the Kept and Left out segments beside it.
    fn tally(&self) -> String {
        format!(
            "{} of {} recorded",
            self.kept_seconds().clock_label(),
            self.input_seconds().clock_label(),
        )
    }
    /// Where one item of the input plays: its episode, and its stretch unless
    /// the item is the episode whole.
    fn place(&self, item: usize) -> Option<(EpisodeId, Option<(f64, f64)>)> {
        match &self.input {
            Population::Episodes(episodes) => episodes.get(item).map(|id| (id.clone(), None)),
            Population::Subtasks { rows, .. } => rows.get(item).map(|row| {
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
            Population::Subtasks { rows, .. } => rows
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
    /// The finer progress curve the detection above kept for `episode`.
    fn curve(&self, episode: &EpisodeId) -> Option<ProgressCurve> {
        match self.measure.kind() {
            CurationNodeKind::IdentifyUnproductiveMoments(detection) => {
                detection.curve(episode).cloned()
            }
            _ => None,
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
    /// Why nothing can be judged yet, when the node above has not measured.
    fn unmeasured(&self, controller: CurationController) -> Option<String> {
        let measured = controller.with_payload(|payload| match self.measure.kind() {
            CurationNodeKind::FindSimilarSubtasks(_) => {
                payload.current_ranking(self.measure.id()).is_some()
            }
            CurationNodeKind::GroupSimilarSubtasks => {
                payload.grouping_for(self.measure.id()).is_some()
            }
            _ => true,
        });
        (!measured).then(|| match self.selection.basis() {
            SelectionBasis::Similarity => {
                "The search above has not answered its question yet. Run it, then choose how close a subtask must be.".to_owned()
            }
            _ => "Group the subtasks above first, then choose the categories to keep.".to_owned(),
        })
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
    let unmeasured = view.unmeasured(controller);
    let spans = EpisodeSpans::of(&view.sample);
    let kept = view.kept();
    let listed_kept = showing() == Showing::Kept;
    let left_out = kept.iter().filter(|kept| !**kept).count();
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
    let dots = StripDot::for_view(&view, controller, &kept);
    // One axis for the rule's slider and the strip, so the line on the strip
    // is always where the slider stands.
    let scale = view.selection.threshold().map(|threshold| {
        ThresholdScale::of(
            dots.as_deref().unwrap_or_default(),
            threshold,
            view.selection.basis() == SelectionBasis::Similarity,
        )
    });
    rsx! {
        section { class: "curation-select-workspace", "aria-label": "Select",
            SelectRule { controller, view: view.clone(), scale }
            if let Some(reason) = unmeasured {
                Notice { tone: NoticeTone::Caution, "{reason}" }
            }
            if let (Some(dots), Some(scale), Some(threshold)) = (dots, scale, view.selection.threshold()) {
                ThresholdStrip {
                    dots,
                    scale,
                    threshold,
                    chosen,
                    on_select: {
                        let mut open = open.clone();
                        move |item| open(item)
                    },
                }
            }
            // Where the kept parts lie inside their episodes. A rule over whole
            // episodes keeps each one end to end or not at all, so its map
            // would be a column of full bars saying what the counts below
            // already say.
            if !matches!(view.input, Population::Episodes(_)) {
                div { class: "curation-select-overview",
                    EpisodeMap {
                        lanes: EpisodeLanes::of(&view.input.episodes(), &view.item_slices(), &spans),
                        // Kept in ink, left out faded; the Kept and Left out
                        // segments under it say the same in words.
                        paint: MapPaint::new(kept.iter().map(|kept| SegmentPaint::Kept(*kept)).collect(), chosen),
                        on_select: {
                            let mut open = open.clone();
                            move |item| open(item)
                        },
                    }
                }
            }
            div { class: "curation-select-filter",
                ModeGroup { label: "Which side of the rule to list".to_owned(), class: "curation-select-sides".to_owned(),
                    ModeButton {
                        purpose: ActionPurpose::Select,
                        checked: listed_kept,
                        onclick: move |_| showing.set(Showing::Kept),
                        "Kept {view.output.len()}"
                    }
                    ModeButton {
                        purpose: ActionPurpose::Select,
                        checked: !listed_kept,
                        onclick: move |_| showing.set(Showing::LeftOut),
                        "Left out {left_out}"
                    }
                }
                // The counts are the segments; beside them, the recording
                // they add up to. Whether this is an output is the card's
                // flag in the graph.
                p { class: "curation-select-tally", role: "status", "{view.tally()}" }
            }
            match &view.input {
                Population::Episodes(_) => rsx! {
                    EpisodeListing { controller, view: view.clone(), kept, listed_kept, tabs }
                },
                Population::Subtasks { .. } => rsx! {
                    SubtaskListing { controller, view: view.clone(), kept, listed_kept, tabs }
                },
                Population::Moments(_) => rsx! {
                    MomentListing { view: view.clone(), kept, listed_kept, tabs }
                },
            }
        }
    }
}
