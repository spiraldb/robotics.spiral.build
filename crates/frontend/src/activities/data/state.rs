//! Durable state and transitions for the Data screen.
//!
//! The controller outlives the mounted Data workbench. Views render its
//! signals and emit semantic transitions. The selected episode drives one
//! resource, while successful immutable episode data is cached on its catalog
//! entry.

use crate::activities::data::DatasetClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use std::sync::Arc;

use dioxus::prelude::*;
use spiral_robotics_common::identity::DatasetId;
use spiral_robotics_common::recording::{DatasetSummary, EpisodeInfo, EpisodeListItem};

use crate::activities::data::dataset_catalog::DatasetCatalogState;
use crate::activities::data::episode_cache::EpisodeData;
use crate::activities::data::query_result::{
    DEFAULT_QUERY_ROWS, QueryExecutionPresentation, QueryState, QueryTab,
};
use crate::activities::data::sql_draft::GeneratedSqlDraft;
use spiral_robotics_common::native_datasets::NativeDatasetSelection;

use crate::shell::notifications::MessageSource;
use crate::shell::state::Studio;

pub(crate) const MAX_QUERY_TABS: usize = 8;

struct ActiveQuery {
    id: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct DataController {
    selected_episode: Signal<Option<String>>,
    dataset_catalog: Signal<DatasetCatalogState>,
    episode_catalog: Signal<EpisodeCatalogState>,
    episode_loader: Resource<Option<EpisodeLoadResult>>,
    draft: QueryDraftSignals,
    execution: QueryExecutionSignals,
}

#[derive(Clone, Debug)]
struct EpisodeLoadResult {
    episode: String,
    expected_frames: u64,
    result: Result<Arc<EpisodeData>, String>,
}

#[derive(Clone, Debug, Default)]
struct EpisodeCatalogState {
    dataset_id: Option<String>,
    episodes: Vec<EpisodeEntry>,
    issues: Vec<String>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
}

/// One catalog record and the full data loaded for that same stable identity.
/// Attaching the data to its record prevents a late request from replacing a
/// different selected episode.
#[derive(Clone, Debug)]
struct EpisodeEntry {
    summary: EpisodeListItem,
    full_data: Option<Arc<EpisodeData>>,
}

/// The selected resource projected into the state consumed by recorded views.
/// Catalog entries cache only successful data, never this transient state.
#[derive(Clone, Debug)]
pub(crate) enum EpisodeDataState {
    Unloaded,
    Loading,
    Ready(Arc<EpisodeData>),
    Failed(String),
}

impl PartialEq for EpisodeDataState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unloaded, Self::Unloaded) | (Self::Loading, Self::Loading) => true,
            (Self::Ready(left), Self::Ready(right)) => Arc::ptr_eq(left, right),
            (Self::Failed(left), Self::Failed(right)) => left == right,
            _ => false,
        }
    }
}

impl EpisodeEntry {
    fn new(summary: EpisodeListItem) -> Self {
        Self {
            summary,
            full_data: None,
        }
    }

    fn replace_summary(mut self, summary: EpisodeListItem) -> Self {
        if self.summary.frames() != summary.frames() {
            self.full_data = None;
        }
        self.summary = summary;
        self
    }
}

#[derive(Clone, Copy)]
struct QueryDraftSignals {
    sql: Signal<String>,
    generated_sql: Signal<String>,
    max_rows: Signal<usize>,
}

#[derive(Clone, Copy)]
struct QueryExecutionSignals {
    query_tabs: Signal<Vec<QueryTab>>,
    active_query: Signal<Option<u64>>,
    active_request: Signal<Option<ActiveQuery>>,
    next_query_id: Signal<u64>,
}

