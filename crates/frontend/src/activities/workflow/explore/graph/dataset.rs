//! The dataset node's workspace: name the dataset the graph reads, then walk
//! its episodes and watch one with the Data view's own player, so an
//! exploration starts from the recording rather than from a table.
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, DatasetSummary};
use spiral_robotics_ui::{
    ActionPurpose, ArrowDownIcon, ArrowUpIcon, Button, ButtonVariant, EmptyState, Fact, FactList,
    Notice, NoticeTone, Select,
};

use super::super::document::ExplorationPayload;
use super::super::state::ExplorationController;
use crate::activities::episode::{
    Episode, EpisodeFactsPresentation as _, EpisodeInspection, EpisodeLabels, FeaturedCamera,
};
use crate::components::dataset_picker::DatasetPicker;
use crate::components::icons::SpinnerIcon;

/// One page of a dataset's recordings; an exploration is authored against the
/// first few of them, not the whole catalog.
const EPISODE_LIMIT: usize = 200;

/// The name a graph carries until it has a dataset to take one from.
const UNTITLED: &str = "Untitled exploration";

#[component]
pub(super) fn DatasetWorkspace(controller: ExplorationController) -> Element {
    let dataset = controller.with_payload(|payload| payload.dataset().cloned());
    // The graph is configured on one camera, so the recording leads with it
    // here too rather than with whichever topic sorts first.
    let featured = controller.with_payload(ExplorationPayload::configured_camera);
    match dataset {
        None => rsx! { DatasetChoice { controller } },
        Some(dataset) => rsx! {
            DatasetEpisodes { key: "{dataset.label()}", dataset, featured }
        },
    }
}

/// A graph without a dataset measures nothing, so the empty workspace asks
/// for one in the same words Curation and Workflow do.
#[component]
fn DatasetChoice(controller: ExplorationController) -> Element {
    let chosen = use_signal(|| None::<DatasetSummary>);
    let mut problem = use_signal(|| None::<String>);
    let use_chosen = move |_| {
        let Some(source) = chosen() else {
            return;
        };
        let selection = source.selection().clone();
        let label = selection.label();
        problem.set(
            controller
                .edit(|payload| {
                    payload.set_dataset(Some(selection));
                    Ok(())
                })
                .err(),
        );
        // A graph that has never been named takes its dataset's name, the way
        // one created from the switcher already does. Five saved explorations
        // all called "Untitled exploration" are five nobody can tell apart in
        // the list that offers them.
        if controller.name() == UNTITLED {
            controller.set_name(label);
        }
    };
    rsx! {
        div { class: "explore-canvas-dataset-choice",
            if let Some(problem) = problem() {
                Notice { tone: NoticeTone::Danger, class: "inline-error", "{problem}" }
            }
            EmptyState { title: "Choose a dataset",
                p { "Every measurement in this graph reads one dataset's recordings." }
                div { class: "explore-canvas-dataset-field",
                    DatasetPicker {
                        id: "explore-canvas-dataset".to_owned(),
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

/// The dataset's recordings, one at a time: the list is a page of episodes in
/// recording order, and the opened one plays on the shared timeline.
#[component]
fn DatasetEpisodes(dataset: DatasetSelection, featured: FeaturedCamera) -> Element {
    let episodes = use_resource(use_reactive(&dataset, |dataset| async move {
        Episode::page(&dataset, 0, EPISODE_LIMIT)
            .await
            .map(|page| page.into_episodes())
    }));
    let listing = episodes.read().clone();
    rsx! {
        div { class: "explore-canvas-dataset",
            match listing {
                Some(Ok(episodes)) => rsx! {
                    DatasetEpisodeList { dataset: dataset.clone(), episodes, featured: featured.clone() }
                },
                Some(Err(error)) => rsx! {
                    EmptyState { tone: NoticeTone::Danger, title: "The episodes could not be listed",
                        p { "{error}" }
                    }
                },
                None => rsx! { EmptyState { icon: rsx! { SpinnerIcon {} }, p { "Reading episodes…" } } },
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
        header { class: "explore-canvas-dataset-head",
            FactList { mono: true,
                Fact { label: "Dataset", "{dataset.label()}" }
                Fact { label: "Episodes", "{count}" }
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
                    label { r#for: "explore-canvas-episode", class: "sr-only", "Episode" }
                    Select {
                        id: "explore-canvas-episode", class: "run-select episode-select", value: "{index}",
                        onchange: move |value: String| opened.set(value.parse().unwrap_or(0)),
                        for (position, episode) in episodes.iter().enumerate() {
                            option { value: "{position}", title: "{episode.id()}",
                                "…{episode.id().short()} · {labels.label(episode.facts()).head()} · {episode.facts().duration_label()}"
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
