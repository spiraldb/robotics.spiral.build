//! The dataset node: the step it starts every path with, and its workspace —
//! name the dataset the graph reads, then walk the episodes it is interpreted
//! over and watch one with the Data view's own player, so a curation starts
//! from the recording rather than from a table.
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, EmptyState, Notice, NoticeTone, RefreshIcon, Remount,
    TextInput, TextInputKind,
};

use super::super::document::{CurationGraph, EpisodeFact};
use super::super::episode_tabs::{
    EpisodeTabs, LeadingTab, OpenEpisode, OpenEpisodes, WholeEpisodeTab,
};
use super::super::review::{
    Counted, IndexBar, NodeBar, RowStanding, SliceEntry, SliceFace, SubtaskReview, TabStop,
};
use super::super::session::UNTITLED;
use super::super::state::CurationController;
use super::super::thumbnail::Thumbnails;
use crate::activities::data::{CheckedEpisode, DatasetLedger, DatasetOverview};
use crate::activities::episode::{Episode, EpisodeListing, EpisodeRows, FeaturedCamera};
use crate::components::dataset_catalog::DatasetCatalog;
use crate::components::icons::SpinnerIcon;
use crate::format::ClockPresentation as _;

/// The step every path starts with: every episode of the dataset as one row,
/// with the facts its format records for it, its first message as
/// `origin_ns`, and `kept` of one range, from its first message to its last,
/// through the episode listing `Episode` uses everywhere.
pub(in crate::activities::curation) struct DatasetStep<'a>(&'a EpisodeListing);

impl<'a> DatasetStep<'a> {
    /// The entry holding it, which the facts of the table are read from.
    pub(in crate::activities::curation) const NAME: &'static str = "dataset";

    pub(in crate::activities::curation) fn new(listing: &'a EpisodeListing) -> Self {
        Self(listing)
    }
    pub(in crate::activities::curation) fn entry(&self) -> String {
        let listed = EpisodeFact::listed_by(self.0);
        let facts = listed
            .iter()
            .map(|fact| format!(", {} AS {}", fact.expression(), fact.key()))
            .collect::<String>();
        let keys = listed
            .iter()
            .map(|fact| format!(", {}", fact.key()))
            .collect::<String>();
        format!(
            "{} AS (SELECT episode_id, start_ns AS origin_ns, \
             [STRUCT_PACK(start_ns := start_ns, end_ns := end_ns)] AS kept{keys} \
             FROM (SELECT id AS episode_id, CAST(message_start_time AS BIGINT) AS start_ns, \
             CAST(message_start_time + duration_ns AS BIGINT) AS end_ns{facts} FROM ({}) listed) spans)",
            Self::NAME,
            self.0.sql(),
        )
    }
}

#[component]
pub(super) fn DatasetWorkspace(controller: CurationController) -> Element {
    let listing = controller.with_payload(|payload| payload.listing().cloned());
    // The graph is configured on one camera, so the recording leads with it
    // here too rather than with whichever topic sorts first.
    let featured = controller.with_payload(CurationGraph::configured_camera);
    match listing {
        None => rsx! { DatasetChoice { controller } },
        Some(listing) => rsx! {
            Remount { identity: listing.dataset().label(),
                DatasetEpisodes { controller, listing, featured }
            }
        },
    }
}

/// A graph without a dataset measures nothing, so the empty workspace asks
/// for one — from the ledger Data's home offers them in, a row pressed to
/// choose it, so the totals that tell datasets apart are there to choose by.
#[component]
fn DatasetChoice(controller: CurationController) -> Element {
    let catalog = DatasetCatalog::use_catalog();
    let mut problem = use_signal(|| None::<String>);
    // The graph keeps the dataset with its format, which decides how its
    // episodes are listed.
    let choose = move |listing: EpisodeListing| {
        let label = listing.dataset().display_name().to_owned();
        let chosen = controller.edit(|payload| {
            payload.set_dataset(Some(listing));
            Ok(())
        });
        problem.set(chosen.err());
        // A graph that has never been named takes its dataset's name.
        // Five saved curations all called "Untitled curation" are five
        // nobody can tell apart in the list that offers them.
        if problem.peek().is_none() && controller.name() == UNTITLED {
            controller.set_name(label);
        }
    };
    let datasets = catalog
        .datasets()
        .iter()
        .map(EpisodeListing::of)
        .collect::<Vec<_>>();
    rsx! {
        section { class: "data-glance curation-canvas-dataset-choice", "aria-label": "Choose a dataset",
            header { class: "data-glance-head",
                h2 { "Choose a dataset" }
            }
            if let Some(problem) = problem() {
                Notice { tone: NoticeTone::Danger, class: "inline-error", "{problem}" }
            }
            if let Some(error) = catalog.problem() {
                Notice { tone: NoticeTone::Danger, "Could not load datasets: {error}" }
                Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                    onclick: move |_| catalog.reread(), "Retry" }
            } else if datasets.is_empty() && catalog.reading() {
                EmptyState { icon: rsx! { SpinnerIcon {} }, p { "Loading datasets…" } }
            } else if datasets.is_empty() {
                p { class: "curation-canvas-dataset-none", "No datasets yet. Import recordings in Data." }
            } else {
                DatasetLedger { datasets, on_select: choose }
            }
        }
    }
}

