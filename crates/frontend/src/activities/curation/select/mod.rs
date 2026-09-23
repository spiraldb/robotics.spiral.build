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

use super::document::{
    CurationNode, CurationNodeKind, PopulationKind, Selection, SelectionBasis,
};
use super::episode_map::{EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, SegmentPaint};
use super::population::{EpisodeSample, Population, Slice};
use super::review::Counted;
use super::state::CurationController;
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use listing::{EpisodeListing, MomentListing, SubtaskListing};
use rule::SelectRule;
use spiral_robotics_ui::{ActionPurpose, EmptyState, ModeButton, ModeGroup, Notice, NoticeTone};
use strip::{StripDot, ThresholdStrip};

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
    output_of_curation: bool,
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
                output_of_curation: payload.outputs().iter().any(|output| output.id() == node),
            })
        })
    }
    fn kept_seconds(&self) -> f64 {
        self.output.slices(&self.sample).iter().map(Slice::seconds).sum()
    }
    fn input_seconds(&self) -> f64 {
        self.input.slices(&self.sample).iter().map(Slice::seconds).sum()
    }
    /// How much the rule keeps, in the population's own unit and in seconds
    /// of recording, which is what training is finally given.
    fn tally(&self, kind: Option<PopulationKind>) -> String {
        format!(
            "Keeps {} of {} · {} of {} recorded",
            self.output.len(),
            Counted::of_kind(kind, self.input.len()),
            self.kept_seconds().clock_label(),
            self.input_seconds().clock_label(),
        )
    }
    /// Why nothing can be judged yet, when the node above has not measured.
    fn unmeasured(&self, controller: CurationController) -> Option<String> {
        let measured = controller.with_payload(|payload| match self.measure.kind() {
            CurationNodeKind::FindSimilarSubtasks(search) => payload
                .ranking_for(self.measure.id())
                .is_some_and(|ranking| ranking.query().trim() == search.query().trim()),
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
    let chosen = use_signal(|| None::<usize>);
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
    let kind = controller.with_payload(|payload| payload.population_kind(&node));
    let unmeasured = view.unmeasured(controller);
    let spans = EpisodeSpans::of(&view.sample);
    let listed_kept = showing() == Showing::Kept;
    let left_out = view.input.len().saturating_sub(view.output.len());
    rsx! {
        section { class: "curation-select-workspace", "aria-label": "Select",
            SelectRule { controller, view: view.clone() }
            p { class: "curation-select-tally", role: "status",
                if view.output_of_curation {
                    span {
                        class: "workflow-stage-output-flag",
                        title: "One of this curation's outputs: the data a training run can use",
                        "Output"
                    }
                }
                span { "{view.tally(kind)}" }
            }
            if let Some(reason) = unmeasured {
                Notice { tone: NoticeTone::Caution, "{reason}" }
            }
            if let Some(dots) = StripDot::for_view(&view, controller) {
                ThresholdStrip { dots, threshold: view.selection.threshold(), percent: view.selection.basis() == SelectionBasis::Similarity }
            }
            div { class: "curation-select-overview",
                EpisodeMap {
                    lanes: EpisodeLanes::of(&view.input.episodes(), &SelectPaint::slices(&view), &spans),
                    paint: MapPaint::new(SelectPaint::of(&view), None),
                    summary: format!("Where the kept parts of {} lie", Counted::episodes(view.input.episodes().len())),
                    on_select: move |_| {},
                }
            }
            div { class: "curation-select-filter",
                ModeGroup { label: "Which side of the rule to list".to_owned(),
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
            }
            match &view.input {
                Population::Episodes(_) => rsx! {
                    EpisodeListing { key: "{node}", controller, view: view.clone(), kept: listed_kept, chosen }
                },
                Population::Subtasks(_) => rsx! {
                    SubtaskListing { key: "{node}", controller, view: view.clone(), kept: listed_kept, chosen }
                },
                Population::Moments(_) => rsx! {
                    MomentListing { key: "{node}", view: view.clone(), kept: listed_kept, chosen }
                },
            }
        }
    }
}

/// How the map paints a Select: every input slice, kept in ink and left-out
/// faded. Moments are drawn one second at a time, as they were judged.
struct SelectPaint;

impl SelectPaint {
    fn slices(view: &SelectView) -> Vec<Slice> {
        match &view.input {
            Population::Moments(rows) => rows
                .iter()
                .map(|row| {
                    Population::Moments(vec![row.clone()])
                        .slices(&view.sample)
                        .remove(0)
                })
                .collect(),
            input => input.slices(&view.sample),
        }
    }
    fn of(view: &SelectView) -> Vec<SegmentPaint> {
        let kept = SelectPaint::slices(&SelectView {
            input: view.output.clone(),
            ..view.clone()
        });
        Self::slices(view)
            .iter()
            .map(|slice| SegmentPaint::Kept(kept.contains(slice)))
            .collect()
    }
}