impl DataController {
    /// Create the Data state owned by the Studio session. None of these
    /// signals depends on the Data workbench being mounted.
    pub(crate) fn use_controller(_namespace: &str, dataset_id: &str) -> Self {
        let initial_sql =
            NativeDatasetSelection::new(dataset_id.split('.').map(str::to_owned).collect())
                .map(|selection| selection.episodes_sql())
                .unwrap_or_default();
        let sql_value = initial_sql.clone();
        let selected_episode = use_signal(|| None::<String>);
        let dataset_catalog = use_signal(DatasetCatalogState::default);
        let episode_catalog = use_signal(EpisodeCatalogState::default);
        let sessions = crate::shell::project::ProjectSessionSignals::current();
        let episode_loader = use_resource(move || {
            let available = sessions.storage_available();
            let selected = selected_episode();
            let source = selected.and_then(|episode| {
                episode_catalog
                    .peek()
                    .episodes
                    .iter()
                    .find(|entry| entry.summary.id() == episode)
                    .map(|entry| {
                        (
                            episode,
                            entry.summary.frames(),
                            entry.full_data.as_ref().map(Arc::clone),
                        )
                    })
            });
            async move {
                if !available {
                    return std::future::pending().await;
                }
                let (episode, expected_frames, cached) = source?;
                let result = match cached {
                    Some(data) => Ok(data),
                    None => EpisodeData::load(&episode).await,
                }
                .and_then(|data| {
                    if data.info().id() == episode && data.info().frames() == expected_frames {
                        Ok(data)
                    } else {
                        Err(format!(
                            "Episode data did not match the selected catalog entry (expected {episode} with {expected_frames} frames, received {} with {} frames).",
                            data.info().id(),
                            data.info().frames(),
                        ))
                    }
                });
                Some(EpisodeLoadResult {
                    episode,
                    expected_frames,
                    result,
                })
            }
        });
        let controller = Self {
            selected_episode,
            dataset_catalog,
            episode_catalog,
            episode_loader,
            draft: QueryDraftSignals {
                sql: use_signal(move || sql_value),
                generated_sql: use_signal(move || initial_sql),
                max_rows: use_signal(|| DEFAULT_QUERY_ROWS),
            },
            execution: QueryExecutionSignals {
                query_tabs: use_signal(Vec::new),
                active_query: use_signal(|| None),
                active_request: use_signal(|| None),
                next_query_id: use_signal(|| 1),
            },
        };
        use_effect(move || {
            if episode_loader.state().cloned() != UseResourceState::Ready {
                return;
            }
            let loaded = episode_loader.read();
            let Some(Some(loaded)) = loaded.as_ref() else {
                return;
            };
            controller.cache_episode_data(loaded);
        });
        let dataset_dependency = dataset_id.to_owned();
        use_effect(use_reactive((&dataset_dependency,), move |(dataset_id,)| {
            controller.sync_dataset(&dataset_id)
        }));
        controller
    }

    pub(crate) fn selected_episode_peek(&self) -> Option<String> {
        self.selected_episode.peek().to_owned()
    }

    pub(crate) fn set_selected_episode(mut self, episode: Option<String>) {
        if self.selected_episode.peek().as_ref() != episode.as_ref() {
            self.selected_episode.set(episode);
        }
    }

    fn cache_episode_data(mut self, loaded: &EpisodeLoadResult) {
        let Ok(data) = &loaded.result else {
            return;
        };
        let mut catalog = self.episode_catalog.write();
        let Some(entry) = catalog
            .episodes
            .iter_mut()
            .find(|entry| entry.summary.id() == loaded.episode)
        else {
            return;
        };
        if entry.summary.frames() != loaded.expected_frames
            || data.info().id() != loaded.episode
            || data.info().frames() != loaded.expected_frames
            || entry
                .full_data
                .as_ref()
                .is_some_and(|cached| Arc::ptr_eq(cached, data))
        {
            return;
        }
        entry.full_data = Some(Arc::clone(data));
    }

    pub(crate) fn selected_episode_info(&self) -> Option<EpisodeInfo> {
        let selected = self.selected_episode.read();
        self.episode_catalog
            .read()
            .episodes
            .iter()
            .find(|entry| Some(entry.summary.id()) == selected.as_deref())
            .and_then(|entry| entry.full_data.as_ref())
            .map(|data| data.info().clone())
    }

    pub(crate) fn selected_episode_data(&self) -> Option<EpisodeDataState> {
        let selected = self.selected_episode.read();
        let catalog = self.episode_catalog.read();
        let entry = catalog
            .episodes
            .iter()
            .find(|entry| Some(entry.summary.id()) == selected.as_deref())?;
        if let Some(data) = &entry.full_data {
            return Some(EpisodeDataState::Ready(Arc::clone(data)));
        }
        let expected_frames = entry.summary.frames();
        drop(catalog);

        if self.episode_loader.state().cloned() == UseResourceState::Pending {
            return Some(EpisodeDataState::Loading);
        }
        let loaded = self.episode_loader.read();
        let Some(loaded) = loaded.as_ref().and_then(Option::as_ref) else {
            return Some(EpisodeDataState::Unloaded);
        };
        if loaded.episode != selected.as_deref()? || loaded.expected_frames != expected_frames {
            return Some(EpisodeDataState::Unloaded);
        }
        Some(match &loaded.result {
            Ok(data) => EpisodeDataState::Ready(Arc::clone(data)),
            Err(error) => EpisodeDataState::Failed(error.clone()),
        })
    }

