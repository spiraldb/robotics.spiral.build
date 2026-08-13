//! Top-level Episodes and Query panels for the Data route.
//!
//! Episode selection and query execution are independent state channels. A
//! query result stays mounted while an operator selects, replays, imports, or
//! exports episodes; neither tool has to impersonate navigation for the other.

use std::collections::{BTreeMap, BTreeSet};

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_core::api::{
    EpisodeSummary, ImportResponse, PublicationStatus, SpiralQueryResponse,
};
use spiral_robotics_core::spiral::{QueryPreset, Table};

use crate::frontend::api::http as api;
use crate::frontend::app::state::studio;
use crate::frontend::screens::data::state::data_controller;

pub(crate) const DEFAULT_QUERY_ROWS: usize = 100;
const QUERY_ROW_OPTIONS: [usize; 5] = [25, 100, 250, 500, 1_000];

/// A complete, renderable snapshot of the Query tool.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryViewModel {
    pub sql: String,
    pub max_rows: usize,
    pub state: QueryState,
}

/// One immutable query execution. Editing the SQL draft or running another
/// statement never mutates an earlier tab.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryTab {
    pub id: u64,
    pub title: String,
    pub model: QueryViewModel,
}

/// Query execution state carried to the visualization region.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QueryState {
    Running,
    Ready(SpiralQueryResponse),
    Failed(String),
}

