//! The dataset node: the step it starts every path with, and its workspace —
//! name the dataset the graph reads, then walk the episodes it is interpreted
//! over and watch one with the Data view's own player, so a curation starts
//! from the recording rather than from a table.
use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, EmptyState, Notice, NoticeTone, RefreshIcon, Remount,
    TextInput, TextInputKind,
};

use super::super::document::{CurationGraph, EpisodeFact};
use super::super::episode_tabs::{EpisodeTabs, LeadingTab, OpenEpisode, OpenEpisodes};
use super::super::path::{PathError, PathFragment, PathStep, Relation, StepSql};
use super::super::population::PUBLISHED_EPISODES;
use super::super::review::{
    Counted, IndexBar, NodeBar, RowStanding, SliceEntry, SliceFace, SubtaskReview, TabStop,
};
use super::super::state::CurationController;
use super::super::thumbnail::Thumbnails;
use crate::activities::data::{DatasetClient, DatasetLedger, DatasetOverview};
use crate::activities::episode::{
    Episode, EpisodeHeading, EpisodeInspection, EpisodeRows, FeaturedCamera, HeadingSubject,
};
use crate::components::dataset_picker::DatasetCatalog;
use crate::components::icons::SpinnerIcon;

/// The name a graph carries until it has a dataset to take one from.
const UNTITLED: &str = "Untitled curation";

/// The dataset's first episodes, each as one whole interval from its first
/// message to its last, with the facts Spiral records for it. Only the
/// published view reads the dataset this way, and only its first
/// [`PUBLISHED_EPISODES`], so no model below is asked about more; a preview
/// is given the episodes the sample already listed.
impl PathFragment for DatasetSelection {
    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        let facts = EpisodeFact::ALL
            .iter()
            .map(|fact| format!("{} AS {}", fact.expression(), fact.key()))
            .collect::<Vec<_>>()
            .join(", ");
        let entry = format!(
            "{} AS (SELECT id AS episode_id, CAST(message_start_time AS BIGINT) AS start_ns, \
             CAST(message_start_time + duration_ns AS BIGINT) AS end_ns, {facts} \
             FROM ({}) listed)",
            step.name(),
            Episode::first_sql(self, PUBLISHED_EPISODES),
        );
        let properties = EpisodeFact::ALL.iter().map(|fact| fact.key()).collect();
        Ok(StepSql::new(
            vec![entry],
            Relation::new(step.name(), properties),
        ))
    }
}

#[component]
pub(super) fn DatasetWorkspace(controller: CurationController) -> Element {
    let dataset = controller.with_payload(|payload| payload.dataset().cloned());
    // The graph is configured on one camera, so the recording leads with it
    // here too rather than with whichever topic sorts first.
    let featured = controller.with_payload(CurationGraph::configured_camera);
    match dataset {
        None => rsx! { DatasetChoice { controller } },
        Some(dataset) => rsx! {
            Remount { identity: dataset.label(),
                DatasetEpisodes { controller, dataset, featured }
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
    let choose = move |selection: DatasetSelection| {
        let label = selection.display_name();
        problem.set(
            controller
                .edit(|payload| {
                    payload.set_dataset(Some(selection));
                    Ok(())
                })
                .err(),
        );
        // A graph that has never been named takes its dataset's name, the way
        // one created from the switcher already does. Five saved curations
        // all called "Untitled curation" are five nobody can tell apart in
        // the list that offers them.
        if controller.name() == UNTITLED {
            controller.set_name(label);
        }
    };
    let datasets = catalog
        .datasets()
        .iter()
        .map(|summary| summary.selection().clone())
        .collect::<Vec<_>>();
    rsx! {
        section { class: "data-glance curation-canvas-dataset-choice", "aria-label": "Choose a dataset",
            header { class: "data-glance-head",
                h2 { "Choose a dataset" }
                p { "Every step of this curation reads one dataset's recordings." }
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
                p { class: "data-rail-note", "This project has no datasets yet. Import recordings in Data first." }
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
    dataset: DatasetSelection,
    featured: FeaturedCamera,
) -> Element {
    let sample = controller.sample();
    rsx! {
        div { class: "curation-canvas-dataset",
            match (sample, controller.sample_error()) {
                (Some(sample), _) => rsx! {
                    DatasetEpisodeList {
                        controller,
                        dataset: dataset.clone(),
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
    dataset: DatasetSelection,
    episodes: Vec<Episode>,
    /// Whether the dataset holds more episodes than the curation reads.
    partial: bool,
    featured: FeaturedCamera,
) -> Element {
    let tabs = OpenEpisodes::use_open();
    let mut wanted = use_signal(String::new);
    let inspection = use_resource(use_reactive((&dataset,), |(dataset,)| async move {
        DatasetClient::current()
            .inspect_dataset(dataset)
            .await
            .map_err(|error| error.to_string())
    }));
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
    // Every count below the Dataset card is over these episodes, so the bar
    // says which they are.
    let reads = if partial {
        format!(
            "Every node reads the first {} of this dataset",
            Counted::episodes(episodes.len())
        )
    } else {
        format!(
            "Every node reads these {}",
            Counted::episodes(episodes.len())
        )
    };
    let opened = episodes.clone();
    rsx! {
        NodeBar {
            lead: rsx! { p { class: "curation-node-bar-reads", "{reads}" } },
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
                    empty: "Choose an episode to watch it.".to_owned(),
                    leading: LeadingTab::dataset(rsx! {
                        DatasetOverview { dataset: dataset.clone(), inspection: inspection.read().clone() }
                    }),
                    render: move |tab: OpenEpisode| {
                        let Some(episode) = opened.iter().find(|episode| episode.id() == tab.episode()).cloned() else {
                            return rsx! {};
                        };
                        let facts = EpisodeFact::DurationSeconds.reading(&episode).into_iter().collect::<Vec<_>>();
                        rsx! {
                            EpisodeHeading { subject: HeadingSubject::episode(&episode).with_facts(facts) }
                            div { class: "curation-review-player",
                                EpisodeInspection { episode, featured_camera: featured.clone() }
                            }
                        }
                    },
                }
            },
        }
    }
}
