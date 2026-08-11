//! The Data screen's episode and query workbench.
//!
//! This module deliberately owns the narrow operating region and the model it
//! sends to the visualization region, but not the outer Studio shell. A caller
//! keeps the latest [`DataIntent`] in a signal, renders [`QueryResultView`] (or its
//! own episode visualization) on the right, and mounts [`DataWorkbench`] on the
//! left. The component uses the existing Studio context and HTTP API, so it can
//! be integrated without introducing a second source of episode selection.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{EpisodeSummary, ImportResponse, SpiralQueryResponse};
use rerobot_core::spiral::QueryPreset;

use crate::frontend::api::http as api;
use crate::frontend::app::state::studio;
use crate::frontend::screens::data::state::data_controller;

pub(crate) const DEFAULT_QUERY_ROWS: usize = 100;
const QUERY_ROW_OPTIONS: [usize; 5] = [25, 100, 250, 500, 1_000];

/// Which tool owns the Data operating region.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum DataWorkbenchTab {
    #[default]
    Episodes,
    Query,
}

/// Everything the right-hand visualization needs to render Data mode.
///
/// Episode selection and SQL output intentionally share this one channel. The
/// shell can switch the right side without making the workbench know whether
/// that side is a tab strip, a dock, or a single contextual canvas.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DataIntent {
    Episode(EpisodeSummary),
    Query(QueryViewModel),
    EmptyEpisodes,
}

/// A complete, renderable snapshot of the Query tool.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryViewModel {
    pub sql: String,
    pub max_rows: usize,
    pub state: QueryState,
}

impl QueryViewModel {
    pub(crate) fn idle(sql: String, max_rows: usize) -> Self {
        Self {
            sql,
            max_rows,
            state: QueryState::Idle,
        }
    }
}

/// Query execution state carried to the visualization region.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QueryState {
    Idle,
    Running,
    Ready(SpiralQueryResponse),
    Failed(String),
}

