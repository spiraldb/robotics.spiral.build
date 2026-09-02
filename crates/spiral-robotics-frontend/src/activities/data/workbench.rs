//! Top-level Episodes and Query editor panels for the Data route.
//!
//! Episode selection and query execution are independent state channels. A
//! result owned by `query_result` stays mounted while an operator selects,
//! replays, imports, or exports episodes; neither tool has to impersonate
//! navigation for the other.

use std::collections::BTreeSet;

use crate::components::interaction::{
    Action, ActionPurpose, ActionVariant, CloseIcon, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, FileAction, FileInput, InlineConfirmation, RefreshIcon,
};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetFormat, HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection,
    HuggingFaceDatasetSource,
};
use spiral_robotics_common::recording::EpisodeListItem;
use spiral_robotics_common::relations::RelationCatalog;
use spiral_robotics_common::status::BackendReadinessState;

use crate::activities::data::catalog::ManagedTable as Table;
use crate::activities::data::query_result::DEFAULT_QUERY_ROWS;
use crate::client::server_fns::{
    EpisodeImportUpload, delete_episode, import_episodes, import_hugging_face_dataset,
    inspect_hugging_face_dataset,
};
use crate::format::ClockPresentation;
use crate::shell::state::studio;
use crate::tasks::SingleFlight;

const QUERY_ROW_OPTIONS: [usize; 5] = [25, 100, 250, 500, 1_000];

