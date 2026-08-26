//! Durable state and transitions for the Data screen.
//!
//! The controller outlives the mounted Data workbench. Views render its
//! signals and emit semantic transitions; async query completion is accepted
//! only by the request generation that started it.

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_api::catalog::QueryPreset;
use spiral_robotics_api::relations::SpiralQueryRequest;

use crate::activities::data::query_result::{
    DEFAULT_QUERY_ROWS, QueryExecutionPresentation, QueryState, QueryTab,
};
use crate::activities::data::sql_draft::GeneratedSqlDraft;
use crate::activities::data::workbench::QueryPresetSql;
use crate::client::http as api;
use crate::shell::notifications::MessageSource;
use crate::shell::state::Studio;

pub(crate) const MAX_QUERY_TABS: usize = 8;

struct ActiveQuery {
    id: u64,
    #[cfg(target_arch = "wasm32")]
    abort: Option<web_sys::AbortController>,
}

impl ActiveQuery {
    fn abort(&self) {
        #[cfg(target_arch = "wasm32")]
        if let Some(abort) = &self.abort {
            abort.abort();
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DataController {
    selected_episode: Signal<Option<String>>,
    draft: QueryDraftSignals,
    execution: QueryExecutionSignals,
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
    relation_generation: Signal<u64>,
}

/// Create the Data controller once at the shell boundary and publish it to the
/// activity view. None of this state depends on the workbench being mounted.
pub(crate) fn use_data_controller(namespace: &str, dataset_id: &str) -> DataController {
    let initial_sql = QueryPreset::Episodes.scoped_sql(namespace, dataset_id);
    let sql_value = initial_sql.clone();
    let controller = DataController {
        selected_episode: use_signal(|| None),
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
            relation_generation: use_signal(|| 0),
        },
    };
    let namespace_dependency = namespace.to_owned();
    let dataset_dependency = dataset_id.to_owned();
    use_effect(use_reactive(
        (&namespace_dependency, &dataset_dependency),
        move |(namespace, dataset_id)| controller.sync_dataset(&namespace, &dataset_id),
    ));
    use_context_provider(|| controller)
}

pub(crate) fn data_controller() -> DataController {
    use_context::<DataController>()
}

impl DataController {
    pub(crate) fn selected_episode(&self) -> Option<String> {
        self.selected_episode.read().to_owned()
    }

    pub(crate) fn selected_episode_peek(&self) -> Option<String> {
        self.selected_episode.peek().to_owned()
    }

    pub(crate) fn set_selected_episode(mut self, episode: Option<String>) {
        self.selected_episode.set(episode);
    }

    fn sync_dataset(mut self, namespace: &str, dataset_id: &str) {
        let previous = self.draft.generated_sql.peek().to_owned();
        let next = QueryPreset::Episodes.scoped_sql(namespace, dataset_id);
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

    pub(crate) fn relation_generation(&self) -> u64 {
        *self.execution.relation_generation.read()
    }

    pub(crate) fn refresh_relations(mut self) {
        let next = self.execution.relation_generation.peek().wrapping_add(1);
        self.execution.relation_generation.set(next);
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
            request.abort();
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

        #[cfg(target_arch = "wasm32")]
        let abort = web_sys::AbortController::new().ok();
        #[cfg(target_arch = "wasm32")]
        let signal = abort.as_ref().map(web_sys::AbortController::signal);
        #[cfg(not(target_arch = "wasm32"))]
        let signal = None::<web_sys::AbortSignal>;
        self.execution.active_request.set(Some(ActiveQuery {
            id,
            #[cfg(target_arch = "wasm32")]
            abort,
        }));

        let request = SpiralQueryRequest::new(statement.clone(), rows);
        // The Data view may be unmounted while the request is in flight. The
        // controller and its generation guard are shell-owned, so completion
        // must outlive the button that launched it.
        dioxus::dioxus_core::spawn_forever(async move {
            let outcome =
                api::AbortableJsonRequest::new(Method::POST, "/api/spiral/query", Some(&request))
                    .with_signal(signal.as_ref())
                    .send()
                    .await;
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
        request.abort();
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
                tokens[1].trim_matches(|character: char| {
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