/// The episodes the curation is interpreted over, one at a time, playing on
/// the shared timeline.
#[component]
fn DatasetEpisodes(
    controller: CurationController,
    listing: EpisodeListing,
    featured: FeaturedCamera,
) -> Element {
    let sample = controller.sample();
    rsx! {
        div { class: "curation-canvas-dataset",
            match (sample, controller.sample_error()) {
                (Some(sample), _) => rsx! {
                    DatasetEpisodeList {
                        controller,
                        listing: listing.clone(),
                        episodes: sample.episodes().to_vec(),
                        partial: sample.is_partial(),
                        featured: featured.clone(),
                    }
                },
                (None, Some(error)) => rsx! {
                    EmptyState { tone: NoticeTone::Danger, title: "The episodes could not be listed",
                        p { "{error}" }
                        Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                            onclick: move |_| controller.reload_sample(), "Retry" }
                    }
                },
                (None, None) => rsx! { EmptyState { icon: rsx! { SpinnerIcon {} }, p { "Loading episodes…" } } },
            }
        }
    }
}

/// The dataset's recordings beside what opens from them: its overview, as
/// Data shows it, and a tab for each episode chosen from the list.
#[component]
fn DatasetEpisodeList(
    controller: CurationController,
    listing: EpisodeListing,
    episodes: Vec<Episode>,
    /// Whether the dataset holds more episodes than the curation reads.
    partial: bool,
    featured: FeaturedCamera,
) -> Element {
    let tabs = OpenEpisodes::use_open();
    let mut wanted = use_signal(String::new);
    let rows = EpisodeRows::of(&episodes);
    // Matched the way Data's rail matches, on the id and the source path as
    // well as the words the row shows, so an id pasted from anywhere finds
    // its episode.
    let faces = episodes
        .iter()
        .filter(|episode| episode.matches(&wanted()))
        .map(|episode| (episode.id().clone(), SliceFace::episode(&rows, episode)))
        .collect::<Vec<_>>();
    let thumbnails = Thumbnails::use_camera(controller, None);
    let front = tabs.front().map(|tab| tab.episode().clone());
    let chosen = faces.iter().position(|(id, _)| front.as_ref() == Some(id));
    // The card names the dataset and its episodes; the bar ends with what
    // they hold, the recording every node's tally is a share of.
    let recorded = episodes
        .iter()
        .filter_map(|episode| episode.facts().duration_seconds())
        .sum::<f64>()
        .clock_label();
    let reads = if partial {
        format!("First {} · {recorded}", Counted::episodes(episodes.len()))
    } else {
        format!("{} · {recorded}", Counted::episodes(episodes.len()))
    };
    let opened = episodes.clone();
    rsx! {
        NodeBar {
            actions: rsx! {
                p {
                    class: "curation-threshold-tally",
                    title: "The episodes every node reads, and the recording they hold",
                    "{reads}"
                }
            },
        }
        SubtaskReview {
            index: rsx! {
                IndexBar {
                    TextInput {
                        kind: TextInputKind::Search,
                        value: wanted(),
                        placeholder: "Filter episodes",
                        "aria-label": "Filter episodes",
                        oninput: move |text| wanted.set(text),
                        onkeydown: move |event: KeyboardEvent| if event.key() == Key::Escape { wanted.set(String::new()) },
                    }
                    Button {
                        purpose: ActionPurpose::Refresh, variant: ButtonVariant::Icon,
                        aria_label: "Refresh episodes", title: "Refresh episodes",
                        onclick: move |_| controller.reload_sample(),
                        RefreshIcon {}
                    }
                }
                if faces.is_empty() {
                    p { class: "curation-review-note", role: "status", "No episodes match." }
                }
                ol { class: "curation-entries", "aria-label": "Episodes this curation reads",
                    for (position, (id, face)) in faces.into_iter().enumerate() {
                        SliceEntry {
                            key: "{id}",
                            face,
                            thumbnails,
                            standing: RowStanding::plain(chosen == Some(position))
                                .roving(TabStop::at(position, chosen)),
                            on_select: move |()| tabs.open(&id, None),
                        }
                    }
                }
            },
            evidence: rsx! {
                EpisodeTabs {
                    open: tabs,
                    leading: LeadingTab::dataset(rsx! {
                        DatasetOverview { listing: listing.clone(),
                            on_open: move |checked: CheckedEpisode| {
                                tabs.open(checked.episode().id(), checked.moment().map(|moment| (moment, moment)));
                            },
                        }
                    }),
                    render: move |tab: OpenEpisode| {
                        let Some(episode) = opened.iter().find(|episode| episode.id() == tab.episode()).cloned() else {
                            return rsx! {};
                        };
                        rsx! { WholeEpisodeTab { episode, featured_camera: featured.clone(), focus: tab.play() } }
                    },
                }
            },
        }
    }
}