/// Dataset navigation, transfer, and per-episode actions.
#[component]
pub(crate) fn EpisodesPanel(namespace: String) -> Element {
    let studio = studio();
    let data = studio.data_controller();
    let importing = use_signal(|| false);
    let mut adding_data = use_signal(|| false);
    let mut confirming_delete = use_signal(|| None::<String>);
    let mut deleting_episode = use_signal(|| None::<String>);

    // Load the tiny dataset index before choosing which episode subset to read.
    use_effect(move || {
        let storage_available = studio.snapshot().read().readiness().storage_available();
        if storage_available
            && !data.datasets_loaded()
            && !data.datasets_loading()
            && data.dataset_catalog_error().is_none()
        {
            studio.run_utility("", data.load_datasets());
        }
    });

    // Preserve a known selection; otherwise initialize deterministically from
    // the default dataset or the first stored dataset.
    use_effect(move || {
        let current = studio.selected_dataset().read().trim().to_owned();
        if let Some(dataset_id) = data.initialize_dataset_selection(&current) {
            studio.selected_dataset().set(dataset_id);
        }
    });

    // Episode rows are requested only for the active dataset. The request is
    // restarted when the shared selection changes.
    use_effect(move || {
        let dataset_id = studio.selected_dataset().read().trim().to_owned();
        let storage_available = studio.snapshot().read().readiness().storage_available();
        if storage_available
            && data.datasets_loaded()
            && spiral_robotics_common::identity::DatasetId::is_canonical(&dataset_id)
            && !data.episodes_loaded(&dataset_id)
            && !data.episodes_loading(&dataset_id)
            && data.episode_catalog_error(&dataset_id).is_none()
        {
            studio.run_utility("", data.load_episodes(dataset_id));
        }
    });

    // Keep the shared selection inside the active dataset without creating a
    // panel-local copy that could drift from Scene, cameras, and Timeline.
    // The comparison before `set` is essential: Dioxus effects are reactive
    // to reads, so writing `None` into an already-empty signal would schedule
    // this effect forever when a dataset has no episodes.
    use_effect(move || {
        let dataset_id = studio.selected_dataset().read().trim().to_owned();
        let episodes = data.episodes(&dataset_id);
        let current = data.selected_episode();
        let next = episodes.reconciled_selection(current.as_deref());
        if current != next {
            data.set_selected_episode(next);
        }
    });

    let dataset_id = studio.selected_dataset().read().trim().to_owned();
    let dataset_valid = spiral_robotics_common::identity::DatasetId::is_canonical(&dataset_id);
    let dataset_ids = data
        .datasets()
        .into_iter()
        .map(|dataset| dataset.id().to_owned())
        .chain(std::iter::once(dataset_id.clone()))
        .collect::<BTreeSet<_>>();
    let episodes = data.episodes(&dataset_id);
    let selected = data.selected_episode();
    let datasets_loading = data.datasets_loading();
    let catalog_loading = data.episodes_loading(&dataset_id);
    let catalog_loaded = data.episodes_loaded(&dataset_id);
    let dataset_error = data.dataset_catalog_error();
    let catalog_error = data.episode_catalog_error(&dataset_id);
    let catalog_issues = data.episode_catalog_issues(&dataset_id);
    let backend_readiness = studio.snapshot().read().readiness().clone();
    let storage_available = backend_readiness.storage_available();
    let storage_starting =
        !storage_available && backend_readiness.state() == BackendReadinessState::Bootstrapping;
    let storage_readiness_detail = backend_readiness.detail().to_owned();
    let reload_dataset_id = dataset_id.clone();
    let import_dataset_id = dataset_id.clone();
    let confirming_delete_id = confirming_delete.read().to_owned();
    let deleting_episode_id = deleting_episode.read().to_owned();

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
                            oninput: move |event| studio.selected_dataset().set(event.value()),
                        }
                    }
                    datalist { id: "data-dataset-options",
                        for id in dataset_ids { option { value: "{id}" } }
                    }
                    if dataset_valid && !catalog_loaded {
                        p { class: "truncate text-[10px] text-ink-faint",
                            if storage_starting { "Connecting to SpiralDB…" } else if !storage_available { "Catalog unavailable" } else if datasets_loading { "Loading datasets…" } else if catalog_loading { "Loading episodes…" } else { "Episodes not loaded" }
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
                        aria_label: if datasets_loading || catalog_loading { "Reloading datasets and episodes" } else { "Reload datasets and episodes" },
                        title: "Reload datasets, then episodes for the selected dataset",
                        disabled: datasets_loading || catalog_loading || !dataset_valid || !storage_available,
                        aria_busy: datasets_loading || catalog_loading,
                        onclick: move |_| {
                            studio.run_utility("", data.reload_dataset(reload_dataset_id.clone()));
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
                        disabled: !dataset_valid || !storage_available,
                        onclick: move |_| adding_data.toggle(),
                        if adding_data() { "Adding data" } else { "Add data" }
                    }
                    FileAction {
                        purpose: ActionPurpose::Import,
                        variant: ActionVariant::Outline,
                        title: "Import episodes into the selected dataset; existing episode IDs are never replaced",
                        disabled: importing() || !dataset_valid || !storage_available,
                        aria_busy: importing(),
                        input: FileInput::new(
                            ".spiral-episodes,application/vnd.spiral-robotics.episodes",
                            move |event: Event<FormData>| {
                            let Some(file) = event.files().first().cloned() else { return };
                            let target_dataset = import_dataset_id.clone();
                            studio.run_utility("", async move {
                                let Some(outcome) = SingleFlight::run(importing, async {
                                        let result = import_episodes(EpisodeImportUpload::new(
                                            target_dataset.clone(),
                                            file.into(),
                                        ))
                                        .await
                                        .map_err(|error| error.to_string())?;
                                        data.reload_dataset(target_dataset).await?;
                                        Ok::<_, String>(result)
                                    })
                                    .await
                                else {
                                    return Ok(());
                                };
                                match outcome {
                                    Ok(result) => {
                                        if let Some(episode) = result.first_episode() {
                                            data.set_selected_episode(Some(episode.to_owned()));
                                        }
                                        studio.report_success(format!(
                                            "Imported {} episodes into {} ({} rows).{}",
                                            result.episodes(),
                                            result.dataset_id(),
                                            result.rows(),
                                            result.warnings().import_warning_suffix(),
                                        ));
                                        Ok(())
                                    }
                                    Err(error) => Err(error),
                                }
                            });
                            },
                        ),
                        if importing() { "Importing…" } else { "Import bundle" }
                    }
                }
            }

            if adding_data() && dataset_valid && storage_available {
                HuggingFaceImport {
                    key: "{dataset_id}",
                    dataset_id: dataset_id.clone(),
                    on_close: move |_| adding_data.set(false),
                }
            }

            div { class: "min-h-0 flex-1 overflow-y-auto",
                if let Some(error) = dataset_error {
                    div { class: "mx-2 mt-2 rounded bg-danger-soft px-2 py-1.5 text-[11px] text-danger", role: "alert",
                        strong { class: "block font-medium", "Datasets unavailable" }
                        span { "{error}" }
                    }
                }
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
                            if storage_starting { "Connecting to SpiralDB…" } else if !storage_available { "Datasets unavailable" } else if datasets_loading { "Loading datasets…" } else if catalog_loading { "Loading episodes…" } else { "Episodes not loaded" }
                        }
                        if storage_starting || !storage_available {
                            p { class: "mt-1 text-[11px] text-ink-faint", "{storage_readiness_detail}" }
                        } else if !datasets_loading && !catalog_loading {
                            p { class: "mt-1 text-[11px] text-ink-faint", "Reload datasets and episodes to try again." }
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
                                key: "{episode.id()}",
                                episode: episode.clone(),
                                state: EpisodeRowState {
                                    selected: selected.as_deref() == Some(episode.id()),
                                    deletion: EpisodeDeletionState::for_episode(
                                        episode.id(),
                                        confirming_delete_id.as_deref(),
                                        deleting_episode_id.as_deref(),
                                    ),
                                },
                                actions: EpisodeRowActions {
                                    on_select: EventHandler::new(move |chosen: EpisodeListItem| {
                                        if deleting_episode.peek().is_none() {
                                            confirming_delete.set(None);
                                        }
                                        if data.selected_episode_peek().as_deref() != Some(chosen.id()) {
                                            data.set_selected_episode(Some(chosen.id().to_owned()));
                                        }
                                    }),
                                    on_delete: EventHandler::new(move |id: String| {
                                        if confirming_delete.read().as_deref() == Some(id.as_str()) {
                                            if deleting_episode.peek().is_none() {
                                                deleting_episode.set(Some(id.clone()));
                                                EpisodeDeletion {
                                                    studio,
                                                    data,
                                                    id,
                                                    signals: EpisodeDeletionSignals {
                                                        deleting: deleting_episode,
                                                        confirming: confirming_delete,
                                                    },
                                                }
                                                .delete();
                                            }
                                        } else {
                                            confirming_delete.set(Some(id));
                                        }
                                    }),
                                    on_cancel_delete: EventHandler::new(move |_| confirming_delete.set(None)),
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

trait ImportWarningsPresentation {
    fn import_warning_suffix(&self) -> String;
}

impl ImportWarningsPresentation for [String] {
    fn import_warning_suffix(&self) -> String {
        if self.is_empty() {
            String::new()
        } else {
            format!(" Warning: {}", self.join(" "))
        }
    }
}

#[derive(Clone, Copy)]
struct HuggingFaceImportState {
    source: HuggingFaceSourceSignals,
    operation: HuggingFaceOperationSignals,
}

#[derive(Clone, Copy)]
struct HuggingFaceSourceSignals {
    format: Signal<HuggingFaceDatasetFormat>,
    repository: Signal<String>,
    revision: Signal<String>,
}

#[derive(Clone, Copy)]
struct HuggingFaceOperationSignals {
    inspecting: Signal<bool>,
    importing: Signal<bool>,
    inspection: Signal<Option<HuggingFaceDatasetInspection>>,
    imported: Signal<Option<HuggingFaceDatasetImportSummary>>,
    error: Signal<Option<String>>,
}

impl HuggingFaceImportState {
    fn use_state() -> Self {
        Self {
            source: HuggingFaceSourceSignals {
                format: use_signal(|| HuggingFaceDatasetFormat::LeRobot),
                repository: use_signal(String::new),
                revision: use_signal(|| "main".to_owned()),
            },
            operation: HuggingFaceOperationSignals {
                inspecting: use_signal(|| false),
                importing: use_signal(|| false),
                inspection: use_signal(|| None),
                imported: use_signal(|| None),
                error: use_signal(|| None),
            },
        }
    }
}

/// Inspect-then-import flow for immutable Hugging Face snapshots. The panel is
/// inline because it belongs to the selected dataset and should not obscure
/// either the episode catalog or Query while an import is being prepared.
#[component]
fn HuggingFaceImport(dataset_id: String, on_close: EventHandler<()>) -> Element {
    let studio = studio();
    let controller = studio.data_controller();
    let HuggingFaceImportState {
        source:
            HuggingFaceSourceSignals {
                mut format,
                mut repository,
                mut revision,
            },
        operation:
            HuggingFaceOperationSignals {
                inspecting,
                importing,
                mut inspection,
                mut imported,
                mut error,
            },
    } = HuggingFaceImportState::use_state();

    let request = HuggingFaceDatasetSource::new(dataset_id.clone(), repository().trim(), format())
        .with_revision(revision().trim());
    let can_inspect = !request.repository().is_empty()
        && !request.revision().is_empty()
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
                                let Some(outcome) = SingleFlight::run(
                                    inspecting,
                                    async move {
                                        inspect_hugging_face_dataset(request)
                                            .await
                                            .map_err(|error| error.to_string())
                                    },
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
                            "Imported {summary.files()} files into {summary.relations().len()} Spiral relations at "
                            code { "{summary.resolved_revision().short_revision()}" }
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
                            div { dt { "Snapshot" } dd { code { "{source.resolved_revision().short_revision()}" } } }
                            div { dt { "Files" } dd { "{source.files().len()}" } }
                            div { dt { "Size" } dd { "{source.total_bytes().byte_size_label()}" } }
                            div { dt { "Target" } dd { code { "{source.dataset_id()}" } } }
                        }
                        div { class: "data-import-plan",
                            strong { "Spiral relations" }
                            ul {
                                for relation in source.planned_relations().iter() {
                                    li { code { "{relation}" } }
                                }
                            }
                        }
                        if !source.warnings().is_empty() {
                            div { class: "data-import-warnings", role: "note",
                                strong { "Before importing" }
                                ul {
                                    for warning in source.warnings().iter() { li { "{warning}" } }
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
                                    let import_request = HuggingFaceDatasetSource::new(
                                        source.dataset_id(),
                                        source.repository(),
                                        source.format(),
                                    )
                                    .with_revision(source.resolved_revision());
                                    let imported_dataset_id = source.dataset_id().to_owned();
                                    error.set(None);
                                    studio.run_utility("", async move {
                                        let Some(outcome) = SingleFlight::run(
                                            importing,
                                            async move {
                                                import_hugging_face_dataset(import_request)
                                                    .await
                                                    .map_err(|error| error.to_string())
                                            },
                                        )
                                        .await else {
                                            return Ok(());
                                        };
                                        match outcome {
                                            Ok(summary) => {
                                                studio.refresh_relation_catalog();
                                                controller.set_sql(summary.suggested_sql().to_owned());
                                                studio.report_success_from(
                                                    crate::shell::notifications::MessageSource::Data,
                                                    format!(
                                                        "Imported {} {} files from {} at {} into {} Spiral relations.",
                                                        summary.files(),
                                                        summary.format().label(),
                                                        summary.repository(),
                                                        summary.resolved_revision().short_revision(),
                                                        summary.relations().len(),
                                                    ),
                                                );
                                                imported.set(Some(summary.clone()));
                                                controller.reload_dataset(imported_dataset_id).await.map_err(|problem| {
                                                    format!(
                                                        "Dataset imported successfully, but Studio could not reload its episodes: {problem}"
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

trait RevisionPresentation {
    fn short_revision(&self) -> &str;
}

impl RevisionPresentation for str {
    fn short_revision(&self) -> &str {
        self.get(..10).unwrap_or(self)
    }
}

trait ByteCountPresentation {
    fn byte_size_label(self) -> String;
}

impl ByteCountPresentation for u64 {
    fn byte_size_label(self) -> String {
        const KIB: f64 = 1024.0;
        const MIB: f64 = KIB * 1024.0;
        const GIB: f64 = MIB * 1024.0;
        let bytes = self as f64;
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
}

/// Read-only SQL controls. Results render in their own sibling panel and stay
/// available while this panel is moved, tabbed, or obscured.
#[component]
pub(crate) fn QueryPanel(namespace: String, dataset_id: String, query_available: bool) -> Element {
    let studio = studio();
    let controller = studio.data_controller();
    let relation_result = studio.relation_catalog_result();
    let relation_error = relation_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .cloned();
    let relation_response = relation_result
        .as_ref()
        .and_then(|result| result.as_ref().ok());
    let schema_relations = SchemaRelationSummary::for_dataset(
        &namespace,
        &dataset_id,
        query_available.then_some(relation_response).flatten(),
    );
    let dataset_valid = spiral_robotics_common::identity::DatasetId::is_canonical(&dataset_id);
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
                                onclick: move |_| studio.refresh_relation_catalog(),
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

#[derive(Clone, Copy, PartialEq)]
struct EpisodeRowState {
    selected: bool,
    deletion: EpisodeDeletionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EpisodeDeletionState {
    Available,
    Confirming,
    Deleting,
    Disabled,
}

impl EpisodeDeletionState {
    fn for_episode(episode: &str, confirming: Option<&str>, deleting: Option<&str>) -> Self {
        if deleting == Some(episode) {
            Self::Deleting
        } else if confirming == Some(episode) {
            Self::Confirming
        } else if deleting.is_some() {
            Self::Disabled
        } else {
            Self::Available
        }
    }
}

#[derive(Clone, PartialEq)]
struct EpisodeRowActions {
    on_select: EventHandler<EpisodeListItem>,
    on_delete: EventHandler<String>,
    on_cancel_delete: EventHandler<()>,
}

#[component]
fn EpisodeRow(
    episode: EpisodeListItem,
    state: EpisodeRowState,
    actions: EpisodeRowActions,
) -> Element {
    let delete_title = "Delete this episode from SpiralDB";

    rsx! {
        li {
            class: "bg-surface",
            article { class: "data-episode-row ui-hover-action-host flex min-h-12 flex-wrap items-center gap-1 px-1.5 py-1",
                Action {
                    purpose: ActionPurpose::Select,
                    variant: ActionVariant::Row,
                    class: "data-episode-select ui-action--bare min-w-0 flex-1 rounded px-1 py-0.5 text-left focus-visible:outline-offset-0",
                    aria_pressed: state.selected,
                    onclick: {
                        let episode = episode.clone();
                        move |_| actions.on_select.call(episode.clone())
                    },
                    span { class: "flex items-baseline gap-1.5",
                        strong { class: "truncate font-mono text-[11px] font-medium text-ink", "{episode.id().short_episode_id()}" }
                        span { class: "ml-auto shrink-0 font-mono text-[10px] text-ink-soft", "{episode.duration_seconds().clock_label()}" }
                    }
                    span { class: "mt-0.5 flex min-w-0 items-center gap-1.5 text-[10px] text-ink-faint",
                        span { class: "truncate", title: "{episode.source()}", "{episode.source()}" }
                        span { class: "ml-auto shrink-0 font-mono", "{episode.frames()}f · {episode.sample_hz():.0}Hz" }
                    }
                }
                if matches!(state.deletion, EpisodeDeletionState::Confirming | EpisodeDeletionState::Deleting) {
                    InlineConfirmation {
                        id: "delete-episode-confirmation-{episode.id()}",
                        class: "data-delete-confirmation",
                        copy: ConfirmationCopy::new(
                            format!(
                                "Delete episode {}? Its streams and video will be permanently removed from SpiralDB.",
                                episode.id().short_episode_id(),
                            ),
                            if state.deletion == EpisodeDeletionState::Deleting { "Deleting…" } else { "Delete episode" },
                            "Keep episode",
                        ),
                        response: ConfirmationResponse::new(
                            ActionPurpose::Delete,
                            {
                                let id = episode.id().to_owned();
                                move |_| actions.on_delete.call(id.clone())
                            },
                            move |_| actions.on_cancel_delete.call(()),
                        ),
                        dismissal: ConfirmationDismissal::new(
                            format!("delete-episode-{}", episode.id()),
                        ).with_pending(state.deletion == EpisodeDeletionState::Deleting),
                    }
                } else {
                    div { class: "data-episode-actions",
                        Action {
                            id: "delete-episode-{episode.id()}",
                            purpose: ActionPurpose::Delete,
                            variant: ActionVariant::Link,
                            class: "data-delete-action ui-hover-action",
                            disabled: state.deletion == EpisodeDeletionState::Disabled,
                            title: delete_title,
                            onclick: {
                                let id = episode.id().to_owned();
                                move |_| actions.on_delete.call(id.clone())
                            },
                            "Delete"
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SchemaRelationSummary {
    name: String,
    label: String,
    description: String,
    columns: String,
    sql: String,
}

impl SchemaRelationSummary {
    fn for_dataset(
        namespace: &str,
        dataset_id: &str,
        response: Option<&RelationCatalog>,
    ) -> Vec<Self> {
        let Some(response) = response else {
            return Table::ALL
                .into_iter()
                .map(|table| Self {
                    name: table.qualified_name(namespace),
                    label: table.name().to_owned(),
                    description: table.description().to_owned(),
                    columns: table.columns().join(", "),
                    sql: table.scoped_sql(namespace, dataset_id),
                })
                .collect();
        };
        response
            .relations()
            .iter()
            .map(|relation| {
                let managed = Table::ALL
                    .into_iter()
                    .find(|table| relation.name() == table.qualified_name(namespace));
                let label = managed
                    .map(|table| table.name().to_owned())
                    .unwrap_or_else(|| {
                        relation
                            .name()
                            .strip_prefix(&format!("{namespace}."))
                            .unwrap_or(relation.name())
                            .to_owned()
                    });
                Self {
                    name: relation.name().to_owned(),
                    label,
                    description: managed
                        .map(ManagedTablePresentation::description)
                        .unwrap_or(relation.relation_type())
                        .to_owned(),
                    columns: relation
                        .columns()
                        .iter()
                        .map(|column| column.name())
                        .collect::<Vec<_>>()
                        .join(", "),
                    sql: managed
                        .map(|table| table.scoped_sql(namespace, dataset_id))
                        .unwrap_or_else(|| format!("SELECT * FROM {} LIMIT 100", relation.name())),
                }
            })
            .collect()
    }
}

/// The workbench narrows a relation's default query to the selected dataset
/// wherever the relation can reach one; the rest keep the shared preset.
pub(crate) trait QueryPresetSql {
    fn scoped_sql(self, namespace: &str, dataset_id: &str) -> String;
}

impl QueryPresetSql for Table {
    fn scoped_sql(self, namespace: &str, dataset_id: &str) -> String {
        let dataset_id = dataset_id.replace('\'', "''");
        let episodes = Table::Episodes.qualified_name(namespace);
        let relation = self.qualified_name(namespace);
        match self {
            Table::Episodes => format!(
                "SELECT * FROM {relation} WHERE dataset_id = '{dataset_id}' ORDER BY started_ms DESC LIMIT 100"
            ),
            Table::EpisodeStreams => format!(
                "SELECT s.* FROM {relation} AS s JOIN {episodes} AS e ON e.episode_id = s.episode_id WHERE e.dataset_id = '{dataset_id}' ORDER BY s.episode_id DESC, s.stream_id LIMIT 100"
            ),
            Table::JointSamples => format!(
                "SELECT j.* FROM {relation} AS j JOIN {episodes} AS e ON e.episode_id = j.episode_id WHERE e.dataset_id = '{dataset_id}' ORDER BY j.episode_id DESC, j.stream_id, j.sample_index LIMIT 100"
            ),
            Table::VideoAssets => format!(
                "SELECT v.episode_id, v.stream_id, v.segment_index, v.camera_id, v.byte_size, v.width, v.height, v.fps, v.frames FROM {relation} AS v JOIN {episodes} AS e ON e.episode_id = v.episode_id WHERE e.dataset_id = '{dataset_id}' ORDER BY v.episode_id DESC, v.stream_id, v.segment_index LIMIT 100"
            ),
            Table::Datasets
            | Table::TrainingCheckpoints
            | Table::Robots
            | Table::StudioSettings
            | Table::TrainingRuns
            | Table::TrainingRunMetrics
            | Table::TrainingRunCheckpoints
            | Table::RolloutRuns
            | Table::WorkflowRuns
            | Table::FeatureRegistry => self.sql(namespace),
        }
    }
}

trait ManagedTablePresentation {
    fn description(self) -> &'static str;
}

impl ManagedTablePresentation for Table {
    fn description(self) -> &'static str {
        match self {
            Table::Datasets => "Dataset identity and metadata",
            Table::Episodes => "Completed episodes and their datasets",
            Table::EpisodeStreams => "Available joint and camera streams",
            Table::JointSamples => "Synchronized state and action samples",
            Table::VideoAssets => "Recorded MP4 segments and media metadata",
            Table::TrainingCheckpoints => "Saved checkpoints by training run",
            Table::Robots => "Robot configuration and hardware assignments",
            Table::StudioSettings => "Persistent Studio preferences",
            Table::TrainingRuns => "Training run configuration and lifecycle",
            Table::TrainingRunMetrics => "Per-step metrics for each training run",
            Table::TrainingRunCheckpoints => "Checkpoint history for each training run",
            Table::RolloutRuns => "Policy rollout lifecycle and outcomes",
            Table::WorkflowRuns => "Saved Workflow graph and latest run snapshot",
            Table::FeatureRegistry => "Registered Workflow feature definitions and provenance",
        }
    }
}

trait EpisodeSelection {
    fn preferred(&self) -> Option<&EpisodeListItem>;
    fn reconciled_selection(&self, current: Option<&str>) -> Option<String>;
}

impl EpisodeSelection for [EpisodeListItem] {
    fn preferred(&self) -> Option<&EpisodeListItem> {
        self.iter()
            .rev()
            .find(|episode| episode.usable() && !episode.discarded())
            .or_else(|| self.last())
    }

    fn reconciled_selection(&self, current: Option<&str>) -> Option<String> {
        current
            .filter(|id| self.iter().any(|episode| episode.id() == *id))
            .map(str::to_owned)
            .or_else(|| self.preferred().map(|episode| episode.id().to_owned()))
    }
}

struct EpisodeDeletionSignals {
    deleting: Signal<Option<String>>,
    confirming: Signal<Option<String>>,
}

struct EpisodeDeletion {
    studio: crate::shell::state::Studio,
    data: crate::activities::data::state::DataController,
    id: String,
    signals: EpisodeDeletionSignals,
}

impl EpisodeDeletion {
    fn delete(self) {
        let deleted_id = self.id.clone();
        let mut deleting = self.signals.deleting;
        let mut confirming = self.signals.confirming;
        self.studio
            .run_utility(format!("Deleted {}.", self.id), async move {
                let result = delete_episode(deleted_id.clone())
                    .await
                    .map_err(|error| error.to_string());
                deleting.set(None);
                if result.is_ok() {
                    confirming.set(None);
                    self.data.remove_episode(&deleted_id);
                    // A slower deletion must not clear a newer choice made while the
                    // request was in flight.
                    if self.data.selected_episode_peek().as_deref() == Some(deleted_id.as_str()) {
                        self.data.set_selected_episode(None);
                    }
                }
                result
            });
    }
}

pub(crate) trait EpisodeIdPresentation {
    fn short_episode_id(&self) -> String;
}

impl EpisodeIdPresentation for str {
    fn short_episode_id(&self) -> String {
        self.strip_prefix("episode_")
            .and_then(|number| number.parse::<u64>().ok())
            .map(|number| format!("#{number}"))
            .unwrap_or_else(|| self.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::recording::EpisodeInfo;

    fn episode(id: &str, usable: bool, discarded: bool) -> EpisodeListItem {
        EpisodeListItem::from(
            &EpisodeInfo::new(id, "default")
                .with_source("test")
                .with_timeline(1, 1.0)
                .with_sample_hz(60.0)
                .with_assessment(discarded, None)
                .with_problem((!usable).then(|| "unavailable".to_owned())),
        )
    }

    #[test]
    fn episode_selection_reconciliation_is_stable_when_the_dataset_is_empty() {
        let episodes = Vec::<EpisodeListItem>::new();
        assert_eq!(episodes.reconciled_selection(None), None);
        assert_eq!(episodes.reconciled_selection(Some("deleted")), None);
    }

    #[test]
    fn episode_selection_reconciliation_preserves_valid_choices_and_replaces_stale_ones() {
        let episodes = [
            episode("older", true, false),
            episode("discarded", true, true),
            episode("newer", true, false),
        ];
        assert_eq!(
            episodes.reconciled_selection(Some("older")).as_deref(),
            Some("older")
        );
        assert_eq!(
            episodes.reconciled_selection(Some("deleted")).as_deref(),
            Some("newer")
        );
    }

    #[test]
    fn episode_deletion_state_has_one_authoritative_variant_per_row() {
        assert_eq!(
            EpisodeDeletionState::for_episode("take-1", None, None),
            EpisodeDeletionState::Available
        );
        assert_eq!(
            EpisodeDeletionState::for_episode("take-1", Some("take-1"), None),
            EpisodeDeletionState::Confirming
        );
        assert_eq!(
            EpisodeDeletionState::for_episode("take-1", Some("take-1"), Some("take-1")),
            EpisodeDeletionState::Deleting
        );
        assert_eq!(
            EpisodeDeletionState::for_episode("take-2", Some("take-1"), Some("take-1")),
            EpisodeDeletionState::Disabled
        );
    }

    #[test]
    fn presets_are_scoped_to_the_current_dataset() {
        assert_eq!(
            Table::JointSamples.scoped_sql("training", "pick-place"),
            "SELECT j.* FROM training.joint_samples AS j JOIN training.episodes AS e ON e.episode_id = j.episode_id WHERE e.dataset_id = 'pick-place' ORDER BY j.episode_id DESC, j.stream_id, j.sample_index LIMIT 100"
        );
        assert_eq!(
            Table::Episodes.scoped_sql("training", "pick-'place"),
            "SELECT * FROM training.episodes WHERE dataset_id = 'pick-''place' ORDER BY started_ms DESC LIMIT 100"
        );
        assert_eq!(
            Table::VideoAssets.scoped_sql("training", "pick-place"),
            "SELECT v.episode_id, v.stream_id, v.segment_index, v.camera_id, v.byte_size, v.width, v.height, v.fps, v.frames FROM training.video_assets AS v JOIN training.episodes AS e ON e.episode_id = v.episode_id WHERE e.dataset_id = 'pick-place' ORDER BY v.episode_id DESC, v.stream_id, v.segment_index LIMIT 100"
        );
    }

    #[test]
    fn generated_episode_ids_read_as_take_numbers() {
        assert_eq!("episode_000024".short_episode_id(), "#24");
        assert_eq!("imported-pass".short_episode_id(), "imported-pass");
    }
}
