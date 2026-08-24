//! Top-level Episodes and Query panels for the Data route.
//!
//! Episode selection and query execution are independent state channels. A
//! query result stays mounted while an operator selects, replays, imports, or
//! exports episodes; neither tool has to impersonate navigation for the other.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_contracts::api::{
    EpisodeImportSummary, EpisodeSummary, HuggingFaceDatasetFormat,
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetRequest,
    QueryColumn, SpiralQueryResponse,
};
use spiral_robotics_contracts::spiral::{QueryPreset, Table};
use spiral_robotics_contracts::workflow::WorkflowRelationsResponse;
use spiral_robotics_ui::{
    Action, ActionLink, ActionPurpose, ActionVariant, CloseIcon, FileAction, InlineConfirmation,
    RefreshIcon, focus_after_render,
};

use crate::frontend::api::http as api;
use crate::frontend::app::state::studio;
use crate::frontend::format::format_clock;
use crate::frontend::screens::data::state::data_controller;
use crate::frontend::tasks::single_flight;

pub(crate) const DEFAULT_QUERY_ROWS: usize = 100;
const QUERY_ROW_OPTIONS: [usize; 5] = [25, 100, 250, 500, 1_000];
const IMAGE_EXTENSION_NAME: &str = "spql.image.Image";
const VARIABLE_TENSOR_EXTENSION_NAME: &str = "arrow.variable_shape_tensor";
const ENCODED_MEDIA_EXTENSION_NAME: &str = "spql.media.EncodedMedia";

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
pub(crate) fn EpisodesPanel(namespace: String) -> Element {
    let mut studio = studio();
    let importing = use_signal(|| false);
    let mut adding_data = use_signal(|| false);
    let mut confirming_delete = use_signal(|| None::<String>);
    let mut deleting_episode = use_signal(|| None::<String>);

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
    let dataset_valid = spiral_robotics_contracts::spiral::is_dataset_id(&dataset_id);
    let all_episodes = studio.episodes();
    let dataset_ids = all_episodes
        .iter()
        .map(|episode| episode.dataset_id.clone())
        .chain(studio.datasets().into_iter().map(|dataset| dataset.id))
        .chain(std::iter::once(dataset_id.clone()))
        .collect::<BTreeSet<_>>();
    let episodes = all_episodes
        .into_iter()
        .filter(|episode| episode.dataset_id == dataset_id)
        .collect::<Vec<_>>();
    let selected = studio.selected_episode.read().clone();
    let replaying = studio.replay();
    let catalog_loading = studio.episodes_loading();
    let catalog_loaded = studio.episodes_loaded();
    let catalog_error = studio.episode_catalog_error();
    let catalog_issues = studio.episode_catalog_issues();
    let import_dataset_id = dataset_id.clone();

    rsx! {
        section {
            class: "flex h-full min-h-0 flex-col bg-surface text-ink",
            "aria-label": "Episodes",

            header { class: "shrink-0 border-b border-line",
                div { class: "flex min-h-10 items-center gap-2 px-2",
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
                    if dataset_valid && !catalog_loaded {
                        p { class: "truncate text-[10px] text-ink-faint",
                            if catalog_loading { "Loading episodes…" } else { "Episodes not loaded" }
                            " · {namespace}"
                        }
                    } else if dataset_valid {
                        p { class: "truncate text-[10px] text-ink-faint",
                            "{episodes.len()} episodes · SpiralDB · {namespace}"
                        }
                    } else {
                        p { class: "truncate text-[10px] text-danger", role: "alert",
                            "Use 1–96 letters, numbers, '.', '_' or '-'."
                        }
                    }
                    }
                    Action {
                        purpose: ActionPurpose::Refresh,
                        variant: ActionVariant::Icon,
                        aria_label: if catalog_loading { "Refreshing episodes" } else { "Refresh episodes" },
                        title: "Refresh episodes and storage status",
                        disabled: catalog_loading,
                        aria_busy: catalog_loading,
                        onclick: move |_| {
                            studio.run_utility("", studio.refresh_episode_catalog());
                        },
                        RefreshIcon {}
                    }
                }
                // The scope note takes its own line rather than competing with two
                // buttons for the width of one. Sharing the row left it 30px of
                // the 105px it needs, so the caption on the transfer controls read
                // "Sele…".
                div { class: "flex min-h-9 flex-wrap items-center gap-x-2 gap-y-0.5 border-t border-line px-2 py-1",
                    span { class: "order-last w-full text-[10px] text-ink-faint", "Selected dataset only" }
                    Action {
                        purpose: ActionPurpose::Create,
                        variant: if adding_data() { ActionVariant::Solid } else { ActionVariant::Outline },
                        aria_pressed: adding_data(),
                        title: "Import a public Hugging Face dataset into native Spiral relations",
                        disabled: !dataset_valid,
                        onclick: move |_| adding_data.toggle(),
                        if adding_data() { "Adding data" } else { "Add data" }
                    }
                    FileAction {
                        purpose: ActionPurpose::Import,
                        variant: ActionVariant::Outline,
                        title: "Import episodes into the selected dataset; existing episode IDs are never replaced",
                        accept: ".spiral-episodes,application/vnd.spiral-robotics.episodes",
                        disabled: importing() || !dataset_valid,
                        aria_busy: importing(),
                        onchange: move |event: Event<FormData>| {
                            let Some(file) = event.files().first().cloned() else { return };
                            let target_dataset = import_dataset_id.clone();
                            studio.run_utility("", async move {
                                let Some(outcome) = single_flight(importing, async {
                                        let result = api::upload_file_json::<EpisodeImportSummary>(
                                            &format!("/api/episodes/import?dataset_id={target_dataset}"),
                                            &file,
                                            "application/vnd.spiral-robotics.episodes",
                                        )
                                        .await?;
                                        studio.refresh_episode_catalog().await?;
                                        Ok::<_, String>(result)
                                    })
                                    .await
                                else {
                                    return Ok(());
                                };
                                match outcome {
                                    Ok(result) => {
                                        if let Some(episode) = result.first_episode {
                                            studio.selected_episode.set(Some(episode));
                                        }
                                        studio.report_success(format!(
                                            "Imported {} episodes into {} ({} rows).{}",
                                            result.episodes,
                                            result.dataset_id,
                                            result.rows,
                                            import_warning_suffix(&result.warnings),
                                        ));
                                        Ok(())
                                    }
                                    Err(error) => Err(error),
                                }
                            });
                        },
                        if importing() { "Importing…" } else { "Import bundle" }
                    }
                    if dataset_valid && !episodes.is_empty() {
                        ActionLink {
                            purpose: ActionPurpose::Export,
                            variant: ActionVariant::Outline,
                            href: "/api/episodes/export?dataset_id={dataset_id}",
                            download: "{dataset_id}.spiral-episodes",
                            title: "Export every episode and its streams, samples, and video from the selected dataset",
                            "Export episodes"
                        }
                    } else {
                        Action {
                            purpose: ActionPurpose::Export,
                            variant: ActionVariant::Outline,
                            class: "text-ink-faint",
                            title: "The selected dataset has no episodes to export",
                            disabled: true,
                            "Export episodes"
                        }
                    }
                }
            }

            if adding_data() && dataset_valid {
                HuggingFaceImport {
                    key: "{dataset_id}",
                    dataset_id: dataset_id.clone(),
                    on_close: move |_| adding_data.set(false),
                }
            }

            div { class: "min-h-0 flex-1 overflow-y-auto",
                if let Some(error) = catalog_error {
                    div { class: "mx-2 mt-2 rounded bg-danger-soft px-2 py-1.5 text-[11px] text-danger", role: "alert",
                        strong { class: "block font-medium", "Episodes unavailable" }
                        span { "{error}" }
                    }
                }
                if !catalog_issues.is_empty() {
                    div { class: "mx-2 mt-2 rounded bg-danger-soft px-2 py-1.5 text-[11px] text-danger", role: "alert",
                        strong { class: "block font-medium", "Episode catalog issue" }
                        for issue in catalog_issues {
                            p { "{issue}" }
                        }
                    }
                }
                if !catalog_loaded {
                    div { class: "px-3 py-8 text-center", "aria-live": "polite",
                        strong { class: "block text-[12px] font-medium text-ink",
                            if catalog_loading { "Loading episodes…" } else { "Episodes not loaded" }
                        }
                        if !catalog_loading {
                            p { class: "mt-1 text-[11px] text-ink-faint", "Use Refresh to try again." }
                        }
                    }
                } else if episodes.is_empty() {
                    div { class: "px-3 py-8 text-center",
                        strong { class: "block text-[12px] font-medium text-ink", "No episodes yet" }
                        p { class: "mt-1 text-[11px] text-ink-faint",
                            "Record in Live, add a Hugging Face dataset, or import an episode bundle."
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
                                selected: selected.as_deref() == Some(episode.id.as_str()),
                                playing: replaying.as_ref().is_some_and(|progress| progress.episode == episode.id),
                                confirming_delete: confirming_delete.read().as_deref() == Some(episode.id.as_str()),
                                deleting: deleting_episode.read().as_deref() == Some(episode.id.as_str()),
                                delete_disabled: deleting_episode.read().is_some(),
                                on_select: move |chosen: EpisodeSummary| {
                                    if deleting_episode.peek().is_none() {
                                        confirming_delete.set(None);
                                    }
                                    if studio.selected_episode.peek().as_deref() != Some(chosen.id.as_str()) {
                                        studio.selected_episode.set(Some(chosen.id));
                                    }
                                },
                                on_delete: move |id: String| {
                                    if confirming_delete.read().as_deref() == Some(id.as_str()) {
                                        if deleting_episode.peek().is_none() {
                                            deleting_episode.set(Some(id.clone()));
                                            delete_episode(
                                                studio,
                                                id,
                                                deleting_episode,
                                                confirming_delete,
                                            );
                                        }
                                    } else {
                                        confirming_delete.set(Some(id));
                                    }
                                },
                                on_cancel_delete: move |_| confirming_delete.set(None),
                            }
                        }
                    }
                }
            }
        }
    }
}

