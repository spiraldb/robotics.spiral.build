//! Select: the one filter of a curation. It keeps the episodes that meet
//! every one of its conditions, narrowed to the ranges its range conditions
//! keep, and shows exactly what is kept and what is left out, and by which
//! condition.
//!
//! Its workspace reads top to bottom as the decision does: how much it
//! keeps, on the node's bar; each condition with the strip its threshold
//! slides on over the values it judges; where in the episodes the kept parts
//! lie; and then the kept (or left-out) parts one at a time beside their
//! recording. Its conditions are read in the browser (`reading`) and
//! compiled to SQL (`step`), and the two keep the same rows.
mod conditions;
mod evidence;
mod listing;
mod reading;
mod step;
#[cfg(test)]
mod tests;

pub(super) use reading::SelectReading;
pub(super) use step::SelectStep;

use super::document::{CurationGraph, CurationNodeKind, Selection};
use super::episode_map::{EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, SegmentPaint};
use super::episode_tabs::OpenEpisodes;
use super::population::{EpisodeSample, KeptRanges, Slice};
use super::review::{Counted, NodeBar, NodeReach};
use super::state::CurationController;
use crate::components::icons::SpinnerIcon;
use crate::format::ClockPresentation as _;
use conditions::SelectConditions;
use dioxus::prelude::*;
use listing::SelectListing;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{EmptyState, Notice, NoticeTone};

/// One Select, read once per render from the graph and the sample it is
/// interpreted over: its conditions, and what they make of what reaches it.
#[derive(Clone, PartialEq)]
pub(in crate::activities::curation) struct SelectView {
    node: String,
    controller: CurationController,
    selection: Selection,
    reading: SelectReading,
    sample: EpisodeSample,
}

impl SelectView {
    pub(in crate::activities::curation) fn of(
        controller: CurationController,
        node: &str,
    ) -> Option<Self> {
        let sample = controller.sample()?;
        let reading = controller.select_reading(node)?;
        let selection = controller.with_payload(|payload| match payload.node(node)?.kind() {
            CurationNodeKind::Select(selection) => Some(selection.clone()),
            _ => None,
        })?;
        Some(Self {
            node: node.to_owned(),
            controller,
            selection,
            reading,
            sample,
        })
    }
    /// Replace the conditions; the document refuses ones that read what is
    /// not above the Select.
    fn set(&self, selection: Selection) {
        let _ = self.controller.set_selection(&self.node, selection);
    }
    /// Read the graph the Select is in.
    fn with_graph<R>(&self, read: impl FnOnce(&CurationGraph) -> R) -> R {
        self.controller.with_payload(read)
    }
    /// Whether any condition narrows the kept ranges, rather than keeping
    /// or dropping whole episodes.
    fn narrows(&self) -> bool {
        self.selection
            .conditions()
            .iter()
            .any(|condition| condition.narrows())
    }
    /// What reaches the Select of `episode`.
    fn reached(&self, episode: &EpisodeId) -> KeptRanges {
        self.reading
            .input()
            .of(episode)
            .cloned()
            .unwrap_or_default()
    }
    /// What the Select keeps of `episode`.
    fn kept(&self, episode: &EpisodeId) -> KeptRanges {
        self.reading
            .output()
            .of(episode)
            .cloned()
            .unwrap_or_default()
    }
    /// How much recording the Select keeps, which is what training is
    /// finally given, of what reaches it.
    fn tally(&self) -> String {
        format!(
            "Keeps {} of {}",
            self.reading.output().seconds().clock_label(),
            self.reading.input().seconds().clock_label(),
        )
    }
    /// How many of the episodes that reach the Select keep anything.
    fn kept_in(&self) -> String {
        format!(
            "Kept in {} of {}",
            self.reading.output().len(),
            Counted::episodes(self.reading.input().len())
        )
    }
}

/// The workspace shown for a selected Select node: what it keeps on the
/// node's bar, its conditions under it, and the review of both sides.
#[component]
pub(super) fn SelectWorkspace(controller: CurationController, node: String) -> Element {
    // Every hook runs before the workspace knows whether it has anything to
    // show, so the sample arriving never changes how many there are.
    let tabs = OpenEpisodes::use_open();
    let Some(view) = SelectView::of(controller, &node) else {
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
    let narrows = view.narrows();
    rsx! {
        section { class: "curation-select-workspace", "aria-label": "Select",
            // The conditions under the bar say what the Select keeps by, so
            // the bar holds only what it keeps.
            NodeBar {
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                actions: rsx! {
                    p { class: "curation-threshold-tally", role: "status", "{view.tally()}" }
                },
            }
            SelectConditions { view: view.clone(), tabs }
            // A Select keeping or dropping whole episodes keeps each one end
            // to end or not at all, so it draws no map: a column of full bars
            // would say what the counts already say.
            if narrows {
                KeptMap { view: view.clone(), tabs }
            }
            SelectListing { view, tabs }
        }
    }
}

/// Where the kept parts lie inside the episodes that reach the Select: each
/// reached range split into what is kept, in ink, and what is left out,
/// faded.
#[component]
fn KeptMap(view: SelectView, tabs: OpenEpisodes) -> Element {
    let spans = EpisodeSpans::of(&view.sample);
    let episodes = view.reading.input().episodes();
    let mut slices = Vec::new();
    let mut paints = Vec::new();
    for episode in &episodes {
        let (reached, kept) = (view.reached(episode), view.kept(episode));
        let mut parts = kept
            .ranges()
            .iter()
            .map(|range| (*range, true))
            .chain(
                reached
                    .without(&kept)
                    .ranges()
                    .iter()
                    .map(|range| (*range, false)),
            )
            .collect::<Vec<_>>();
        parts.sort_by(|left, right| left.0.0.total_cmp(&right.0.0));
        for ((start, end), kept) in parts {
            slices.push(Slice::new(episode.clone(), start, end, None));
            paints.push(SegmentPaint::Kept(kept));
        }
    }
    let front = tabs.front();
    let chosen = front.and_then(|tab| {
        slices
            .iter()
            .position(|slice| tab.holds(slice.episode(), slice.start()))
    });
    let lanes = EpisodeLanes::of(&episodes, &slices, &spans);
    rsx! {
        div { class: "curation-select-overview",
            EpisodeMap {
                lanes,
                paint: MapPaint::new(paints, chosen),
                summary: view.kept_in(),
                on_select: move |segment: usize| {
                    if let Some(slice) = slices.get(segment) {
                        tabs.open(slice.episode(), Some((slice.start(), slice.end())));
                    }
                },
            }
        }
    }
}
