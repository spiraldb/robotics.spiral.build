//! The dataset node: the step it starts every path with, and its workspace —
//! name the dataset the graph reads, then walk the episodes it is interpreted
//! over and watch one with the Data view's own player, so a curation starts
//! from the recording rather than from a table.
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, DatasetSummary};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, EmptyState, Notice, NoticeTone, Remount, TextInput,
    TextInputKind,
};

use super::super::document::{CurationGraph, EpisodeFact};
use super::super::episode_tabs::{EpisodeTabs, LeadingTab, OpenEpisode, OpenEpisodes};
use super::super::path::{PathError, PathFragment, PathStep, Relation, StepSql};
use super::super::poster::{PosterCut, PosterFrames, StillCamera};
use super::super::review::{
    Counted, EvidenceHeading, IndexBar, NodeBar, RowStanding, SliceEntry, SliceFace, SliceShape,
    SubtaskReview, TabStop,
};
use super::super::state::CurationController;
use crate::activities::data::{DatasetClient, DatasetOverview};
use crate::activities::episode::{Episode, EpisodeInspection, EpisodeLabels, FeaturedCamera};
use crate::components::dataset_picker::DatasetPicker;
use crate::components::icons::SpinnerIcon;

/// The name a graph carries until it has a dataset to take one from.
const UNTITLED: &str = "Untitled curation";

/// Every episode of the dataset as one whole interval, from its first message
/// to its last, with the facts Spiral records for it. Only the published view
/// reads the dataset this way; a preview is given the episodes the sample
/// already listed.
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
            Episode::listing_sql(self),
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
/// for one.
#[component]
fn DatasetChoice(controller: CurationController) -> Element {
    let chosen = use_signal(|| None::<DatasetSummary>);
    let mut problem = use_signal(|| None::<String>);
    let use_chosen = move |_| {
        let Some(source) = chosen() else {
            return;
        };
        let selection = source.selection().clone();
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
    rsx! {
        div { class: "curation-canvas-dataset-choice",
            if let Some(problem) = problem() {
                Notice { tone: NoticeTone::Danger, class: "inline-error", "{problem}" }
            }
            EmptyState { title: "Choose a dataset",
                p { "Every measurement in this graph reads one dataset's recordings." }
                div { class: "curation-canvas-dataset-field",
                    DatasetPicker {
                        id: "curation-canvas-dataset".to_owned(),
                        chosen,
                        label: "Dataset",
                    }
                    Button {
                        purpose: ActionPurpose::Apply, variant: ButtonVariant::Outline,
                        disabled: chosen().is_none(),
                        onclick: use_chosen,
                        "Use this dataset"
                    }
                }
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
                            onclick: move |_| controller.reload_sample(), "Try again" }
                    }
                },
                (None, None) => rsx! { EmptyState { icon: rsx! { SpinnerIcon {} }, p { "Reading episodes…" } } },
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
    let labels = EpisodeLabels::of(episodes.iter().map(Episode::facts));
    // On a square-root scale, as Data's rail draws it, so one runaway take
    // does not flatten every other bar into a dot.
    let longest = episodes
        .iter()
        .filter_map(|episode| episode.facts().duration_seconds())
        .fold(0.0_f64, f64::max);
    let faces = episodes
        .iter()
        .map(|episode| {
            let share = episode
                .facts()
                .duration_seconds()
                .filter(|_| longest > 0.0)
                .map_or(0.0, |seconds| (seconds / longest).sqrt());
            let label = labels.label(episode.facts());
            (
                episode.id().clone(),
                SliceFace::episode(episode, label.head()).shaped(SliceShape::Length(share)),
            )
        })
        .filter(|(_, face)| face.says(&wanted()))
        .collect::<Vec<_>>();
    let camera = StillCamera::use_camera(controller, None);
    let stills = faces
        .iter()
        .map(|(_, face)| face.still().clone())
        .collect::<Vec<_>>();
    let frames =
        PosterFrames::use_frames(PosterCut::of(Some(&dataset), camera.as_deref(), &stills));
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
                    }
                }
                if faces.is_empty() {
                    p { class: "curation-review-note", role: "status", "No episode says “{wanted}”." }
                }
                ol { class: "curation-entries", "aria-label": "Episodes this curation reads",
                    for (position, (id, face)) in faces.into_iter().enumerate() {
                        SliceEntry {
                            key: "{id}",
                            face,
                            frames,
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
                            EvidenceHeading { subject: episode.name(), episode: Some(episode.id().clone()), facts }
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