/// Dataset navigation, transfer, and per-episode actions.
#[component]
pub(crate) fn EpisodesPanel(namespace: String, spiral_available: bool) -> Element {
    let mut studio = studio();
    let controller = data_controller();
    let mut importing = use_signal(|| false);
    let mut confirming_delete = use_signal(|| None::<String>);

    // Keep the shared selection inside the active dataset without creating a
    // panel-local copy that could drift from Scene, cameras, and Timeline.
    // The comparison before `set` is essential: Dioxus effects are reactive
    // to reads, so writing `None` into an already-empty signal would schedule
    // this effect forever when a dataset has no episodes.
    use_effect(move || {
        let dataset_id = studio.selected_dataset.read().trim().to_owned();
        let episodes = studio
            .episodes()
            .into_iter()
            .filter(|episode| episode.dataset_id == dataset_id)
            .collect::<Vec<_>>();
        let current = studio.selected_episode.read().clone();
        let next = reconciled_episode_selection(current.as_deref(), &episodes);
        if current != next {
            studio.selected_episode.set(next);
        }
    });

    let dataset_id = studio.selected_dataset.read().trim().to_owned();
    let dataset_valid = spiral_robotics_core::spiral::is_dataset_id(&dataset_id);
    let all_episodes = studio.episodes();
    let dataset_ids = all_episodes
        .iter()
        .map(|episode| episode.dataset_id.clone())
        .chain(std::iter::once(dataset_id.clone()))
        .collect::<BTreeSet<_>>();
    let episodes = all_episodes
        .into_iter()
        .filter(|episode| episode.dataset_id == dataset_id)
        .collect::<Vec<_>>();
    let exportable = episodes
        .iter()
        .filter(|episode| episode.usable() && !episode.discarded)
        .count();
    let selected = studio.selected_episode.read().clone();
    let replaying = studio.replay();
    let publications = controller
        .publications()
        .into_iter()
        .map(|status| (status.episode_id.clone(), status))
        .collect::<BTreeMap<_, _>>();
    let publication_error = controller.publication_error();

    rsx! {
        section {
            class: "flex h-full min-h-0 flex-col bg-surface text-ink",
            "aria-label": "Episodes",

            header { class: "flex min-h-10 shrink-0 items-center gap-2 border-b border-line px-2",
                div { class: "data-dataset-heading min-w-0 flex-1",
                    label {
                        span { "Dataset" }
                        input {
                            class: "data-dataset-input",
                            list: "data-dataset-options",
                            value: dataset_id.clone(),
                            maxlength: "96",
                            pattern: "[A-Za-z0-9_.-]+",
                            placeholder: "pick-place-v1",
                            oninput: move |event| studio.selected_dataset.set(event.value()),
                        }
                    }
                    datalist { id: "data-dataset-options",
                        for id in dataset_ids { option { value: "{id}" } }
                    }
                    if dataset_valid {
                        p { class: "truncate text-[10px] text-ink-faint",
                            "{episodes.len()} episodes · {exportable} exportable · {namespace}"
                        }
                    } else {
                        p { class: "truncate text-[10px] text-danger", role: "alert",
                            "Use 1–96 letters, numbers, '.', '_' or '-'."
                        }
                    }
                }
                label {
                    class: if importing() { "btn btn-outline cursor-wait text-ink-faint" } else { "btn btn-outline" },
                    title: "Import a LeRobot v2 or v3 ZIP into this dataset",
                    input {
                        class: "sr-only",
                        r#type: "file",
                        accept: ".zip,application/zip",
                        disabled: importing() || !dataset_valid,
                        onchange: move |event| {
                            let Some(file) = event.files().first().cloned() else { return };
                            let dataset_id = dataset_id.clone();
                            importing.set(true);
                            studio.run_utility("", async move {
                                let outcome = async {
                                    api::upload_file_json::<ImportResponse>(
                                        &format!("/api/episodes/import?dataset_id={dataset_id}"),
                                        &file,
                                        "application/zip",
                                    )
                                    .await
                                }
                                .await;
                                importing.set(false);
                                match outcome {
                                    Ok(result) => {
                                        if let Some(id) = result.first_episode {
                                            studio.selected_episode.set(Some(id));
                                        }
                                        studio.report_success(format!(
                                            "Imported {} {} into {dataset_id} ({} frames).",
                                            result.episodes,
                                            if result.episodes == 1 { "episode" } else { "episodes" },
                                            result.frames,
                                        ));
                                        Ok(())
                                    }
                                    Err(error) => Err(error),
                                }
                            });
                        }
                    }
                    if importing() { "Importing…" } else { "Import" }
                }
                if exportable == 0 || !dataset_valid {
                    button {
                        r#type: "button",
                        class: "btn btn-outline",
                        disabled: true,
                        title: "Record or import a usable episode before exporting",
                        "Export"
                    }
                } else {
                    a {
                        class: "btn btn-outline",
                        href: format!("/api/episodes/export.zip?dataset_id={dataset_id}"),
                        download: format!("{dataset_id}-lerobot-v3.zip"),
                        title: "Download the exportable episodes as a LeRobot v3 ZIP",
                        "Export"
                    }
                }
            }

            div { class: "min-h-0 flex-1 overflow-y-auto",
                if let Some(error) = publication_error {
                    div { class: "mx-2 mt-2 rounded bg-danger-soft px-2 py-1.5 text-[11px] text-danger", role: "alert",
                        strong { class: "block font-medium", "Storage state unavailable" }
                        span { "{error}" }
                    }
                }
                if episodes.is_empty() {
                    div { class: "px-3 py-8 text-center",
                        strong { class: "block text-[12px] font-medium text-ink", "No episodes yet" }
                        p { class: "mt-1 text-[11px] text-ink-faint",
                            "Record in Live mode or import a LeRobot dataset."
                        }
                    }
                } else {
                    div { class: "sticky top-0 z-10 flex min-h-6 items-center border-b border-line bg-sunken px-2 text-[10px] text-ink-faint",
                        span { class: "flex-1", "Episode" }
                        span { "Newest first" }
                    }
                    ul { class: "divide-y divide-line", "aria-label": "Dataset episodes",
                        for episode in episodes.iter().rev().cloned() {
                            EpisodeRow {
                                key: "{episode.id}",
                                episode: episode.clone(),
                                storage_status: publications.get(&episode.id).cloned(),
                                spiral_available,
                                selected: selected.as_deref() == Some(episode.id.as_str()),
                                playing: replaying.as_ref().is_some_and(|progress| progress.episode == episode.id),
                                confirming_delete: confirming_delete.read().as_deref() == Some(episode.id.as_str()),
                                on_select: move |chosen: EpisodeSummary| {
                                    confirming_delete.set(None);
                                    if studio.selected_episode.peek().as_deref() != Some(chosen.id.as_str()) {
                                        studio.selected_episode.set(Some(chosen.id));
                                    }
                                },
                                on_delete: move |id: String| {
                                    if confirming_delete.read().as_deref() == Some(id.as_str()) {
                                        confirming_delete.set(None);
                                        delete_episode(studio, id);
                                    } else {
                                        confirming_delete.set(Some(id));
                                    }
                                },
                                on_cancel_delete: move |_| confirming_delete.set(None),
                                on_push: move |id: String| controller.push_to_spiral(studio, id),
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Read-only SQL controls. Results render in their own sibling panel and stay
/// available while this panel is moved, tabbed, or obscured.
#[component]
pub(crate) fn QueryPanel(namespace: String, dataset_id: String, query_available: bool) -> Element {
    let studio = studio();
    let controller = data_controller();
    let dataset_valid = spiral_robotics_core::spiral::is_dataset_id(&dataset_id);
    let can_query = query_available && dataset_valid;
    let query_running = controller.query_running();
    let run_query = move || controller.run_query(studio, can_query);

    rsx! {
        section { class: "flex h-full min-h-0 flex-col bg-surface text-ink", "aria-label": "Query",
            if !dataset_valid {
                div { class: "mx-2 mt-2 rounded bg-danger-soft px-2 py-1.5 text-[11px] text-danger", role: "alert",
                    strong { class: "block font-medium", "Fix the dataset ID" }
                    span { "Use 1–96 letters, numbers, '.', '_' or '-' before running a dataset query." }
                }
            } else if !query_available {
                div { class: "mx-2 mt-2 rounded border border-line bg-sunken px-2 py-1.5 text-[11px] text-ink-soft", role: "status",
                    strong { class: "block font-medium text-ink", "Configure SpiralDB to run Query" }
                    span { "Open Settings and configure a Spiral catalog, project, and namespace." }
                }
            }
            div { class: "flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-line px-2 py-1",
                span { class: "text-[10px] text-ink-faint", "Examples" }
                for preset in QueryPreset::ALL {
                    button {
                        key: "{preset.label()}",
                        r#type: "button",
                        class: "link min-h-6",
                        onclick: {
                            let namespace = namespace.clone();
                            let dataset_id = dataset_id.clone();
                            move |_| controller.set_sql(preset_sql(&namespace, &dataset_id, preset))
                        },
                        "{preset.label()}"
                    }
                }
            }
            details { class: "query-schema-browser",
                summary {
                    span { "Database tables" }
                    small { "{Table::ALL.len()} relations · {namespace}" }
                }
                div { class: "query-schema-list",
                    for table in Table::ALL {
                        button {
                            key: "{table.name()}",
                            r#type: "button",
                            class: "query-schema-row",
                            onclick: {
                                let namespace = namespace.clone();
                                let dataset_id = dataset_id.clone();
                                move |_| controller.set_sql(preset_sql(
                                    &namespace,
                                    &dataset_id,
                                    table_preset(table),
                                ))
                            },
                            span {
                                strong { "{table.name()}" }
                                small { "{table_description(table)}" }
                            }
                            code { {table.columns().join(", ")} }
                        }
                    }
                }
            }
            label { class: "flex min-h-0 flex-1 flex-col px-2 pt-2",
                span { class: "label", "Read-only SQL" }
                textarea {
                    class: "query-input min-h-32 flex-1 resize-none",
                    spellcheck: "false",
                    value: controller.sql(),
                    oninput: move |event| controller.set_sql(event.value()),
                    onkeydown: move |event| {
                        if event.key() == Key::Enter
                            && (event.modifiers().ctrl() || event.modifiers().meta())
                        {
                            event.prevent_default();
                            run_query();
                        }
                    },
                }
            }
            div { class: "flex shrink-0 items-end gap-2 px-2 py-2",
                label { class: "w-24",
                    span { class: "label", "Max rows" }
                    select {
                        class: "field",
                        value: "{controller.max_rows()}",
                        onchange: move |event| {
                            let rows = event.value().parse().unwrap_or(DEFAULT_QUERY_ROWS);
                            controller.set_max_rows(rows);
                        },
                        for option in QUERY_ROW_OPTIONS {
                            option { value: "{option}", selected: controller.max_rows() == option, "{option}" }
                        }
                    }
                }
                p { class: "min-w-[7rem] flex-1 text-[10px] leading-snug text-ink-faint",
                    span { class: "block", "Keeps newest 8" }
                    span { class: "block whitespace-nowrap", "⌘/Ctrl Enter" }
                }
                if query_running {
                    button {
                        r#type: "button",
                        class: "btn btn-outline",
                        onclick: move |_| controller.cancel_query(),
                        "Cancel query"
                    }
                } else {
                    button {
                        r#type: "button",
                        class: "btn btn-primary",
                        disabled: !can_query || controller.sql().trim().is_empty(),
                        onclick: move |_| run_query(),
                        "Run query"
                    }
                }
            }
        }
    }
}

#[component]
fn EpisodeRow(
    episode: EpisodeSummary,
    storage_status: Option<PublicationStatus>,
    spiral_available: bool,
    selected: bool,
    playing: bool,
    confirming_delete: bool,
    on_select: EventHandler<EpisodeSummary>,
    on_delete: EventHandler<String>,
    on_cancel_delete: EventHandler<()>,
    on_push: EventHandler<String>,
) -> Element {
    let state = if playing {
        "Replaying"
    } else if episode.problem.is_some() {
        "Incomplete"
    } else if episode.discarded {
        "Discarded"
    } else {
        "Recorded"
    };
    let storage_state = episode_storage_state(storage_status.as_ref());
    let storage_class = storage_state.class_name();
    let storage_title = episode_storage_title(storage_status.as_ref());
    let can_push = episode.usable() && !episode.discarded;
    let push_disabled = !spiral_available || !can_push;
    let push_title = if !spiral_available {
        "Configure SpiralDB in Settings before pushing this episode"
    } else if !can_push {
        "Only complete, kept episodes can be pushed to SpiralDB"
    } else if storage_state == EpisodeStorageState::Failed {
        "Retry pushing this episode to SpiralDB"
    } else {
        "Push this local episode to SpiralDB"
    };
    let delete_title = if storage_state == EpisodeStorageState::Synced {
        "Remove this computer's copy; the SpiralDB copy is kept"
    } else {
        "Delete this local episode"
    };

    rsx! {
        li {
            class: if selected { "bg-selected" } else { "bg-surface hover:bg-hover" },
            article { class: "data-episode-row flex min-h-12 flex-wrap items-center gap-1 px-1.5 py-1",
                button {
                    r#type: "button",
                    class: "data-episode-select min-w-0 flex-1 rounded px-1 py-0.5 text-left focus-visible:outline-offset-0",
                    "aria-pressed": selected,
                    onclick: {
                        let episode = episode.clone();
                        move |_| on_select.call(episode.clone())
                    },
                    span { class: "flex items-baseline gap-1.5",
                        strong { class: "truncate font-mono text-[11px] font-medium text-ink", "{short_id(&episode.id)}" }
                        span { class: if playing { "text-[10px] text-accent" } else { "text-[10px] text-ink-faint" }, "{state}" }
                        span { class: "data-storage-state {storage_class}", title: "{storage_title}", "{storage_state.label()}" }
                        span { class: "ml-auto shrink-0 font-mono text-[10px] text-ink-soft", "{format_duration(episode.duration_seconds)}" }
                    }
                    span { class: "mt-0.5 flex min-w-0 items-center gap-1.5 text-[10px] text-ink-faint",
                        span { class: "truncate", "{episode.source}" }
                        span { class: "ml-auto shrink-0 font-mono", "{episode.frames}f · {episode.sample_hz:.0}Hz" }
                    }
                }
                if confirming_delete {
                    div { class: "data-delete-confirmation", role: "alert",
                        span {
                            if storage_state == EpisodeStorageState::Synced {
                                "Remove the local copy? The SpiralDB copy stays."
                            } else {
                                "Delete this local episode permanently?"
                            }
                        }
                        button {
                            r#type: "button",
                            class: "btn btn-danger bg-danger-soft",
                            onclick: {
                                let id = episode.id.clone();
                                move |_| on_delete.call(id.clone())
                            },
                            "Confirm"
                        }
                        button {
                            r#type: "button",
                            class: "btn btn-outline",
                            onclick: move |_| on_cancel_delete.call(()),
                            "Cancel"
                        }
                    }
                } else {
                    div { class: "data-episode-actions",
                        if matches!(storage_state, EpisodeStorageState::Local | EpisodeStorageState::Failed) {
                            button {
                                r#type: "button",
                                class: "btn data-push-action",
                                disabled: push_disabled,
                                title: push_title,
                                onclick: {
                                    let id = episode.id.clone();
                                    move |_| on_push.call(id.clone())
                                },
                                if storage_state == EpisodeStorageState::Failed { "Retry" } else { "Push" }
                            }
                        }
                        button {
                            r#type: "button",
                            class: "btn btn-danger data-delete-action",
                            title: delete_title,
                            onclick: {
                                let id = episode.id.clone();
                                move |_| on_delete.call(id.clone())
                            },
                            "Delete"
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EpisodeStorageState {
    Local,
    Queued,
    Pushing,
    Synced,
    Failed,
}

impl EpisodeStorageState {
    const fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Queued => "Queued",
            Self::Pushing => "Pushing…",
            Self::Synced => "Synced",
            Self::Failed => "Failed",
        }
    }

    const fn class_name(self) -> &'static str {
        match self {
            Self::Local => "data-storage-local",
            Self::Queued | Self::Pushing => "data-storage-active",
            Self::Synced => "data-storage-synced",
            Self::Failed => "data-storage-failed",
        }
    }
}

fn episode_storage_state(status: Option<&PublicationStatus>) -> EpisodeStorageState {
    let Some(status) = status else {
        return EpisodeStorageState::Local;
    };
    match status.spiral.as_deref() {
        Some("queued") => EpisodeStorageState::Queued,
        Some("publishing") => EpisodeStorageState::Pushing,
        Some("published") => EpisodeStorageState::Synced,
        Some(value) if value.starts_with("failed:") => EpisodeStorageState::Failed,
        _ => EpisodeStorageState::Local,
    }
}

fn episode_storage_title(status: Option<&PublicationStatus>) -> String {
    match episode_storage_state(status) {
        EpisodeStorageState::Local => "Stored only on this computer".to_owned(),
        EpisodeStorageState::Queued => "Waiting to push to SpiralDB".to_owned(),
        EpisodeStorageState::Pushing => "Pushing this episode to SpiralDB".to_owned(),
        EpisodeStorageState::Synced => "Stored in SpiralDB".to_owned(),
        EpisodeStorageState::Failed => status
            .and_then(|status| status.spiral.as_deref())
            .and_then(|value| value.strip_prefix("failed:"))
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .unwrap_or("Could not save to SpiralDB. Check the connection and retry.")
            .to_owned(),
    }
}

#[component]
pub(crate) fn QueryResultView() -> Element {
    let controller = data_controller();
    let tabs = controller.query_tabs();
    let active_id = controller.active_query();
    let active = active_id
        .and_then(|id| tabs.iter().find(|tab| tab.id == id))
        .or_else(|| tabs.last())
        .cloned();
    let tab_ids = tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();

    rsx! {
        section { class: "flex h-full min-h-0 flex-col bg-surface", "aria-label": "Query results",
            if !tabs.is_empty() {
                div { class: "query-result-tabs", role: "tablist", "aria-label": "Query results",
                    for tab in tabs.iter() {
                        button {
                            id: "query-tab-{tab.id}",
                            class: if active_id == Some(tab.id) { "query-result-tab query-result-tab-active" } else { "query-result-tab" },
                            r#type: "button",
                            role: "tab",
                            tabindex: if active_id == Some(tab.id) { "0" } else { "-1" },
                            "aria-selected": active_id == Some(tab.id),
                            "aria-controls": "query-panel-{tab.id}",
                            onclick: {
                                let id = tab.id;
                                move |_| controller.select_query(id)
                            },
                            onkeydown: {
                                let ids = tab_ids.clone();
                                let id = tab.id;
                                move |event| {
                                    let target = match event.key() {
                                        Key::ArrowLeft => query_tab_neighbor(&ids, id, false),
                                        Key::ArrowRight => query_tab_neighbor(&ids, id, true),
                                        Key::Home => ids.first().copied(),
                                        Key::End => ids.last().copied(),
                                        Key::Delete => {
                                            let target = query_tab_neighbor_after_close(&ids, id);
                                            controller.close_query(id);
                                            target
                                        }
                                        _ => return,
                                    };
                                    event.prevent_default();
                                    if let Some(target) = target {
                                        controller.select_query(target);
                                        focus_query_tab(target);
                                    }
                                }
                            },
                            span { class: query_tab_state_class(&tab.model.state), "aria-hidden": "true" }
                            "{tab.title}"
                        }
                    }
                }
            }
            if let Some(tab) = active {
                div {
                    id: "query-panel-{tab.id}",
                    class: "query-result-panel",
                    role: "tabpanel",
                    tabindex: "0",
                    "aria-labelledby": "query-tab-{tab.id}",
                    header { class: "query-result-meta",
                        span { "Limit · {tab.model.max_rows} rows" }
                        details {
                            summary { "SQL" }
                            code { "{tab.model.sql}" }
                        }
                        button {
                            r#type: "button",
                            class: "query-active-tab-close",
                            "aria-label": "Close {tab.title}",
                            onclick: {
                                let ids = tab_ids.clone();
                                let id = tab.id;
                                move |_| {
                                    let target = query_tab_neighbor_after_close(&ids, id);
                                    controller.close_query(id);
                                    if let Some(target) = target {
                                        focus_query_tab(target);
                                    }
                                }
                            },
                            svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
                                path { d: "M4.5 4.5l7 7m0-7l-7 7", fill: "none", stroke: "currentColor", stroke_width: "1.5", stroke_linecap: "round" }
                            }
                        }
                        if let QueryState::Ready(result) = &tab.model.state {
                            span { class: "font-mono tabular-nums", role: "status", "{result.rows.len()} rows · {result.elapsed_ms} ms" }
                            if result.truncated { span { class: "chip chip-caution", "Truncated" } }
                        }
                    }
                    {render_query_result(&tab.model)}
                }
            } else {
                div { class: "flex flex-1 items-center justify-center p-6 text-center",
                    div {
                        strong { class: "block text-[12px] font-medium text-ink", "No query results" }
                        p { class: "mt-1 max-w-[52ch] text-[11px] text-ink-faint",
                            "Run a read-only query to inspect data."
                        }
                    }
                }
            }
        }
    }
}

fn query_tab_neighbor(ids: &[u64], current: u64, forward: bool) -> Option<u64> {
    let index = ids.iter().position(|id| *id == current)?;
    let offset = if forward {
        (index + 1) % ids.len()
    } else {
        (index + ids.len() - 1) % ids.len()
    };
    ids.get(offset).copied()
}

fn query_tab_neighbor_after_close(ids: &[u64], current: u64) -> Option<u64> {
    let index = ids.iter().position(|id| *id == current)?;
    index
        .checked_sub(1)
        .and_then(|index| ids.get(index))
        .or_else(|| ids.get(index + 1))
        .copied()
}

#[cfg(feature = "web")]
fn focus_query_tab(id: u64) {
    use wasm_bindgen::JsCast as _;

    let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(&format!("query-tab-{id}")))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let _ = element.focus();
}

#[cfg(not(feature = "web"))]
fn focus_query_tab(_id: u64) {}

fn render_query_result(model: &QueryViewModel) -> Element {
    rsx! {
        match &model.state {
                QueryState::Running => rsx! {
                    div { class: "flex flex-1 items-center justify-center p-6 text-[11px] text-ink-soft", role: "status",
                        span { class: "mr-2 h-1.5 w-1.5 rounded-full bg-accent" }
                        "Running query…"
                    }
                },
                QueryState::Failed(error) => rsx! {
                    div { class: "m-3 rounded bg-danger-soft px-3 py-2 text-[11px] text-danger", role: "alert",
                        strong { class: "block font-medium", "Query failed" }
                        p { class: "mt-0.5", "{error}" }
                    }
                },
                QueryState::Ready(result) if result.rows.is_empty() => rsx! {
                    div { class: "flex flex-1 items-center justify-center p-6 text-center",
                        div {
                            strong { class: "block text-[12px] font-medium text-ink", "No rows" }
                            p { class: "mt-1 text-[11px] text-ink-faint", "The query completed successfully." }
                        }
                    }
                },
                QueryState::Ready(result) => rsx! {
                    div { class: "min-h-0 flex-1 overflow-auto",
                        table { class: "query-table",
                            thead { tr {
                                for column in &result.columns {
                                    th { key: "{column.name}", scope: "col",
                                        title: if column.nullable {
                                            format!("{} · nullable", column.data_type)
                                        } else {
                                            column.data_type.clone()
                                        },
                                        "{column.name}"
                                    }
                                }
                            } }
                            tbody {
                                for (index, row) in result.rows.iter().enumerate() {
                                    tr { key: "{index}",
                                        for column in &result.columns {
                                            {
                                                let value = query_cell(row.get(&column.name));
                                                rsx! { td { key: "{column.name}", title: "{value}", "{value}" } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
    }
}

fn query_tab_state_class(state: &QueryState) -> &'static str {
    match state {
        QueryState::Running => "query-tab-state query-tab-running",
        QueryState::Ready(_) => "query-tab-state query-tab-ready",
        QueryState::Failed(_) => "query-tab-state query-tab-failed",
    }
}

fn preset_sql(namespace: &str, dataset_id: &str, preset: QueryPreset) -> String {
    let dataset_id = dataset_id.replace('\'', "''");
    let episodes = spiral_robotics_core::spiral::relation(namespace, Table::Episodes);
    let relation = spiral_robotics_core::spiral::relation(namespace, preset.table());
    match preset {
        QueryPreset::Datasets => preset.sql(namespace),
        QueryPreset::Episodes => format!(
            "SELECT * FROM {relation} WHERE dataset_id = '{dataset_id}' ORDER BY started_ms DESC LIMIT 100"
        ),
        QueryPreset::EpisodeStreams => format!(
            "SELECT s.* FROM {relation} AS s JOIN {episodes} AS e ON e.episode_id = s.episode_id WHERE e.dataset_id = '{dataset_id}' ORDER BY s.episode_id DESC, s.stream_id LIMIT 100"
        ),
        QueryPreset::JointSamples => format!(
            "SELECT j.* FROM {relation} AS j JOIN {episodes} AS e ON e.episode_id = j.episode_id WHERE e.dataset_id = '{dataset_id}' ORDER BY j.episode_id DESC, j.stream_id, j.sample_index LIMIT 100"
        ),
        QueryPreset::VideoAssets => format!(
            "SELECT v.episode_id, v.stream_id, v.camera_id, v.byte_size, v.width, v.height, v.fps, v.frames FROM {relation} AS v JOIN {episodes} AS e ON e.episode_id = v.episode_id WHERE e.dataset_id = '{dataset_id}' ORDER BY v.episode_id DESC, v.stream_id LIMIT 100"
        ),
        QueryPreset::TrainingCheckpoints => preset.sql(namespace),
    }
}

pub(crate) fn default_query_sql(namespace: &str, dataset_id: &str) -> String {
    preset_sql(namespace, dataset_id, QueryPreset::Episodes)
}

fn table_preset(table: Table) -> QueryPreset {
    match table {
        Table::Datasets => QueryPreset::Datasets,
        Table::Episodes => QueryPreset::Episodes,
        Table::EpisodeStreams => QueryPreset::EpisodeStreams,
        Table::JointSamples => QueryPreset::JointSamples,
        Table::VideoAssets => QueryPreset::VideoAssets,
        Table::TrainingCheckpoints => QueryPreset::TrainingCheckpoints,
    }
}

fn table_description(table: Table) -> &'static str {
    match table {
        Table::Datasets => "Dataset identity and metadata",
        Table::Episodes => "Completed episodes and their datasets",
        Table::EpisodeStreams => "Available joint and camera streams",
        Table::JointSamples => "Synchronized state and action samples",
        Table::VideoAssets => "Recorded MP4 segments and media metadata",
        Table::TrainingCheckpoints => "Saved checkpoints by training run",
    }
}

fn preferred_episode(episodes: &[EpisodeSummary]) -> Option<&EpisodeSummary> {
    episodes
        .iter()
        .rev()
        .find(|episode| episode.usable() && !episode.discarded)
        .or_else(|| episodes.last())
}

fn reconciled_episode_selection(
    current: Option<&str>,
    episodes: &[EpisodeSummary],
) -> Option<String> {
    current
        .filter(|id| episodes.iter().any(|episode| episode.id == *id))
        .map(str::to_owned)
        .or_else(|| preferred_episode(episodes).map(|episode| episode.id.clone()))
}

fn delete_episode(studio: crate::frontend::app::state::Studio, id: String) {
    let mut selected = studio.selected_episode;
    let path = format!("/api/episodes/{id}");
    let deleted_id = id.clone();
    studio.run_utility(format!("Deleted {id}."), async move {
        api::send::<()>(Method::DELETE, &path, None).await?;
        // A slower deletion must not clear a newer choice made while the
        // request was in flight.
        if selected.peek().as_deref() == Some(deleted_id.as_str()) {
            selected.set(None);
        }
        Ok(())
    });
}

pub(crate) fn short_id(id: &str) -> String {
    id.strip_prefix("episode_")
        .and_then(|number| number.parse::<u64>().ok())
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| id.to_owned())
}

fn format_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "0:00.0".to_owned();
    }
    let minutes = (seconds / 60.0).floor() as u64;
    let remainder = seconds - minutes as f64 * 60.0;
    format!("{minutes}:{remainder:04.1}")
}

fn query_cell(value: Option<&serde_json::Value>) -> String {
    let rendered = match value {
        None | Some(serde_json::Value::Null) => "null".to_owned(),
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(value) => serde_json::to_string(value).unwrap_or_default(),
    };
    const LIMIT: usize = 240;
    if rendered.chars().count() > LIMIT {
        format!("{}…", rendered.chars().take(LIMIT).collect::<String>())
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(id: &str, usable: bool, discarded: bool) -> EpisodeSummary {
        EpisodeSummary {
            dataset_id: "default".to_owned(),
            id: id.to_owned(),
            file: format!("{id}.jsonl"),
            arms: Vec::new(),
            frames: 1,
            duration_seconds: 1.0,
            recording_gaps: Vec::new(),
            sample_hz: 60.0,
            started_ms: 0,
            source: "test".to_owned(),
            max_follow_error_degrees: None,
            discarded,
            problem: (!usable).then(|| "unavailable".to_owned()),
        }
    }

    #[test]
    fn episode_selection_reconciliation_is_stable_when_the_dataset_is_empty() {
        assert_eq!(reconciled_episode_selection(None, &[]), None);
        assert_eq!(reconciled_episode_selection(Some("deleted"), &[]), None);
    }

    #[test]
    fn episode_selection_reconciliation_preserves_valid_choices_and_replaces_stale_ones() {
        let episodes = vec![
            episode("older", true, false),
            episode("discarded", true, true),
            episode("newer", true, false),
        ];
        assert_eq!(
            reconciled_episode_selection(Some("older"), &episodes).as_deref(),
            Some("older")
        );
        assert_eq!(
            reconciled_episode_selection(Some("deleted"), &episodes).as_deref(),
            Some("newer")
        );
    }

    #[test]
    fn presets_are_scoped_to_the_current_dataset() {
        assert_eq!(
            preset_sql("training", "pick-place", QueryPreset::JointSamples),
            "SELECT j.* FROM training.joint_samples AS j JOIN training.episodes AS e ON e.episode_id = j.episode_id WHERE e.dataset_id = 'pick-place' ORDER BY j.episode_id DESC, j.stream_id, j.sample_index LIMIT 100"
        );
    }

    #[test]
    fn result_tab_keyboard_navigation_wraps_and_close_prefers_the_left_neighbor() {
        let ids = [3, 7, 9];
        assert_eq!(query_tab_neighbor(&ids, 3, false), Some(9));
        assert_eq!(query_tab_neighbor(&ids, 9, true), Some(3));
        assert_eq!(query_tab_neighbor_after_close(&ids, 7), Some(3));
        assert_eq!(query_tab_neighbor_after_close(&ids, 3), Some(7));
    }

    #[test]
    fn generated_episode_ids_read_as_take_numbers() {
        assert_eq!(short_id("episode_000024"), "#24");
        assert_eq!(short_id("imported-pass"), "imported-pass");
    }

    #[test]
    fn spiral_storage_state_keeps_episode_order_independent_from_location() {
        let status = |state: &str, spiral: Option<&str>| PublicationStatus {
            episode_id: "episode_000024".to_owned(),
            state: state.to_owned(),
            spiral: spiral.map(str::to_owned),
            ..PublicationStatus::default()
        };

        assert_eq!(episode_storage_state(None), EpisodeStorageState::Local);
        assert_eq!(
            episode_storage_state(Some(&status("queued", Some("queued")))),
            EpisodeStorageState::Queued
        );
        assert_eq!(
            episode_storage_state(Some(&status("publishing", Some("publishing")))),
            EpisodeStorageState::Pushing
        );
        assert_eq!(
            episode_storage_state(Some(&status("published", Some("published")))),
            EpisodeStorageState::Synced
        );
        assert_eq!(
            episode_storage_state(Some(&status("failed", Some("failed: unavailable")))),
            EpisodeStorageState::Failed
        );
        // A success at a different configured destination must not claim the
        // episode is present in SpiralDB.
        assert_eq!(
            episode_storage_state(Some(&status("published", None))),
            EpisodeStorageState::Local
        );
    }

    #[test]
    fn failed_spiral_status_explains_the_retry() {
        let status = PublicationStatus {
            episode_id: "episode_000024".to_owned(),
            state: "failed".to_owned(),
            spiral: Some("failed: destination unavailable".to_owned()),
            ..PublicationStatus::default()
        };
        assert_eq!(
            episode_storage_title(Some(&status)),
            "destination unavailable"
        );
    }
}