/// Dataset navigation and Query controls for the left side of the Studio.
#[component]
pub(crate) fn DataWorkbench(
    dataset: String,
    query_available: bool,
    on_intent: EventHandler<DataIntent>,
) -> Element {
    let mut studio = studio();
    let controller = data_controller();
    let mut importing = use_signal(|| false);
    let mut confirming_delete = use_signal(|| None::<String>);

    // The selected episode is global Studio state. Land on a useful episode
    // once and publish it to the visualization without creating a parallel
    // local selection that could drift from the existing timeline.
    use_effect(move || {
        let episodes = studio.episodes();
        if studio.selected_episode.read().is_none()
            && let Some(episode) = preferred_episode(&episodes).cloned()
        {
            on_intent.call(DataIntent::Episode(episode));
            return;
        }
        match studio.focused_episode() {
            Some(episode) if controller.tab() == DataWorkbenchTab::Episodes => {
                controller.present_episode(studio, episode, on_intent);
            }
            None if controller.tab() == DataWorkbenchTab::Episodes => {
                controller.present_empty(on_intent);
            }
            _ => {}
        }
    });

    let episodes = studio.episodes();
    let exportable = episodes
        .iter()
        .filter(|episode| episode.usable() && !episode.discarded)
        .count();
    let selected = studio.selected_episode.read().clone();
    let replaying = studio.replay();

    let run_query = move || controller.run_query(studio, query_available, on_intent);

    rsx! {
        section {
            class: "flex h-full min-h-0 flex-col bg-surface text-ink",
            "aria-label": "Data workbench",

            header { class: "flex min-h-10 shrink-0 items-center gap-2 border-b border-line px-2",
                div { class: "min-w-0 flex-1",
                    h2 { class: "truncate text-[12px] font-semibold tracking-[-0.01em]", "{dataset}" }
                    p { class: "truncate text-[10px] text-ink-faint",
                        "{episodes.len()} episodes · {exportable} exportable"
                    }
                }
                label {
                    class: if importing() { "btn btn-outline cursor-wait text-ink-faint" } else { "btn btn-outline" },
                    title: "Import a LeRobot v2 or v3 ZIP into this dataset",
                    input {
                        class: "sr-only",
                        r#type: "file",
                        accept: ".zip,application/zip",
                        disabled: importing(),
                        onchange: move |event| {
                            let Some(file) = event.files().first().cloned() else { return };
                            importing.set(true);
                            studio.run_utility("", async move {
                                let outcome = async {
                                    let bytes = file.read_bytes().await.map_err(|error| {
                                        format!("Could not read {}: {error}", file.name())
                                    })?;
                                    api::upload_json::<ImportResponse>(
                                        "/api/episodes/import",
                                        &bytes,
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
                                            "Imported {} {} ({} frames).",
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
                if exportable == 0 {
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
                        href: "/api/episodes/export.zip",
                        download: "rerobot-lerobot-v3-dataset.zip",
                        title: "Download the exportable episodes as a LeRobot v3 ZIP",
                        "Export"
                    }
                }
            }

            nav {
                class: "grid shrink-0 grid-cols-2 gap-px border-b border-line bg-sunken p-0.5",
                "aria-label": "Data tools",
                for choice in [DataWorkbenchTab::Episodes, DataWorkbenchTab::Query] {
                    button {
                        key: "{choice.label()}",
                        r#type: "button",
                        class: if controller.tab() == choice {
                            "min-h-7 rounded-[3px] bg-surface text-[11px] font-medium text-ink shadow-sm"
                        } else {
                            "min-h-7 rounded-[3px] text-[11px] text-ink-soft hover:bg-hover hover:text-ink"
                        },
                        "aria-pressed": controller.tab() == choice,
                        onclick: move |_| {
                            confirming_delete.set(None);
                            controller.select_tab(studio, choice, on_intent);
                        },
                        "{choice.label()}"
                    }
                }
            }

            match controller.tab() {
                DataWorkbenchTab::Episodes => rsx! {
                    div { class: "min-h-0 flex-1 overflow-y-auto",
                        if episodes.is_empty() {
                            div { class: "px-3 py-8 text-center",
                                strong { class: "block text-[12px] font-medium text-ink", "No episodes yet" }
                                p { class: "mt-1 text-[11px] text-ink-faint",
                                    "Record in Live mode or import a LeRobot dataset."
                                }
                            }
                        } else {
                            div { class: "sticky top-0 z-10 flex min-h-6 items-center border-b border-line bg-sunken px-2 text-[10px] text-ink-faint",
                                span { class: "flex-1", "Episodes" }
                                span { "Newest first" }
                            }
                            ul { class: "divide-y divide-line", "aria-label": "Dataset episodes",
                                for episode in episodes.iter().rev().cloned() {
                                    EpisodeRow {
                                        key: "{episode.id}",
                                        episode: episode.clone(),
                                        selected: selected.as_deref() == Some(episode.id.as_str()),
                                        playing: replaying.as_ref().is_some_and(|progress| progress.episode == episode.id),
                                        confirming_delete: confirming_delete.read().as_deref() == Some(episode.id.as_str()),
                                        on_select: move |chosen: EpisodeSummary| {
                                            confirming_delete.set(None);
                                            controller.present_episode(studio, chosen, on_intent);
                                        },
                                        on_delete: move |id: String| {
                                            if confirming_delete.read().as_deref() == Some(id.as_str()) {
                                                confirming_delete.set(None);
                                                delete_episode(studio, id);
                                            } else {
                                                confirming_delete.set(Some(id));
                                            }
                                        },
                                    }
                                }
                            }
                        }
                    }
                },
                DataWorkbenchTab::Query => rsx! {
                    div { class: "flex min-h-0 flex-1 flex-col",
                        if !query_available {
                            div { class: "mx-2 mt-2 rounded border border-line bg-sunken px-2 py-1.5 text-[11px] text-ink-soft", role: "status",
                                strong { class: "block font-medium text-ink", "Configure SpiralDB to run Query" }
                                span { "Open Settings and configure a Spiral catalog, project, and dataset." }
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
                                        let dataset = dataset.clone();
                                        move |_| {
                                            let statement = preset_sql(&dataset, preset);
                                            controller.set_sql(statement, on_intent);
                                        }
                                    },
                                    "{preset.label()}"
                                }
                            }
                        }
                        label { class: "flex min-h-0 flex-1 flex-col px-2 pt-2",
                            span { class: "label", "Read-only SQL" }
                            textarea {
                                class: "query-input min-h-32 flex-1 resize-none",
                                spellcheck: "false",
                                value: controller.sql(),
                                oninput: move |event| {
                                    controller.set_sql(event.value(), on_intent);
                                },
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
                                        controller.set_max_rows(rows, on_intent);
                                    },
                                    for option in QUERY_ROW_OPTIONS {
                                        option { value: "{option}", selected: controller.max_rows() == option, "{option}" }
                                    }
                                }
                            }
                            p { class: "min-w-0 flex-1 text-[10px] leading-snug text-ink-faint",
                                "SELECT, WITH, or EXPLAIN · ⌘/Ctrl Enter"
                            }
                            button {
                                r#type: "button",
                                class: "btn btn-primary",
                                disabled: !query_available || controller.sql().trim().is_empty() || matches!(controller.query().state, QueryState::Running),
                                onclick: move |_| run_query(),
                                if matches!(controller.query().state, QueryState::Running) { "Running…" } else { "Run query" }
                            }
                        }
                    }
                },
            }
        }
    }
}

impl DataWorkbenchTab {
    const fn label(self) -> &'static str {
        match self {
            Self::Episodes => "Episodes",
            Self::Query => "Query",
        }
    }
}

#[component]
fn EpisodeRow(
    episode: EpisodeSummary,
    selected: bool,
    playing: bool,
    confirming_delete: bool,
    on_select: EventHandler<EpisodeSummary>,
    on_delete: EventHandler<String>,
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

    rsx! {
        li {
            class: if selected { "bg-selected" } else { "bg-surface hover:bg-hover" },
            article { class: "flex min-h-12 items-center gap-1 px-1.5 py-1",
                button {
                    r#type: "button",
                    class: "min-w-0 flex-1 rounded px-1 py-0.5 text-left focus-visible:outline-offset-0",
                    "aria-pressed": selected,
                    onclick: {
                        let episode = episode.clone();
                        move |_| on_select.call(episode.clone())
                    },
                    span { class: "flex items-baseline gap-1.5",
                        strong { class: "truncate font-mono text-[11px] font-medium text-ink", "{short_id(&episode.id)}" }
                        span { class: if playing { "text-[10px] text-accent" } else { "text-[10px] text-ink-faint" }, "{state}" }
                        span { class: "ml-auto shrink-0 font-mono text-[10px] text-ink-soft", "{format_duration(episode.duration_seconds)}" }
                    }
                    span { class: "mt-0.5 flex min-w-0 items-center gap-1.5 text-[10px] text-ink-faint",
                        span { class: "truncate", "{episode.source}" }
                        span { class: "ml-auto shrink-0 font-mono", "{episode.frames}f · {episode.sample_hz:.0}Hz" }
                    }
                }
                button {
                    r#type: "button",
                    class: if confirming_delete { "btn btn-danger bg-danger-soft" } else { "btn btn-danger" },
                    title: if confirming_delete { "Delete permanently — click again to confirm" } else { "Delete this episode" },
                    onclick: {
                        let id = episode.id.clone();
                        move |_| on_delete.call(id.clone())
                    },
                    if confirming_delete { "Delete?" } else { "Delete" }
                }
            }
        }
    }
}

/// Default rendering for the query consequence of [`DataIntent`].
///
/// Kept independent from [`DataWorkbench`] so the Studio can mount it in the
/// right visualization region and replace it later with a richer grid without
/// changing query execution or the left-side controls.
#[component]
pub(crate) fn QueryResultView(model: QueryViewModel) -> Element {
    rsx! {
        section { class: "flex h-full min-h-0 flex-col bg-surface", "aria-label": "Query result",
            header { class: "flex min-h-8 shrink-0 items-center gap-3 border-b border-line px-2 text-[10px] text-ink-faint",
                strong { class: "text-[11px] font-medium text-ink", "Query result" }
                span { "Up to {model.max_rows} rows" }
                if let QueryState::Ready(result) = &model.state {
                    span { class: "ml-auto font-mono tabular-nums", "{result.rows.len()} rows · {result.elapsed_ms} ms" }
                    if result.truncated { span { class: "chip chip-caution", "Truncated" } }
                }
            }
            match &model.state {
                QueryState::Idle => rsx! {
                    div { class: "flex flex-1 items-center justify-center p-6 text-center",
                        div {
                            strong { class: "block text-[12px] font-medium text-ink", "Ready to query" }
                            p { class: "mt-1 max-w-[52ch] text-[11px] text-ink-faint",
                                "Run the read-only SQL on the left. Results stay in this visualization region."
                            }
                        }
                    }
                },
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
                                    th { key: "{column.name}",
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
}

fn preset_sql(dataset: &str, preset: QueryPreset) -> String {
    preset.sql(dataset)
}

pub(crate) fn default_query_sql(dataset: &str) -> String {
    preset_sql(dataset, QueryPreset::Episodes)
}

fn preferred_episode(episodes: &[EpisodeSummary]) -> Option<&EpisodeSummary> {
    episodes
        .iter()
        .rev()
        .find(|episode| episode.usable() && !episode.discarded)
        .or_else(|| episodes.last())
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

    #[test]
    fn presets_are_scoped_to_the_current_dataset() {
        assert_eq!(
            preset_sql("training", QueryPreset::JointSamples),
            "SELECT * FROM training.joint_samples ORDER BY episode_id DESC, stream_id, sample_index LIMIT 100"
        );
    }

    #[test]
    fn generated_episode_ids_read_as_take_numbers() {
        assert_eq!(short_id("episode_000024"), "#24");
        assert_eq!(short_id("imported-pass"), "imported-pass");
    }
}