    pub(crate) fn reload_selected_episode_data(mut self, episode: &str) {
        if self.selected_episode.peek().as_deref() != Some(episode) {
            return;
        }
        if let Some(entry) = self
            .episode_catalog
            .write()
            .episodes
            .iter_mut()
            .find(|entry| entry.summary.id() == episode)
        {
            entry.full_data = None;
        }
        self.episode_loader.restart();
    }

    pub(crate) fn datasets(&self) -> Vec<DatasetSummary> {
        self.dataset_catalog.read().datasets().to_vec()
    }

    pub(crate) fn datasets_loading(&self) -> bool {
        self.dataset_catalog.read().loading()
    }

    pub(crate) fn dataset_catalog_error(&self) -> Option<crate::client::CommandError> {
        self.dataset_catalog.read().error().cloned()
    }

    /// Both catalogs are fetched by the session, independent of which activity
    /// is mounted. Mutations and Retry invalidate these same query owners.
    pub(crate) fn use_polling(mut self, studio: Studio) {
        let sessions = crate::shell::project::ProjectSessionSignals::current();
        let datasets = QueryPolicy::new(QueryDomain::Data, 5_000).use_when(
            move || sessions.storage_available(),
            || DatasetClient::current().list_datasets(),
        );
        use_effect(move || {
            let result = datasets.read();
            let pending = *datasets.state().read() == UseResourceState::Pending;
            if pending {
                self.dataset_catalog.write().pending();
            } else if let Some(result) = result.as_ref() {
                self.dataset_catalog.write().accept(result.clone());
            }
        });
        let episodes = QueryPolicy::new(QueryDomain::Data, 5_000).use_when(move || sessions.storage_available(), move || {
            let dataset = studio.selected_dataset().read().to_owned();
            async move {
                let dataset = DatasetId::try_from(dataset.as_str()).map(String::from)
                    .map_err(|error| crate::client::CommandError::new(spiral_robotics_common::command_failure::CommandFailureCode::InvalidInput, error.to_string()))?;
                let result = DatasetClient::current().list_episodes(dataset.clone()).await?;
                Ok::<_, crate::client::CommandError>((dataset, result))
            }
        });
        use_effect(move || {
            let result = episodes.read();
            let pending = *episodes.state().read() == UseResourceState::Pending;
            let dataset = studio.selected_dataset().read().to_owned();
            if self.episode_catalog.peek().dataset_id.as_deref() != Some(&dataset) {
                self.episode_catalog.set(EpisodeCatalogState {
                    dataset_id: Some(dataset.clone()),
                    ..Default::default()
                });
            }
            if pending {
                self.episode_catalog.write().loading = true;
            } else {
                match result.as_ref() {
                    Some(Ok((loaded_dataset, catalog))) if loaded_dataset == &dataset => {
                        self.accept_episodes(
                            catalog.episodes().to_vec(),
                            catalog.warnings().to_vec(),
                        );
                    }
                    Some(Err(error)) => {
                        let mut catalog = self.episode_catalog.write();
                        catalog.loading = false;
                        catalog.error = Some(error.to_string());
                    }
                    _ => {}
                }
            }
        });
    }

    fn accept_episodes(mut self, episodes: Vec<EpisodeListItem>, issues: Vec<String>) {
        let mut catalog = self.episode_catalog.write();
        let changed = !catalog
            .episodes
            .iter()
            .map(|entry| &entry.summary)
            .eq(episodes.iter());
        let previous = std::mem::take(&mut catalog.episodes);
        catalog.episodes = episodes
            .into_iter()
            .map(|summary| {
                previous
                    .iter()
                    .find(|entry| entry.summary.id() == summary.id())
                    .cloned()
                    .map_or_else(
                        || EpisodeEntry::new(summary.clone()),
                        |entry| entry.replace_summary(summary.clone()),
                    )
            })
            .collect();
        catalog.issues = issues;
        catalog.loading = false;
        catalog.loaded = true;
        catalog.error = None;
        drop(catalog);
        if changed && self.selected_episode.peek().is_some() {
            self.episode_loader.restart();
        }
    }