fn import_warning_suffix(warnings: &[String]) -> String {
    if warnings.is_empty() {
        String::new()
    } else {
        format!(" Warning: {}", warnings.join(" "))
    }
}

/// Inspect-then-import flow for immutable Hugging Face snapshots. The panel is
/// inline because it belongs to the selected dataset and should not obscure
/// either the episode catalog or Query while an import is being prepared.
#[component]
fn HuggingFaceImport(dataset_id: String, on_close: EventHandler<()>) -> Element {
    let studio = studio();
    let controller = data_controller();
    let mut format = use_signal(|| HuggingFaceDatasetFormat::LeRobot);
    let mut repository = use_signal(String::new);
    let mut revision = use_signal(|| "main".to_owned());
    let inspecting = use_signal(|| false);
    let importing = use_signal(|| false);
    let mut inspection = use_signal(|| None::<HuggingFaceDatasetInspection>);
    let mut imported = use_signal(|| None::<HuggingFaceDatasetImportSummary>);
    let mut error = use_signal(|| None::<String>);

    let request = HuggingFaceDatasetRequest {
        dataset_id: dataset_id.clone(),
        repository: repository().trim().to_owned(),
        revision: revision().trim().to_owned(),
        format: format(),
    };
    let can_inspect = !request.repository.is_empty()
        && !request.revision.is_empty()
        && !inspecting()
        && !importing();
    let panel_pending = inspecting() || importing();

    rsx! {
        section { class: "data-import-workspace", "aria-labelledby": "data-import-title",
            header { class: "data-import-heading",
                div {
                    h3 { id: "data-import-title", "Add data from Hugging Face" }
                    p { "A public snapshot becomes native Spiral relations under the new Dataset ID above." }
                }
                Action {
                    purpose: ActionPurpose::Close,
                    variant: ActionVariant::Icon,
                    aria_label: "Close dataset import",
                    title: "Close dataset import",
                    disabled: panel_pending,
                    onclick: move |_| on_close.call(()),
                    CloseIcon {}
                }
            }

            div { class: "data-import-form",
                div { class: "data-import-format", role: "group", "aria-label": "Dataset format",
                    for option in [HuggingFaceDatasetFormat::LeRobot, HuggingFaceDatasetFormat::Mcap] {
                        Action {
                            key: "{option.label()}",
                            purpose: ActionPurpose::Select,
                            variant: ActionVariant::Segment,
                            aria_pressed: format() == option,
                            disabled: panel_pending,
                            onclick: move |_| {
                                format.set(option);
                                inspection.set(None);
                                imported.set(None);
                                error.set(None);
                            },
                            "{option.label()}"
                        }
                    }
                }
                label { class: "data-import-repository",
                    span { class: "label", "Dataset repository" }
                    input {
                        class: "field",
                        value: repository(),
                        placeholder: "lerobot/pusht",
                        spellcheck: "false",
                        autocomplete: "off",
                        disabled: panel_pending,
                        oninput: move |event| {
                            repository.set(event.value());
                            inspection.set(None);
                            imported.set(None);
                            error.set(None);
                        }
                    }
                }
                label { class: "data-import-revision",
                    span { class: "label", "Revision" }
                    input {
                        class: "field font-mono",
                        value: revision(),
                        placeholder: "main",
                        spellcheck: "false",
                        autocomplete: "off",
                        disabled: panel_pending,
                        oninput: move |event| {
                            revision.set(event.value());
                            inspection.set(None);
                            imported.set(None);
                            error.set(None);
                        }
                    }
                }
                Action {
                    purpose: ActionPurpose::Apply,
                    variant: ActionVariant::Outline,
                    disabled: !can_inspect,
                    aria_busy: inspecting(),
                    onclick: {
                        let request = request.clone();
                        move |_| {
                            let request = request.clone();
                            error.set(None);
                            inspection.set(None);
                            imported.set(None);
                            studio.run_utility("", async move {
                                let Some(outcome) = single_flight(
                                    inspecting,
                                    api::send_json::<_, HuggingFaceDatasetInspection>(
                                        Method::POST,
                                        "/api/datasets/huggingface/inspect",
                                        Some(&request),
                                    ),
                                )
                                .await else {
                                    return Ok(());
                                };
                                match outcome {
                                    Ok(result) => {
                                        inspection.set(Some(result));
                                        Ok(())
                                    }
                                    Err(problem) => {
                                        error.set(Some(problem.clone()));
                                        Err(format!("Could not inspect the Hugging Face dataset: {problem}"))
                                    }
                                }
                            });
                        }
                    },
                    if inspecting() { "Inspecting…" } else { "Inspect source" }
                }
            }

            if let Some(problem) = error() {
                div { class: "data-import-error", role: "alert",
                    strong { "Could not continue" }
                    span { "{problem}" }
                }
            }
            if let Some(summary) = imported() {
                div { class: "data-import-complete", role: "status",
                    div {
                        strong { "Dataset ready in Query" }
                        p {
                            "Imported {summary.files} files into {summary.relations.len()} Spiral relations at "
                            code { "{short_revision(&summary.resolved_revision)}" }
                            "."
                        }
                    }
                    Action {
                        purpose: ActionPurpose::Close,
                        variant: ActionVariant::Outline,
                        onclick: move |_| on_close.call(()),
                        "Done"
                    }
                }
            } else if let Some(source) = inspection() {
                div { class: "data-import-inspection",
                    div { class: "data-import-inspection-scroll",
                        dl { class: "data-import-facts",
                            div { dt { "Snapshot" } dd { code { "{short_revision(&source.resolved_revision)}" } } }
                            div { dt { "Files" } dd { "{source.files.len()}" } }
                            div { dt { "Size" } dd { "{format_bytes(source.total_bytes)}" } }
                            div { dt { "Target" } dd { code { "{source.dataset_id}" } } }
                        }
                        div { class: "data-import-plan",
                            strong { "Spiral relations" }
                            ul {
                                for relation in source.planned_relations.iter() {
                                    li { code { "{relation}" } }
                                }
                            }
                        }
                        if !source.warnings.is_empty() {
                            div { class: "data-import-warnings", role: "note",
                                strong { "Before importing" }
                                ul {
                                    for warning in source.warnings.iter() { li { "{warning}" } }
                                }
                            }
                        }
                    }
                    div { class: "data-import-confirm",
                        p { "The resolved commit is immutable. Existing datasets and relations are never replaced." }
                        Action {
                            purpose: ActionPurpose::Import,
                            variant: ActionVariant::Solid,
                            disabled: importing(),
                            aria_busy: importing(),
                            onclick: {
                                let source = source.clone();
                                move |_| {
                                    let import_request = HuggingFaceDatasetRequest {
                                        dataset_id: source.dataset_id.clone(),
                                        repository: source.repository.clone(),
                                        revision: source.resolved_revision.clone(),
                                        format: source.format,
                                    };
                                    error.set(None);
                                    studio.run_utility("", async move {
                                        let Some(outcome) = single_flight(
                                            importing,
                                            api::send_json::<_, HuggingFaceDatasetImportSummary>(
                                                Method::POST,
                                                "/api/datasets/huggingface/import",
                                                Some(&import_request),
                                            ),
                                        )
                                        .await else {
                                            return Ok(());
                                        };
                                        match outcome {
                                            Ok(summary) => {
                                                controller.refresh_relations();
                                                controller.set_sql(summary.suggested_sql.clone());
                                                studio.report_success_from(
                                                    crate::frontend::app::notifications::MessageSource::Data,
                                                    format!(
                                                        "Imported {} {} files from {} at {} into {} Spiral relations.",
                                                        summary.files,
                                                        summary.format.label(),
                                                        summary.repository,
                                                        short_revision(&summary.resolved_revision),
                                                        summary.relations.len(),
                                                    ),
                                                );
                                                imported.set(Some(summary.clone()));
                                                studio.refresh_episode_catalog().await.map_err(|problem| {
                                                    format!(
                                                        "Dataset imported successfully, but Studio could not refresh the catalog: {problem}"
                                                    )
                                                })?;
                                                Ok(())
                                            }
                                            Err(problem) => {
                                                error.set(Some(problem.clone()));
                                                Err(format!("Could not import the Hugging Face dataset: {problem}"))
                                            }
                                        }
                                    });
                                }
                            },
                            if importing() { "Importing with Spiral…" } else { "Import dataset" }
                        }
                    }
                }
            } else {
                p { class: "data-import-guidance",
                    "Use a repository ID or its huggingface.co dataset URL. Studio discovers files; Spiral performs the durable remote reads and format parsing."
                }
            }
        }
    }
}

