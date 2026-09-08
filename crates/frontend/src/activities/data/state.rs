//! SQL drafts and query results owned by the Data workbench.
use crate::activities::data::DatasetClient;
use crate::activities::data::query_result::{
    DEFAULT_QUERY_ROWS, QueryExecutionPresentation, QueryState, QueryTab,
};
use crate::shell::{notifications::MessageSource, state::StudioProjectState};
use dioxus::prelude::*;
pub(crate) const MAX_QUERY_TABS: usize = 8;
struct ActiveQuery {
    id: u64,
}
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct QueryController {
    draft: QueryDraftSignals,
    execution: QueryExecutionSignals,
}
#[derive(Clone, Copy, PartialEq)]
struct QueryDraftSignals {
    sql: Signal<String>,
    max_rows: Signal<usize>,
}

#[derive(Clone, Copy, PartialEq)]
struct QueryExecutionSignals {
    query_tabs: Signal<Vec<QueryTab>>,
    active_query: Signal<Option<u64>>,
    active_request: Signal<Option<ActiveQuery>>,
    next_query_id: Signal<u64>,
}

impl QueryController {
    pub(crate) fn use_controller(initial_sql: String) -> Self {
        use_hook(|| Self {
            draft: QueryDraftSignals {
                sql: Signal::new(initial_sql),
                max_rows: Signal::new(DEFAULT_QUERY_ROWS),
            },
            execution: QueryExecutionSignals {
                query_tabs: Signal::new(Vec::new()),
                active_query: Signal::new(None),
                active_request: Signal::new(None),
                next_query_id: Signal::new(1),
            },
        })
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

    pub(crate) fn run_query(mut self, studio: StudioProjectState, query_available: bool) {
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

        // The request may outlive the initiating panel; the Data workspace owns its result.
        dioxus::dioxus_core::Runtime::current().spawn(self.draft.sql.origin_scope(), async move {
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn query_tabs_name_the_first_relation_without_hiding_identity() {
        assert_eq!(
            "SELECT * FROM robotics.episodes".query_title(3),
            "episodes · 3"
        );
        assert_eq!("EXPLAIN SELECT 1".query_title(4), "Query 4");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
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

    #[wasm_bindgen_test::wasm_bindgen_test]
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

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct OwnershipHarness(Rc<RefCell<ObservedOwners>>);
    #[derive(Default)]
    struct ObservedOwners {
        mounted: Option<Signal<bool>>,
        controllers: Vec<QueryController>,
    }
    impl OwnershipHarness {
        fn render(self) -> Element {
            use_context_provider(|| self.clone());
            let mounted = use_signal(|| true);
            self.0.borrow_mut().mounted = Some(mounted);
            rsx! { if mounted() { QueryOwner {} QueryOwner {} } }
        }
    }
    #[component]
    fn QueryOwner() -> Element {
        let controller = QueryController::use_controller("SELECT 1".into());
        let harness = use_context::<OwnershipHarness>();
        use_hook(move || harness.0.borrow_mut().controllers.push(controller));
        rsx! { span { "{controller.sql()}" } }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn query_state_is_isolated_and_dropped_with_its_component_owner() {
        let harness = OwnershipHarness::default();
        let mut dom = VirtualDom::new_with_props(OwnershipHarness::render, harness.clone());
        dom.rebuild_in_place();
        let (first, second, mut mounted) = {
            let observed = harness.0.borrow();
            (
                observed.controllers[0],
                observed.controllers[1],
                observed.mounted.unwrap(),
            )
        };
        assert_ne!(
            first.draft.sql.origin_scope(),
            second.draft.sql.origin_scope()
        );
        assert_ne!(first.draft.sql.origin_scope(), ScopeId::ROOT);
        dom.in_runtime(|| first.set_sql("select 'local draft'".into()));
        dom.render_immediate_to_vec();
        assert_ne!(first.sql(), second.sql());
        dom.in_runtime(|| mounted.set(false));
        dom.render_immediate_to_vec();
        assert!(first.draft.sql.try_read().is_err());
        assert!(second.draft.sql.try_read().is_err());
        dom.in_runtime(|| mounted.set(true));
        dom.render_immediate_to_vec();
        let replacement = harness.0.borrow().controllers[2];
        assert_ne!(replacement.sql(), "select 'local draft'");
    }
}
