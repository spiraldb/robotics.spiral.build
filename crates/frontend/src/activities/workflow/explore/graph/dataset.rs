//! The dataset node's workspace: name the dataset the graph reads, then walk
//! its episodes and watch one with the Data view's own player, so an
//! exploration starts from the recording rather than from a table.
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, DatasetSummary};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, EmptyState, Fact, FactList, Notice, NoticeTone, Select,
};

use super::super::document::ExplorationPayload;
use super::super::review::PlayedEpisode;
use super::super::state::ExplorationController;
use crate::activities::data::{DatasetClient, FeaturedCamera, OpenedEpisode};
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
        let sql = format!(
            "SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, \
             message_start_time, message_end_time, duration_ns, channel_count \
             FROM spql.mcap.episodes('{}') ORDER BY message_start_time, source_uri LIMIT {EPISODE_LIMIT}",
            dataset.sql_name().replace('\'', "''"),
        );
        DatasetClient::current()
            .run_spiral_query(sql, EPISODE_LIMIT)
            .await
            .map_err(|error| error.to_string())
            .and_then(|snapshot| {
                snapshot
                    .into_rows()
                    .into_iter()
                    .map(|row| OpenedEpisode::from_row(dataset.clone(), row))
                    .collect::<Result<Vec<_>, _>>()
            })
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
    episodes: Vec<OpenedEpisode>,
    featured: FeaturedCamera,
) -> Element {
    let mut opened = use_signal(|| 0_usize);
    let count = episodes.len();
    let index = opened().min(count.saturating_sub(1));
    let current = episodes.get(index).cloned();
    rsx! {
        header { class: "explore-canvas-dataset-head",
            FactList { mono: true,
                Fact { label: "Dataset", "{dataset.label()}" }
                Fact { label: "Episodes", "{count}" }
            }
            if count > 1 {
                div { class: "explore-canvas-episode-picker",
                    Button {
                        purpose: ActionPurpose::Navigate, variant: ButtonVariant::Outline,
                        disabled: index == 0, aria_label: "Previous episode", title: "Previous episode",
                        onclick: move |_| opened.set(index.saturating_sub(1)),
                        "‹"
                    }
                    label { r#for: "explore-canvas-episode", class: "sr-only", "Episode" }
                    Select {
                        id: "explore-canvas-episode", value: "{index}",
                        onchange: move |value: String| opened.set(value.parse().unwrap_or(0)),
                        for (position, episode) in episodes.iter().enumerate() {
                            option { value: "{position}", title: "{episode.title()}", "Episode {position + 1}" }
                        }
                    }
                    Button {
                        purpose: ActionPurpose::Navigate, variant: ButtonVariant::Outline,
                        disabled: index + 1 >= count, aria_label: "Next episode", title: "Next episode",
                        onclick: move |_| opened.set((index + 1).min(count - 1)),
                        "›"
                    }
                }
            }
        }
        match current {
            Some(episode) => rsx! {
                PlayedEpisode { key: "{episode.title()}", episode, featured_camera: featured }
            },
            None => rsx! { EmptyState { p { "This dataset has no recordings yet." } } },
        }
    }
}