    pub(crate) fn retry_datasets(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Data]);
    }

    pub(crate) fn invalidate_episode_catalog(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Data]);
    }

    fn sync_dataset(mut self, dataset_id: &str) {
        let previous = self.draft.generated_sql.peek().to_owned();
        let next = NativeDatasetSelection::new(dataset_id.split('.').map(str::to_owned).collect())
            .map(|selection| selection.episodes_sql())
            .unwrap_or_default();
        if previous == next {
            return;
        }
        let mut current = self.draft.sql.peek().to_owned();
        if current.rebase_generated(&previous, &next) {
            self.draft.sql.set(current);
        }
        self.draft.generated_sql.set(next);
    }

    pub(crate) fn sql(&self) -> String {
        self.draft.sql.read().to_owned()
    }

    pub(crate) fn max_rows(&self) -> usize {
        self.draft.max_rows.read().to_owned()
    }

    pub(crate) fn query_tabs(&self) -> Vec<QueryTab> {
        self.execution.query_tabs.read().to_owned()
    }

    pub(crate) fn active_query(&self) -> Option<u64> {
        self.execution.active_query.read().to_owned()
    }

    pub(crate) fn query_running(&self) -> bool {
        self.execution.active_request.read().is_some()
    }

    pub(crate) fn select_query(mut self, id: u64) {
        if self
            .execution
            .query_tabs
            .peek()
            .iter()
            .any(|tab| tab.id() == id)
        {
            self.execution.active_query.set(Some(id));
        }
    }

    pub(crate) fn close_query(mut self, id: u64) {
        if self
            .execution
            .active_request
            .peek()
            .as_ref()
            .is_some_and(|request| request.id == id)
            && let Some(request) = self.execution.active_request.replace(None)
        {
            let _ = request;
        }
        let mut tabs = self.execution.query_tabs.peek().to_owned();
        let mut active = self.execution.active_query.peek().to_owned();
        if !tabs.close_query(&mut active, id) {
            return;
        }
        self.execution.query_tabs.set(tabs);
        self.execution.active_query.set(active);
    }

    pub(crate) fn set_sql(mut self, sql: String) {
        self.draft.sql.set(sql);
    }

    pub(crate) fn set_max_rows(mut self, rows: usize) {
        self.draft.max_rows.set(rows);
    }

    pub(crate) fn run_query(mut self, studio: Studio, query_available: bool) {
        if !query_available {
            studio.report_warning_from(
                MessageSource::Data,
                "Configure a Spiral catalog, project, and namespace in Settings before running Query.",
            );
            return;
        }
        let statement = self.draft.sql.read().trim().to_owned();
        if statement.is_empty() {
            return;
        }
        if self.execution.active_request.peek().is_some() {
            studio.report_warning_from(
                MessageSource::Data,
                "A query is already running. Cancel it before starting another.",
            );
            return;
        }

        let rows = (self.draft.max_rows)();
        let id = self.execution.next_query_id.peek().to_owned();
        self.execution.next_query_id.set(id.wrapping_add(1));
        let title = statement.query_title(id);
        let running = QueryTab::new(
            id,
            title,
            QueryExecutionPresentation::new(statement.clone(), rows, QueryState::Running),
        );
        let mut tabs = self.execution.query_tabs.peek().to_owned();
        tabs.push_bounded(running);
        self.execution.query_tabs.set(tabs);
        self.execution.active_query.set(Some(id));

        self.execution.active_request.set(Some(ActiveQuery { id }));

        // The Data view may be unmounted while the request is in flight. The
        // controller and its generation guard are shell-owned, so completion
        // must outlive the button that launched it.
        crate::client::project::ProjectClient::current().spawn(async move {
            let outcome = DatasetClient::current()
                .run_spiral_query(statement.clone(), rows)
                .await
                .map_err(|error| error.to_string());
            if self
                .execution
                .active_request
                .peek()
                .as_ref()
                .is_none_or(|request| request.id != id)
            {
                return;
            }
            let state = match outcome {
                Ok(response) => QueryState::Ready(response),
                Err(error) => {
                    studio.report_from(MessageSource::Data, format!("Query failed: {error}"));
                    QueryState::Failed(error)
                }
            };
            let mut tabs = self.execution.query_tabs.peek().to_owned();
            if let Some(tab) = tabs.iter_mut().find(|tab| tab.id() == id) {
                tab.replace_presentation(QueryExecutionPresentation::new(statement, rows, state));
                self.execution.query_tabs.set(tabs);
            }
            self.execution.active_request.set(None);
        });
    }

    pub(crate) fn cancel_query(mut self) {
        let Some(request) = self.execution.active_request.replace(None) else {
            return;
        };
        let mut tabs = self.execution.query_tabs.peek().to_owned();
        if let Some(tab) = tabs.iter_mut().find(|tab| tab.id() == request.id) {
            tab.mark_cancelled();
            self.execution.query_tabs.set(tabs);
        }
    }
}

