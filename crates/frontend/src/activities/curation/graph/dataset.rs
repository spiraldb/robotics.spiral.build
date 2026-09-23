//! The dataset node's workspace: name the dataset the graph reads, then walk
//! the episodes it is interpreted over and watch one with the Data view's own
//! player, so a curation starts from the recording rather than from a table.
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, DatasetSummary};
use spiral_robotics_ui::{
    ActionPurpose, ArrowDownIcon, ArrowUpIcon, Button, ButtonVariant, EmptyState, Fact, FactList,
    Notice, NoticeTone, Select,
};

use super::super::document::CurationGraph;
use super::super::state::CurationController;
use crate::activities::episode::{Episode, EpisodeInspection, EpisodeLabels, FeaturedCamera};
use crate::components::dataset_picker::DatasetPicker;
use crate::components::icons::SpinnerIcon;

/// The name a graph carries until it has a dataset to take one from.
const UNTITLED: &str = "Untitled curation";

#[component]
pub(super) fn DatasetWorkspace(controller: CurationController) -> Element {
    let dataset = controller.with_payload(|payload| payload.dataset().cloned());
    // The graph is configured on one camera, so the recording leads with it
    // here too rather than with whichever topic sorts first.
    let featured = controller.with_payload(CurationGraph::configured_camera);
    match dataset {
        None => rsx! { DatasetChoice { controller } },
        Some(dataset) => rsx! {
            DatasetEpisodes { key: "{dataset.label()}", controller, dataset, featured }
        },
    }
}

/// A graph without a dataset measures nothing, so the empty workspace asks
/// for one in the same words Curation and Workflow do.
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

/// The dataset's recordings and the one on screen. Stepping through them keeps
/// the same controls the Data view uses to open an episode.
#[component]
fn DatasetEpisodeList(
    dataset: DatasetSelection,
    episodes: Vec<Episode>,
    /// Whether the dataset holds more episodes than the curation reads.
    partial: bool,
    featured: FeaturedCamera,
) -> Element {
    let mut opened = use_signal(|| 0_usize);
    let count = episodes.len();
    let index = opened().min(count.saturating_sub(1));
    let current = episodes.get(index).cloned();
    let labels = EpisodeLabels::of(episodes.iter().map(Episode::facts));
    let mut step = move |delta: isize| {
        opened.set(
            index
                .saturating_add_signed(delta)
                .min(count.saturating_sub(1)),
        );
    };
    rsx! {
        header { class: "curation-canvas-dataset-head",
            FactList { mono: true,
                Fact { label: "Dataset", "{dataset.display_name()}" }
                Fact { label: "Episodes",
                    if partial { "First {count}" } else { "{count}" }
                }
            }
            if count > 1 {
                // The same stepper Curation steps its episodes with: ↑ and ↓
                // while a stepper button has focus, the list for a jump.
                div { class: "episode-stepper", role: "group", "aria-label": "Step between episodes",
                    span { class: "episode-steps",
                        onkeydown: move |event: KeyboardEvent| match event.key() {
                            Key::ArrowUp => { event.prevent_default(); step(-1); }
                            Key::ArrowDown => { event.prevent_default(); step(1); }
                            _ => {}
                        },
                        Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                            aria_label: "Previous episode", title: "Previous episode · ↑",
                            disabled: index == 0, onclick: move |_| step(-1), ArrowUpIcon {} }
                        Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                            aria_label: "Next episode", title: "Next episode · ↓",
                            disabled: index + 1 >= count, onclick: move |_| step(1), ArrowDownIcon {} }
                    }
                    label { r#for: "curation-canvas-episode", class: "sr-only", "Episode" }
                    Select {
                        id: "curation-canvas-episode", class: "run-select episode-select", value: "{index}",
                        onchange: move |value: String| opened.set(value.parse().unwrap_or(0)),
                        for (position, episode) in episodes.iter().enumerate() {
                            option { value: "{position}", title: "{episode.id()}",
                                "{labels.line(episode)}"
                            }
                        }
                    }
                }
            }
        }
        match current {
            Some(episode) => rsx! {
                EpisodeInspection { episode, featured_camera: featured }
            },
            None => rsx! { EmptyState { p { "This dataset has no recordings yet." } } },
        }
    }
}