fn short_revision(revision: &str) -> &str {
    revision.get(..10).unwrap_or(revision)
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

/// Read-only SQL controls. Results render in their own sibling panel and stay
/// available while this panel is moved, tabbed, or obscured.
#[component]
pub(crate) fn QueryPanel(namespace: String, dataset_id: String, query_available: bool) -> Element {
    let studio = studio();
    let controller = data_controller();
    let mut relation_catalog = use_resource(move || {
        let _generation = controller.relation_generation();
        async move {
            if query_available {
                api::get::<WorkflowRelationsResponse>("/api/spiral/relations").await
            } else {
                Ok(WorkflowRelationsResponse::default())
            }
        }
    });
    let relation_result = relation_catalog.read_unchecked().clone();
    let relation_error = relation_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .cloned();
    let relation_response = relation_result
        .as_ref()
        .and_then(|result| result.as_ref().ok());
    let schema_relations = schema_relation_views(
        &namespace,
        &dataset_id,
        query_available.then_some(relation_response).flatten(),
    );
    let dataset_valid = spiral_robotics_contracts::spiral::is_dataset_id(&dataset_id);
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
            details { class: "query-schema-browser",
                summary {
                    span { "Database relations" }
                    small {
                        if relation_result.is_none() && query_available {
                            "Discovering…"
                        } else {
                            "{schema_relations.len()} relations · {namespace}"
                        }
                    }
                }
                div { class: "query-schema-list",
                    if let Some(problem) = relation_error {
                        div { class: "query-schema-error", role: "alert",
                            span { "Catalog discovery failed" }
                            Action {
                                purpose: ActionPurpose::Retry,
                                variant: ActionVariant::Link,
                                onclick: move |_| relation_catalog.restart(),
                                "Retry"
                            }
                            small { "{problem}" }
                        }
                    }
                    for relation in schema_relations.iter().cloned() {
                        Action {
                            key: "{relation.name}",
                            purpose: ActionPurpose::Apply,
                            variant: ActionVariant::Row,
                            class: "query-schema-row",
                            title: "Query {relation.name}",
                            onclick: move |_| controller.set_sql(relation.sql.clone()),
                            span {
                                strong { "{relation.label}" }
                                small { "{relation.description}" }
                            }
                            code { "{relation.columns}" }
                        }
                    }
                }
            }
            label { class: "flex min-h-0 flex-1 flex-col overflow-hidden px-2 pt-2",
                span { class: "label", "Read-only SQL" }
                // `min-h-0`, not `min-h-32`. A 112px floor on a box whose parent
                // is free to shrink to 37px does not make the box taller — it
                // makes it hang 75px out of the bottom of its own panel, with
                // the Max rows select and Run query drawn on top of the SQL.
                textarea {
                    class: "query-input min-h-0 flex-1 resize-none",
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
                    Action {
                        purpose: ActionPurpose::Cancel,
                        variant: ActionVariant::Outline,
                        onclick: move |_| controller.cancel_query(),
                        "Cancel query"
                    }
                } else {
                    Action {
                        purpose: ActionPurpose::Start,
                        variant: ActionVariant::Solid,
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
    selected: bool,
    playing: bool,
    confirming_delete: bool,
    deleting: bool,
    delete_disabled: bool,
    on_select: EventHandler<EpisodeSummary>,
    on_delete: EventHandler<String>,
    on_cancel_delete: EventHandler<()>,
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
    let delete_title = "Delete this episode from SpiralDB";

    rsx! {
        li {
            class: "bg-surface",
            article { class: "data-episode-row ui-hover-action-host flex min-h-12 flex-wrap items-center gap-1 px-1.5 py-1",
                Action {
                    purpose: ActionPurpose::Select,
                    variant: ActionVariant::Row,
                    class: "data-episode-select ui-action--bare min-w-0 flex-1 rounded px-1 py-0.5 text-left focus-visible:outline-offset-0",
                    aria_pressed: selected,
                    onclick: {
                        let episode = episode.clone();
                        move |_| on_select.call(episode.clone())
                    },
                    span { class: "flex items-baseline gap-1.5",
                        strong { class: "truncate font-mono text-[11px] font-medium text-ink", "{short_id(&episode.id)}" }
                        span { class: if playing { "text-[10px] text-accent" } else { "text-[10px] text-ink-faint" }, "{state}" }
                        // No storage badge. SpiralDB is the only persistent path
                        // this product has, so the chip was green on every row of
                        // every catalog and could never have read otherwise; the
                        // panel header already names the store once. A tag that
                        // cannot report an exception is not reporting anything.
                        span { class: "ml-auto shrink-0 font-mono text-[10px] text-ink-soft", "{format_clock(episode.duration_seconds)}" }
                    }
                    span { class: "mt-0.5 flex min-w-0 items-center gap-1.5 text-[10px] text-ink-faint",
                        span { class: "truncate", title: "{episode.source}", "{episode.source}" }
                        span { class: "ml-auto shrink-0 font-mono", "{episode.frames}f · {episode.sample_hz:.0}Hz" }
                    }
                }
                if confirming_delete {
                    InlineConfirmation {
                        id: "delete-episode-confirmation-{episode.id}",
                        class: "data-delete-confirmation",
                        message: format!(
                            "Delete episode {}? Its streams and video will be permanently removed from SpiralDB.",
                            short_id(&episode.id),
                        ),
                        confirm_label: if deleting { "Deleting…" } else { "Delete episode" },
                        cancel_label: "Keep episode",
                        purpose: ActionPurpose::Delete,
                        pending: deleting,
                        return_focus_id: "delete-episode-{episode.id}",
                        on_confirm: {
                            let id = episode.id.clone();
                            move |_| on_delete.call(id.clone())
                        },
                        on_cancel: move |_| on_cancel_delete.call(()),
                    }
                } else {
                    div { class: "data-episode-actions",
                        Action {
                            id: "delete-episode-{episode.id}",
                            purpose: ActionPurpose::Delete,
                            variant: ActionVariant::Link,
                            class: "data-delete-action ui-hover-action",
                            disabled: delete_disabled,
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
                        div {
                            class: if active_id == Some(tab.id) { "query-result-tab-item query-result-tab-item-active ui-hover-action-host" } else { "query-result-tab-item ui-hover-action-host" },
                            Action {
                                id: "query-tab-{tab.id}",
                                purpose: ActionPurpose::Select,
                                variant: ActionVariant::Tab,
                                class: if active_id == Some(tab.id) { "query-result-tab query-result-tab-active" } else { "query-result-tab" },
                                role: "tab",
                                tabindex: if active_id == Some(tab.id) { "0" } else { "-1" },
                                aria_selected: active_id == Some(tab.id),
                                aria_controls: "query-panel-{tab.id}",
                                onclick: {
                                    let id = tab.id;
                                    move |_| controller.select_query(id)
                                },
                                onkeydown: {
                                    let ids = tab_ids.clone();
                                    let id = tab.id;
                                    move |event: KeyboardEvent| {
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
                                span { class: "query-result-tab-label", "{tab.title}" }
                            }
                            Action {
                                purpose: ActionPurpose::Close,
                                variant: ActionVariant::Icon,
                                class: "query-result-tab-close ui-hover-action",
                                aria_label: "Close {tab.title}",
                                title: "Close {tab.title}",
                                onclick: {
                                    let ids = tab_ids.clone();
                                    let id = tab.id;
                                    move |_| {
                                        let target = query_tab_neighbor_after_close(&ids, id);
                                        controller.close_query(id);
                                        if let Some(target) = target {
                                            controller.select_query(target);
                                            focus_query_tab(target);
                                        }
                                    }
                                },
                                CloseIcon {}
                            }
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
                        if let QueryState::Ready(result) = &tab.model.state {
                            span { class: "query-result-count", role: "status",
                                strong { "{result.rows.len()} rows" }
                                span { "{result.elapsed_ms} ms" }
                            }
                            if result.truncated { span { class: "chip chip-caution", "Truncated" } }
                        }
                        span { class: "query-result-limit", "Limit · {tab.model.max_rows} rows" }
                        details {
                            summary { "SQL" }
                            code { "{tab.model.sql}" }
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

fn focus_query_tab(id: u64) {
    focus_after_render(format!("query-tab-{id}"));
}

pub(crate) fn render_query_result(model: &QueryViewModel) -> Element {
    rsx! { QueryResultContent { model: model.clone() } }
}

#[component]
fn QueryResultContent(model: QueryViewModel) -> Element {
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
                QueryState::Ready(result) => {
                    let has_video_previews = query_needs_video_preview_column(result);
                    let result_row_count = result.rows.len();
                    rsx! {
                        div { class: "query-result-ready",
                            div { class: "query-result-table-scroll",
                            table { class: "query-table", "aria-label": "Query results",
                                thead { tr {
                                    if has_video_previews {
                                        th { class: "query-media-heading", scope: "col",
                                            span { class: "query-column-name", "Preview" }
                                            span { class: "query-column-type", "Video" }
                                        }
                                    }
                                    for column in &result.columns {
                                        th { key: "{column.name}", scope: "col",
                                            title: query_column_title(column),
                                            span { class: "query-column-name", "{column.name}" }
                                            span { class: "query-column-type", "{query_column_kind(column)}" }
                                        }
                                    }
                                } }
                                tbody {
                                    for (index, row) in result.rows.iter().enumerate() {
                                        tr { key: "{index}",
                                            if has_video_previews {
                                                {render_query_video_cell(row, index)}
                                            }
                                            for column in &result.columns {
                                                {render_query_cell(column, row, index, result_row_count)}
                                            }
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct SchemaRelationView {
    name: String,
    label: String,
    description: String,
    columns: String,
    sql: String,
}

fn schema_relation_views(
    namespace: &str,
    dataset_id: &str,
    response: Option<&WorkflowRelationsResponse>,
) -> Vec<SchemaRelationView> {
    let Some(response) = response else {
        return Table::ALL
            .into_iter()
            .map(|table| SchemaRelationView {
                name: spiral_robotics_contracts::spiral::relation(namespace, table),
                label: table.name().to_owned(),
                description: table_description(table).to_owned(),
                columns: table.columns().join(", "),
                sql: preset_sql(namespace, dataset_id, table_preset(table)),
            })
            .collect();
    };
    response
        .relations
        .iter()
        .map(|relation| {
            let managed = Table::ALL.into_iter().find(|table| {
                relation.name == spiral_robotics_contracts::spiral::relation(namespace, *table)
            });
            let label = managed
                .map(|table| table.name().to_owned())
                .unwrap_or_else(|| {
                    relation
                        .name
                        .strip_prefix(&format!("{namespace}."))
                        .unwrap_or(&relation.name)
                        .to_owned()
                });
            SchemaRelationView {
                name: relation.name.clone(),
                label,
                description: managed
                    .map(table_description)
                    .unwrap_or(&relation.relation_type)
                    .to_owned(),
                columns: relation
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                sql: managed
                    .map(|table| preset_sql(namespace, dataset_id, table_preset(table)))
                    .unwrap_or_else(|| format!("SELECT * FROM {} LIMIT 100", relation.name)),
            }
        })
        .collect()
}

fn preset_sql(namespace: &str, dataset_id: &str, preset: QueryPreset) -> String {
    let dataset_id = dataset_id.replace('\'', "''");
    let episodes = spiral_robotics_contracts::spiral::relation(namespace, Table::Episodes);
    let relation = spiral_robotics_contracts::spiral::relation(namespace, preset.table());
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
            "SELECT v.episode_id, v.stream_id, v.segment_index, v.camera_id, v.byte_size, v.width, v.height, v.fps, v.frames FROM {relation} AS v JOIN {episodes} AS e ON e.episode_id = v.episode_id WHERE e.dataset_id = '{dataset_id}' ORDER BY v.episode_id DESC, v.stream_id, v.segment_index LIMIT 100"
        ),
        QueryPreset::TrainingCheckpoints
        | QueryPreset::Robots
        | QueryPreset::StudioSettings
        | QueryPreset::TrainingRuns
        | QueryPreset::RolloutRuns
        | QueryPreset::WorkflowRuns => preset.sql(namespace),
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
        Table::Robots => QueryPreset::Robots,
        Table::StudioSettings => QueryPreset::StudioSettings,
        Table::TrainingRuns => QueryPreset::TrainingRuns,
        Table::RolloutRuns => QueryPreset::RolloutRuns,
        Table::WorkflowRuns => QueryPreset::WorkflowRuns,
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
        Table::Robots => "Robot configuration and hardware assignments",
        Table::StudioSettings => "Persistent Studio preferences",
        Table::TrainingRuns => "Training lifecycle and metric history",
        Table::RolloutRuns => "Policy rollout lifecycle and outcomes",
        Table::WorkflowRuns => "Saved Workflow graph and latest run snapshot",
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

fn delete_episode(
    studio: crate::frontend::app::state::Studio,
    id: String,
    mut deleting_episode: Signal<Option<String>>,
    mut confirming_delete: Signal<Option<String>>,
) {
    let mut selected = studio.selected_episode;
    let path = format!("/api/episodes/{id}");
    let deleted_id = id.clone();
    studio.run_utility(format!("Deleted {id}."), async move {
        let result = api::send::<()>(Method::DELETE, &path, None).await;
        deleting_episode.set(None);
        if result.is_ok() {
            confirming_delete.set(None);
            studio.remove_episode(&deleted_id);
            // A slower deletion must not clear a newer choice made while the
            // request was in flight.
            if selected.peek().as_deref() == Some(deleted_id.as_str()) {
                selected.set(None);
            }
        }
        result
    });
}

fn query_column_title(column: &QueryColumn) -> String {
    let mut title = column.data_type.clone();
    if let Some(semantic_type) = &column.semantic_type {
        title.push_str(" · ");
        title.push_str(semantic_type);
    }
    if column.nullable {
        title.push_str(" · nullable");
    }
    title
}

fn humanize_query_column(name: &str) -> String {
    let words = name
        .trim_end_matches("_preview")
        .trim_end_matches("_image")
        .split('_')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let mut label = words.join(" ");
    if let Some(first) = label.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    if label.is_empty() {
        "Image".to_owned()
    } else {
        label
    }
}

fn query_image_alt(
    column: &QueryColumn,
    row: &serde_json::Map<String, serde_json::Value>,
    row_index: usize,
    row_count: usize,
) -> String {
    let prefix = column
        .name
        .strip_suffix("_preview")
        .or_else(|| column.name.strip_suffix("_image"));
    let mut context = Vec::new();
    for (label, field) in [
        ("episode", "episode_id"),
        ("stream", "stream_id"),
        ("frame", "frame_index"),
    ] {
        let specific = prefix.map(|prefix| format!("{prefix}_{field}"));
        let value = specific
            .as_deref()
            .and_then(|field| row.get(field))
            .or_else(|| row.get(field))
            .map(query_cell_value)
            .filter(|value| value != "null");
        if let Some(value) = value {
            context.push(format!("{label} {value}"));
        }
    }
    let context = if context.is_empty() {
        String::new()
    } else {
        format!(" for {}", context.join(", "))
    };
    format!(
        "{}{}; result {} of {}",
        humanize_query_column(&column.name),
        context,
        row_index + 1,
        row_count
    )
}

fn query_cell_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}

fn render_query_cell(
    column: &QueryColumn,
    row: &serde_json::Map<String, serde_json::Value>,
    row_index: usize,
    row_count: usize,
) -> Element {
    let value = row.get(&column.name);
    if query_is_image_column(column) {
        return rsx! {
            td { key: "{column.name}", class: "query-media-cell",
                if let Some(src) = query_image_src(column, value) {
                    img {
                        class: "query-image-preview",
                        src,
                        alt: query_image_alt(column, row, row_index, row_count),
                        loading: "lazy",
                        decoding: "async",
                        draggable: "false",
                    }
                } else {
                    span { class: "query-image-unavailable", "No image" }
                }
            }
        };
    }
    if column.semantic_type.as_deref() == Some(ENCODED_MEDIA_EXTENSION_NAME) {
        return render_query_video_cell(row, row_index);
    }
    let rendered = query_cell(value);
    let display = if value.is_none_or(serde_json::Value::is_null) {
        "—"
    } else {
        rendered.as_str()
    };
    rsx! {
        td {
            key: "{column.name}",
            class: query_value_class(value),
            title: "{rendered}",
            "{display}"
        }
    }
}

fn query_is_image_column(column: &QueryColumn) -> bool {
    matches!(
        column.semantic_type.as_deref(),
        Some(IMAGE_EXTENSION_NAME | VARIABLE_TENSOR_EXTENSION_NAME)
    )
}

fn query_image_src<'a>(
    column: &QueryColumn,
    value: Option<&'a serde_json::Value>,
) -> Option<&'a str> {
    matches!(
        column.semantic_type.as_deref(),
        Some(IMAGE_EXTENSION_NAME | VARIABLE_TENSOR_EXTENSION_NAME)
    )
    .then(|| value?.as_str())?
    .filter(|value| value.starts_with("data:image/"))
}

fn query_needs_video_preview_column(result: &SpiralQueryResponse) -> bool {
    if result
        .columns
        .iter()
        .any(|column| column.semantic_type.as_deref() == Some(ENCODED_MEDIA_EXTENSION_NAME))
    {
        return false;
    }
    let has = |name: &str| result.columns.iter().any(|column| column.name == name);
    has("episode_id")
        && has("stream_id")
        && has("segment_index")
        && (has("camera_id")
            || has("byte_size")
            || result.columns.iter().any(|column| {
                column.semantic_type.as_deref() == Some(ENCODED_MEDIA_EXTENSION_NAME)
            }))
}

fn query_column_kind(column: &QueryColumn) -> &str {
    match column.semantic_type.as_deref() {
        Some(IMAGE_EXTENSION_NAME | VARIABLE_TENSOR_EXTENSION_NAME) => "Image",
        Some(ENCODED_MEDIA_EXTENSION_NAME) => "Video",
        _ => &column.data_type,
    }
}

fn query_value_class(value: Option<&serde_json::Value>) -> &'static str {
    match value {
        None | Some(serde_json::Value::Null) => "query-value-null",
        Some(serde_json::Value::Number(_)) => "query-value-number",
        Some(serde_json::Value::Bool(_)) => "query-value-boolean",
        _ => "",
    }
}

fn render_query_video_cell(
    row: &serde_json::Map<String, serde_json::Value>,
    row_index: usize,
) -> Element {
    let Some((url, label)) = query_video_preview(row) else {
        return rsx! {
            td { class: "query-media-cell query-media-unavailable", title: "This row does not identify one exact video segment", "—" }
        };
    };
    rsx! {
        td { class: "query-media-cell",
            video {
                class: "query-video-preview",
                src: url,
                preload: "none",
                controls: true,
                playsinline: true,
                "aria-label": "Query video {label}, row {row_index + 1}",
                "Your browser cannot preview this MP4 segment."
            }
        }
    }
}

fn query_video_preview(
    row: &serde_json::Map<String, serde_json::Value>,
) -> Option<(String, String)> {
    let episode = row.get("episode_id")?.as_str()?;
    let stream = row.get("stream_id")?.as_str()?;
    let segment = row.get("segment_index")?.as_u64()?.try_into().ok()?;
    let label = row
        .get("camera_id")
        .and_then(serde_json::Value::as_str)
        .filter(|label| !label.is_empty())
        .unwrap_or(stream)
        .to_owned();
    Some((api::episode_video_url(episode, stream, segment), label))
}

pub(crate) fn short_id(id: &str) -> String {
    id.strip_prefix("episode_")
        .and_then(|number| number.parse::<u64>().ok())
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| id.to_owned())
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

    fn query_column(name: &str, semantic_type: Option<&str>) -> QueryColumn {
        QueryColumn {
            name: name.to_owned(),
            data_type: "Utf8".to_owned(),
            nullable: false,
            semantic_type: semantic_type.map(str::to_owned),
        }
    }

    fn episode(id: &str, usable: bool, discarded: bool) -> EpisodeSummary {
        EpisodeSummary {
            dataset_id: "default".to_owned(),
            id: id.to_owned(),
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
        assert_eq!(
            preset_sql("training", "pick-'place", QueryPreset::Episodes),
            "SELECT * FROM training.episodes WHERE dataset_id = 'pick-''place' ORDER BY started_ms DESC LIMIT 100"
        );
        assert_eq!(
            preset_sql("training", "pick-place", QueryPreset::VideoAssets),
            "SELECT v.episode_id, v.stream_id, v.segment_index, v.camera_id, v.byte_size, v.width, v.height, v.fps, v.frames FROM training.video_assets AS v JOIN training.episodes AS e ON e.episode_id = v.episode_id WHERE e.dataset_id = 'pick-place' ORDER BY v.episode_id DESC, v.stream_id, v.segment_index LIMIT 100"
        );
    }

    #[test]
    fn image_previews_require_both_semantic_type_and_an_encoded_image() {
        let value = serde_json::json!("data:image/jpeg;base64,preview");
        let image = query_column("frame", Some(IMAGE_EXTENSION_NAME));
        let plain = query_column("frame", None);
        assert_eq!(
            query_image_src(&image, Some(&value)),
            Some("data:image/jpeg;base64,preview")
        );
        assert_eq!(query_image_src(&plain, Some(&value)), None);
        assert_eq!(
            query_image_src(&image, Some(&serde_json::json!("not-an-image"))),
            None
        );
    }

    #[test]
    fn image_semantics_render_as_inline_image_columns() {
        let image = query_column("top_preview", Some(IMAGE_EXTENSION_NAME));
        let tensor = query_column("wrist_preview", Some(VARIABLE_TENSOR_EXTENSION_NAME));
        let plain = query_column("episode_id", None);

        assert!(query_is_image_column(&image));
        assert!(query_is_image_column(&tensor));
        assert!(!query_is_image_column(&plain));
        assert_eq!(query_column_kind(&image), "Image");
        assert_eq!(query_column_kind(&tensor), "Image");
        assert_eq!(query_column_kind(&plain), "Utf8");
    }

    #[test]
    fn inline_image_alt_uses_camera_specific_identity_and_row_position() {
        let column = query_column("wrist_preview", Some(IMAGE_EXTENSION_NAME));
        let row = serde_json::Map::from_iter([
            ("episode_id".to_owned(), serde_json::json!("take-12")),
            (
                "wrist_stream_id".to_owned(),
                serde_json::json!("follower/wrist"),
            ),
            ("wrist_frame_index".to_owned(), serde_json::json!(42)),
        ]);
        assert_eq!(
            query_image_alt(&column, &row, 2, 18),
            "Wrist for episode take-12, stream follower/wrist, frame 42; result 3 of 18"
        );
    }

    #[test]
    fn nulls_and_numbers_have_stable_table_treatment() {
        assert_eq!(query_value_class(None), "query-value-null");
        assert_eq!(
            query_value_class(Some(&serde_json::Value::Null)),
            "query-value-null"
        );
        assert_eq!(
            query_value_class(Some(&serde_json::json!(0.82))),
            "query-value-number"
        );
        assert_eq!(
            query_value_class(Some(&serde_json::json!(true))),
            "query-value-boolean"
        );
    }

    #[test]
    fn video_rows_map_to_the_exact_range_streaming_endpoint() {
        let columns = vec![
            query_column("episode_id", None),
            query_column("stream_id", None),
            query_column("segment_index", None),
            query_column("camera_id", None),
        ];
        let row = serde_json::Map::from_iter([
            ("episode_id".to_owned(), serde_json::json!("take 1")),
            ("stream_id".to_owned(), serde_json::json!("follower/camera")),
            ("segment_index".to_owned(), serde_json::json!(2)),
            ("camera_id".to_owned(), serde_json::json!("wrist")),
        ]);
        let result = SpiralQueryResponse {
            columns,
            rows: vec![row.clone()],
            truncated: false,
            elapsed_ms: 1,
        };
        assert!(query_needs_video_preview_column(&result));
        assert_eq!(
            query_video_preview(&row),
            Some((
                "/api/episodes/take%201/videos/follower%2Fcamera?segment=2".to_owned(),
                "wrist".to_owned(),
            ))
        );

        let encoded = SpiralQueryResponse {
            columns: vec![
                query_column("episode_id", None),
                query_column("stream_id", None),
                query_column("segment_index", None),
                query_column("media", Some(ENCODED_MEDIA_EXTENSION_NAME)),
            ],
            ..result
        };
        assert!(!query_needs_video_preview_column(&encoded));
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
}