trait QueryTabs {
    fn close_query(&mut self, active: &mut Option<u64>, id: u64) -> bool;
    fn push_bounded(&mut self, tab: QueryTab);
}

impl QueryTabs for Vec<QueryTab> {
    fn close_query(&mut self, active: &mut Option<u64>, id: u64) -> bool {
        let Some(index) = self.iter().position(|tab| tab.id() == id) else {
            return false;
        };
        self.remove(index);
        if *active == Some(id) {
            *active = index
                .checked_sub(1)
                .and_then(|index| self.get(index))
                .or_else(|| self.get(index))
                .map(QueryTab::id);
        }
        true
    }

    fn push_bounded(&mut self, tab: QueryTab) {
        if self.len() >= MAX_QUERY_TABS {
            self.remove(0);
        }
        self.push(tab);
    }
}

trait QuerySqlPresentation {
    fn query_title(&self, id: u64) -> String;
}

impl QuerySqlPresentation for str {
    fn query_title(&self, id: u64) -> String {
        let normalized = self.split_whitespace().collect::<Vec<_>>();
        let table = normalized
            .windows(2)
            .find(|tokens| tokens[0].eq_ignore_ascii_case("from"))
            .map(|tokens| {
                tokens[1]
                    .split('(')
                    .next()
                    .unwrap_or_default()
                    .trim_matches(|character: char| {
                        !character.is_alphanumeric() && character != '_' && character != '.'
                    })
            })
            .and_then(|relation| relation.rsplit('.').next())
            .filter(|relation| !relation.is_empty());
        table
            .map(|table| format!("{table} · {id}"))
            .unwrap_or_else(|| format!("Query {id}"))
    }
}

#[cfg(test)]
mod tests {
    use crate::activities::data::query_result::{QueryExecutionPresentation, QueryState, QueryTab};

    use super::{MAX_QUERY_TABS, QuerySqlPresentation, QueryTabs};

    #[test]
    fn query_tabs_name_the_first_relation_without_hiding_identity() {
        assert_eq!(
            "SELECT * FROM robotics.episodes".query_title(3),
            "episodes · 3"
        );
        assert_eq!("EXPLAIN SELECT 1".query_title(4), "Query 4");
    }

    #[test]
    fn closing_the_active_result_selects_a_neighbor_without_touching_others() {
        let tab = |id| {
            QueryTab::new(
                id,
                format!("Query {id}"),
                QueryExecutionPresentation::new(format!("SELECT {id}"), 100, QueryState::Running),
            )
        };
        let mut tabs = vec![tab(1), tab(2), tab(3)];
        let mut active = Some(2);

        assert!(tabs.close_query(&mut active, 2));
        assert_eq!(active, Some(1));
        assert_eq!(tabs.iter().map(QueryTab::id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(tabs[1].presentation().sql(), "SELECT 3");
    }

    #[test]
    fn retained_query_results_are_bounded_to_the_newest_tabs() {
        let tab = |id| {
            QueryTab::new(
                id,
                format!("Query {id}"),
                QueryExecutionPresentation::new(format!("SELECT {id}"), 100, QueryState::Running),
            )
        };
        let mut tabs = (1..=MAX_QUERY_TABS as u64).map(tab).collect::<Vec<_>>();
        tabs.push_bounded(tab(99));
        assert_eq!(tabs.len(), MAX_QUERY_TABS);
        assert_eq!(tabs.first().map(QueryTab::id), Some(2));
        assert_eq!(tabs.last().map(QueryTab::id), Some(99));
    }
}
