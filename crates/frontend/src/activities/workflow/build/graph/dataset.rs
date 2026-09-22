//! The dataset node's workspace: name the dataset the graph reads, then walk
//! its episodes and watch one with the Data view's own player, so a Build
//! graph starts from the recording rather than from a table.
use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, EmptyState, Fact, FactList, Notice, NoticeTone, Select,
};

use super::super::state::BuildController;
use crate::activities::data::{DatasetClient, EpisodeReview, EpisodeTransport, OpenedEpisode};
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::components::icons::SpinnerIcon;

/// One page of a dataset's recordings; a Build graph is authored against the
/// first few of them, not the whole catalog.
const EPISODE_LIMIT: usize = 200;

/// The name a graph carries until it has a dataset to take one from.
const UNTITLED_BUILD: &str = "Untitled build";

#[component]
pub(super) fn DatasetWorkspace(controller: BuildController) -> Element {
    let dataset = controller.with_payload(|payload| payload.dataset().cloned());
    match dataset {
        None => rsx! { DatasetChoice { controller } },
        Some(dataset) => rsx! { DatasetEpisodes { key: "{dataset.label()}", dataset } },
    }
}

/// A graph without a dataset measures nothing, so the empty workspace asks
/// for one in the same words Curation and Workflow do.
#[component]
fn DatasetChoice(controller: BuildController) -> Element {
    let sessions = crate::shell::project::ProjectSessionSignals::current();
    let datasets = QueryPolicy::on_demand(QueryDomain::Catalog).use_when(
        move || sessions.catalog_available(),
        || DatasetClient::current().list_datasets(),
    );
    let mut chosen = use_signal(String::new);
    let mut problem = use_signal(|| None::<String>);
    let response = datasets.read();
    // The option carries the catalog's resource id, so the graph takes the
    // catalog entry's own selection rather than one parsed back from a label.
    let source = response
        .as_ref()
        .and_then(|listed| listed.as_ref().ok())
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.resource_id() == chosen())
                .cloned()
        });
    let chosen_source = source.clone();
    let use_chosen = move |_| {
        if let Some(source) = chosen_source.clone() {
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
            // A graph that has never been named takes its dataset's name, the
            // way one created from the switcher already does. Five saved
            // builds all called "Untitled build" are five builds nobody can
            // tell apart in the list that offers them.
            if controller.name() == UNTITLED_BUILD {
                controller.set_name(label);
            }
        }
    };
    rsx! {
        div { class: "build-canvas-dataset-choice",
            if let Some(problem) = problem() {
                Notice { tone: NoticeTone::Danger, class: "inline-error", "{problem}" }
            }
            match response.as_ref() {
                Some(Ok(items)) => rsx! {
                    EmptyState { title: "Choose a dataset",
                        p { "Every measurement in this graph reads one dataset's recordings." }
                        div { class: "build-canvas-dataset-field",
                            label { r#for: "build-canvas-dataset", class: "sr-only", "Dataset" }
                            Select {
                                id: "build-canvas-dataset", value: "{chosen}",
                                disabled: items.is_empty(),
                                onchange: move |value| chosen.set(value),
                                option { value: "", disabled: true, selected: chosen().is_empty(),
                                    if items.is_empty() { "No datasets available" } else { "Choose a dataset" } }
                                for item in items {
                                    option { value: "{item.resource_id()}", "{item.selection().label()}" }
                                }
                            }
                            Button {
                                purpose: ActionPurpose::Apply, variant: ButtonVariant::Outline,
                                disabled: source.is_none(),
                                onclick: use_chosen,
                                "Use this dataset"
                            }
                        }
                    }
                },
                Some(Err(error)) => rsx! {
                    EmptyState { tone: NoticeTone::Danger, title: "The dataset catalog is unavailable",
                        p { "{error}" }
                    }
                },
                None => rsx! {
                    EmptyState { icon: rsx! { SpinnerIcon {} }, p { "Loading datasets…" } }
                },
            }
        }
    }
}

/// The dataset's recordings, one at a time: the list is a page of episodes in
/// recording order, and the opened one plays on the shared timeline.
#[component]
fn DatasetEpisodes(dataset: DatasetSelection) -> Element {
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
        div { class: "build-canvas-dataset",
            match listing {
                Some(Ok(episodes)) => rsx! {
                    DatasetEpisodeList { dataset: dataset.clone(), episodes }
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
fn DatasetEpisodeList(dataset: DatasetSelection, episodes: Vec<OpenedEpisode>) -> Element {
    let mut opened = use_signal(|| 0_usize);
    let count = episodes.len();
    let index = opened().min(count.saturating_sub(1));
    let current = episodes.get(index).cloned();
    rsx! {
        header { class: "build-canvas-dataset-head",
            FactList { mono: true,
                Fact { label: "Dataset", "{dataset.label()}" }
                Fact { label: "Episodes", "{count}" }
            }
            if count > 1 {
                div { class: "build-canvas-episode-picker",
                    Button {
                        purpose: ActionPurpose::Navigate, variant: ButtonVariant::Outline,
                        disabled: index == 0, aria_label: "Previous episode", title: "Previous episode",
                        onclick: move |_| opened.set(index.saturating_sub(1)),
                        "‹"
                    }
                    label { r#for: "build-canvas-episode", class: "sr-only", "Episode" }
                    Select {
                        id: "build-canvas-episode", value: "{index}",
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
                OpenedEpisodeStage { key: "{episode.title()}", episode }
            },
            None => rsx! { EmptyState { p { "This dataset has no recordings yet." } } },
        }
    }
}

/// One episode on its own clock, exactly as the Data and Curation views show
/// it; the transport is remade for each episode through the component key.
#[component]
fn OpenedEpisodeStage(episode: OpenedEpisode) -> Element {
    let transport = EpisodeTransport::use_opened_episode(&episode);
    rsx! { EpisodeReview { episode, transport } }
}
