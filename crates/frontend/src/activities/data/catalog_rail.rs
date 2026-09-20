//! Datasets and their episodes, the way an operator thinks about recordings:
//! a dataset is a folder of MCAP files, and each file is one episode.
use super::{
    DatasetClient,
    episode_list::{ListedEpisode, OpenedEpisode, SharedSourcePrefix},
};
use crate::components::icons::DataIcon;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetInspection, DatasetSelection, ProjectResource};
use spiral_robotics_common::relations::SpiralQuerySnapshot;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, RefreshIcon, TextInput, TextInputKind, Tree,
    TreeBranch, TreeItem,
};

type ResourceCatalog = ReadSignal<Option<Result<Vec<ProjectResource>, String>>>;
const EPISODE_PAGE_SIZE: usize = 50;

/// One chronological page of a dataset's members.
struct EpisodePage {
    episodes: Vec<ListedEpisode>,
    has_more: bool,
}

impl EpisodePage {
    fn sql(dataset: &DatasetSelection, offset: usize) -> String {
        format!(
            "SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, \
             message_start_time, message_end_time, duration_ns, channel_count \
             FROM spql.mcap.episodes('{}') ORDER BY message_start_time, source_uri LIMIT {} OFFSET {offset}",
            dataset.sql_name().replace('\'', "''"),
            EPISODE_PAGE_SIZE + 1
        )
    }
    fn from_snapshot(snapshot: SpiralQuerySnapshot, offset: usize) -> Result<Self, String> {
        let has_more = snapshot.rows().len() > EPISODE_PAGE_SIZE || snapshot.truncated();
        let episodes = snapshot
            .into_rows()
            .into_iter()
            .take(EPISODE_PAGE_SIZE)
            .enumerate()
            .map(|(index, row)| {
                serde_json::from_value(serde_json::Value::Object(row))
                    .map(|record| ListedEpisode::new(offset + index + 1, record))
                    .map_err(|error| format!("Reading episode metadata: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { episodes, has_more })
    }
}

/// What the rail reports back to the workspace.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct RailActions {
    on_select_dataset: EventHandler<DatasetSelection>,
    on_open_episode: EventHandler<OpenedEpisode>,
    on_import: EventHandler<()>,
}

impl RailActions {
    pub(super) fn new(
        on_select_dataset: EventHandler<DatasetSelection>,
        on_open_episode: EventHandler<OpenedEpisode>,
    ) -> Self {
        Self {
            on_select_dataset,
            on_open_episode,
            on_import: EventHandler::new(|_| {}),
        }
    }
    pub(super) fn with_import(mut self, on_import: EventHandler<()>) -> Self {
        self.on_import = on_import;
        self
    }
}

/// What the workspace currently shows, so the rail can highlight it.
#[derive(Clone, PartialEq)]
pub(super) struct RailSelection {
    dataset: Option<DatasetSelection>,
    episode_id: Option<String>,
    inspection: Option<DatasetInspection>,
}

impl RailSelection {
    pub(super) fn new(
        dataset: Option<DatasetSelection>,
        episode_id: Option<String>,
        inspection: Option<DatasetInspection>,
    ) -> Self {
        Self {
            dataset,
            episode_id,
            inspection,
        }
    }
}

#[component]
pub(super) fn CatalogRail(
    resources: ResourceCatalog,
    selection: RailSelection,
    actions: RailActions,
) -> Element {
    let mut filter = use_signal(String::new);
    let result = resources.read();
    rsx! {
        section { class: "data-rail", "aria-label": "Datasets",
            header { class: "data-rail-head",
                TextInput { class: "data-rail-search", kind: TextInputKind::Search, placeholder: "Filter datasets and episodes",
                    aria_label: "Filter datasets and episodes", value: filter(), oninput: move |value| filter.set(value) }
                Button { purpose: ActionPurpose::Refresh, variant: ButtonVariant::Icon, aria_label: "Refresh catalog", title: "Refresh catalog",
                    onclick: move |_| super::relations::CatalogRefresh::request(), RefreshIcon {} }
                Button { purpose: ActionPurpose::Import, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                    title: "Import MCAP recordings as a new dataset", onclick: move |_| actions.on_import.call(()), "Import" }
            }
            div { class: "data-rail-scroll",
                match &*result {
                    None => rsx! { p { class: "data-rail-note", role: "status", "Reading SpiralDB catalog…" } },
                    Some(Err(error)) => rsx! { p { class: "data-rail-note", role: "alert", "{error} Refresh to retry." } },
                    Some(Ok(entries)) => {
                        let needle = filter().to_lowercase();
                        // A search never hides a dataset: each one stays listed and
                        // searches its own episodes, so a match in a collapsed dataset
                        // is found and the operator's expansion choices survive.
                        let datasets = entries.iter().filter(|resource| resource.kind() == "dataset").cloned().collect::<Vec<_>>();
                        rsx! {
                            if datasets.is_empty() {
                                div { class: "data-rail-note",
                                    strong { "No datasets yet" }
                                    p { "Import MCAP recordings to create the first one." }
                                }
                            }
                            Tree { label: "Datasets", class: "data-dataset-list",
                                for resource in datasets.iter() {
                                    DatasetBranch {
                                        key: "{resource.resource_id()}",
                                        resource: resource.clone(),
                                        selected: selection.dataset.as_ref() == Some(resource.selection()),
                                        selected_episode_id: selection.episode_id.clone(),
                                        inspection: selection.inspection.clone().filter(|_| selection.dataset.as_ref() == Some(resource.selection())),
                                        filter: needle.clone(),
                                        actions,
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DatasetBranch(
    resource: ProjectResource,
    selected: bool,
    selected_episode_id: Option<String>,
    inspection: Option<DatasetInspection>,
    filter: String,
    actions: RailActions,
) -> Element {
    let mut expanded = use_signal(|| false);
    use_effect(move || {
        if selected {
            expanded.set(true);
        }
    });
    let name = resource.selection().label();
    let members = inspection.as_ref().and_then(DatasetInspection::members);
    let select = resource.selection().clone();
    // A search opens every dataset so its matches show; the operator's own
    // expansion choice returns when the field clears.
    let open = expanded() || !filter.is_empty();
    let branch = TreeBranch::new(name.clone(), open, move |next| expanded.set(next));
    rsx! {
        TreeItem { class: "data-dataset", branch, selected,
            onselect: move |_| { actions.on_select_dataset.call(select.clone()); expanded.set(true); },
            label: rsx! {
                DataIcon {}
                span { class: "truncate", title: "{name}", "{name}" }
            },
            trailing: rsx! {
                span { class: "data-dataset-count", title: "Episodes",
                    if let Some(members) = members { "{members}" }
                }
            },
            DatasetEpisodes { key: "{resource.resource_id()}", dataset: resource.selection().clone(), selected_episode_id, filter, on_open: actions.on_open_episode }
        }
    }
}

#[component]
fn DatasetEpisodes(
    dataset: DatasetSelection,
    selected_episode_id: Option<String>,
    filter: String,
    on_open: EventHandler<OpenedEpisode>,
) -> Element {
    let mut loaded = use_signal(Vec::<ListedEpisode>::new);
    let mut has_more = use_signal(|| false);
    let mut offset = use_signal(|| 0_usize);
    let query_dataset = dataset.clone();
    let mut page = use_resource(move || {
        let dataset = query_dataset.clone();
        let offset = offset();
        async move {
            let snapshot = DatasetClient::current()
                .run_spiral_query(EpisodePage::sql(&dataset, offset), EPISODE_PAGE_SIZE + 1)
                .await
                .map_err(|error| error.to_string())?;
            EpisodePage::from_snapshot(snapshot, offset)
        }
    });
    use_effect(move || {
        if let Some(Ok(page)) = &*page.read() {
            let first = page
                .episodes
                .first()
                .map_or(usize::MAX, ListedEpisode::ordinal);
            let mut all = loaded.write();
            all.retain(|episode| episode.ordinal() < first);
            all.extend(page.episodes.iter().cloned());
            has_more.set(page.has_more);
        }
    });
    // A search reads every page, so a match on page three is found without a
    // "Load more" click; the pages stay loaded once the field clears.
    let searching = !filter.is_empty();
    use_effect(use_reactive((&searching,), move |(searching,)| {
        let pending = *page.state().read() == UseResourceState::Pending;
        let more = matches!(&*page.read(), Some(Ok(page)) if page.has_more);
        if searching && more && !pending {
            let next = *offset.peek() + EPISODE_PAGE_SIZE;
            offset.set(next);
        }
    }));
    let pending = *page.state().read() == UseResourceState::Pending;
    let episodes = loaded.read();
    let prefix = episodes.shared_source_prefix();
    let rows = episodes
        .iter()
        .filter(|episode| episode.matches(&filter))
        .cloned()
        .collect::<Vec<_>>();
    let unmatched = searching && rows.is_empty() && !episodes.is_empty();
    rsx! {
        Fragment {
            for episode in rows {
                {
                    let source = episode.record().source_label(prefix);
                    let selected = selected_episode_id.as_deref() == Some(episode.record().id());
                    let open_dataset = dataset.clone();
                    let open_source = source.clone();
                    let open_episode = episode.clone();
                    rsx! {
                        TreeItem { key: "{episode.record().id()}", class: "data-episode", selected,
                            title: "{episode.record().facts_label()}\n{episode.record().source_uri().unwrap_or_default()}",
                            onselect: move |_| on_open.call(OpenedEpisode::new(open_dataset.clone(), open_episode.clone(), open_source.clone())),
                            label: rsx! {
                                span { class: "data-episode-ordinal", "{episode.title()}" }
                                span { class: "data-episode-head", "{source.head()}" }
                                span { class: "data-episode-duration", "{episode.record().duration_label()}" }
                                span { class: "data-episode-sub",
                                    if let Some(tail) = source.tail() { span { class: "data-episode-tail", "{tail}" } }
                                    span { "{episode.record().recorded_date_label()}" }
                                    span { "{episode.record().message_count_label()} msgs" }
                                }
                            },
                        }
                    }
                }
            }
            if pending {
                li { class: "data-rail-note", role: "status", if searching { "Searching…" } else { "Loading episodes…" } }
            } else if let Some(Err(error)) = &*page.read() {
                li { class: "data-rail-note", role: "alert",
                    p { "{error}" }
                    Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Link, size: ButtonSize::Compact, onclick: move |_| page.restart(), "Retry" }
                }
            } else if episodes.is_empty() {
                li { class: "data-rail-note", "No episodes in this dataset." }
            } else if unmatched && !has_more() {
                li { class: "data-rail-note", "No episodes match." }
            } else if has_more() && !searching {
                li { class: "data-rail-more",
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                        onclick: move |_| offset.set(offset() + EPISODE_PAGE_SIZE), "Load {EPISODE_PAGE_SIZE} more" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, uri: &str, start: u64) -> serde_json::Map<String, serde_json::Value> {
        serde_json::json!({
            "id": id, "source_uri": uri, "source_bytes": 15_932_137_u64, "message_count": 11_404_u64,
            "message_start_time": start, "message_end_time": start + 20_501_879_296, "duration_ns": 20_501_879_296_u64,
            "channel_count": 23
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn labels_start_where_source_paths_differ() {
        let page = EpisodePage::from_snapshot(
            SpiralQuerySnapshot::new(
                vec![],
                vec![
                    row(
                        "a",
                        "hf://d/x@1/data/val/fold_skirts/episode_aa/episode.fo.mcap",
                        1_743_701_335_734_982_912,
                    ),
                    row(
                        "b",
                        "hf://d/x@1/data/val/place_fruit/episode_bb/episode.fo.mcap",
                        1_744_170_956_524_465_408,
                    ),
                ],
                false,
            ),
            50,
        )
        .unwrap();
        let prefix = page.episodes.shared_source_prefix();
        let first = page.episodes[0].record().source_label(prefix);
        assert_eq!(first.head(), "fold_skirts");
        assert_eq!(first.tail(), Some("episode_aa/episode.fo.mcap"));
        assert_eq!(page.episodes[0].ordinal(), 51);
        assert_eq!(
            page.episodes[0].record().facts_label(),
            "Recorded 2025-04-03 17:28:55 UTC · 11,404 messages · 23 channels · 15.2 MiB"
        );
        assert_eq!(page.episodes[0].record().duration_label(), "0:20.5");
        assert_eq!(page.episodes[0].record().message_count_label(), "11,404");
        assert!(!page.has_more);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn identifier_named_files_read_as_recording_times_or_stay_unnamed() {
        let page = EpisodePage::from_snapshot(
            SpiralQuerySnapshot::new(
                vec![],
                vec![
                    row(
                        "a",
                        "spfsu://p/attempts/01a07ed2-1ed5-7542-bbf3-15a78c389242/blobs/01a07ed2-1fdd-77a2-a9ae-5c3bbca09151.bin",
                        1_743_701_335_734_982_912,
                    ),
                    row(
                        "b",
                        "spfsu://p/attempts/01a0b638-0359-74d2-b28c-3daf15a6ecfa/blobs/01a0b638-099b-7740-a44b-ef08485546ed.bin",
                        0,
                    ),
                ],
                false,
            ),
            0,
        )
        .unwrap();
        let prefix = page.episodes.shared_source_prefix();
        let [clocked, epoch] = page.episodes.as_slice() else {
            panic!("two episodes");
        };
        let label = clocked.record().source_label(prefix);
        assert_eq!(label.head(), "Recorded 17:28:55 UTC");
        assert_eq!(label.tail(), None);
        assert_eq!(clocked.record().recorded_date_label(), "2025-04-03");
        let label = epoch.record().source_label(prefix);
        assert_eq!(label.head(), "Unnamed recording");
        assert_eq!(epoch.record().recorded_date_label(), "no wall clock");
        assert!(
            epoch
                .record()
                .facts_label()
                .starts_with("Recorded without a wall clock · ")
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_search_term_matches_the_source_path_or_ordinal_regardless_of_case() {
        let page = EpisodePage::from_snapshot(
            SpiralQuerySnapshot::new(
                vec![],
                vec![
                    row(
                        "a",
                        "hf://d/x@1/data/Fold_Skirts/episode_aa/episode.fo.mcap",
                        1,
                    ),
                    row(
                        "b",
                        "hf://d/x@1/data/place_fruit/episode_bb/episode.fo.mcap",
                        2,
                    ),
                ],
                false,
            ),
            50,
        )
        .unwrap();
        let [skirts, fruit] = page.episodes.as_slice() else {
            panic!("two episodes");
        };
        assert!(skirts.matches("fold"));
        assert!(!fruit.matches("fold"));
        assert!(fruit.matches("#52"));
        assert!(fruit.matches(""));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_single_file_is_labelled_by_its_name_and_bad_rows_are_errors() {
        let page = EpisodePage::from_snapshot(
            SpiralQuerySnapshot::new(
                vec![],
                vec![row("a", "spfsu://project/attempts/take-24.mcap", 0)],
                false,
            ),
            0,
        )
        .unwrap();
        let prefix = page.episodes.shared_source_prefix();
        assert_eq!(
            page.episodes[0].record().source_label(prefix).head(),
            "take-24.mcap"
        );
        assert!(
            EpisodePage::from_snapshot(
                SpiralQuerySnapshot::new(vec![], vec![Default::default()], false),
                0
            )
            .is_err()
        );
        let dataset = DatasetSelection::new(vec!["owner's recordings".into()]).unwrap();
        assert!(EpisodePage::sql(&dataset, 50).contains("'owner''s recordings'"));
        assert!(EpisodePage::sql(&dataset, 50).ends_with("LIMIT 51 OFFSET 50"));
    }
}
