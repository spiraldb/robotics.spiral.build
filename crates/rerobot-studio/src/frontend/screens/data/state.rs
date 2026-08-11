//! Durable state and transitions for the Data screen.
//!
//! The controller outlives the mounted Data workbench. Views render its
//! signals and emit semantic transitions; async query completion is accepted
//! only by the request generation that started it.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{EpisodeSummary, SpiralQueryRequest};

use crate::frontend::api::http as api;
use crate::frontend::app::state::Studio;
use crate::frontend::screens::data::workbench::{
    DEFAULT_QUERY_ROWS, DataIntent, DataWorkbenchTab, QueryState, QueryViewModel, default_query_sql,
};
use crate::frontend::screens::sql_draft::rebase_generated_sql;

#[derive(Clone, Copy)]
pub(crate) struct DataController {
    tab: Signal<DataWorkbenchTab>,
    sql: Signal<String>,
    generated_sql: Signal<String>,
    max_rows: Signal<usize>,
    query: Signal<QueryViewModel>,
    intent: Signal<Option<DataIntent>>,
    query_generation: Signal<u64>,
}

/// Create the Data controller once at the shell boundary and publish it to the
/// activity view. None of this state depends on the workbench being mounted.
pub(crate) fn use_data_controller(dataset: &str) -> DataController {
    let initial_sql = default_query_sql(dataset);
    let sql_value = initial_sql.clone();
    let query_value = initial_sql.clone();
    let controller = DataController {
        tab: use_signal(DataWorkbenchTab::default),
        sql: use_signal(move || sql_value),
        generated_sql: use_signal(move || initial_sql),
        max_rows: use_signal(|| DEFAULT_QUERY_ROWS),
        query: use_signal(move || QueryViewModel::idle(query_value, DEFAULT_QUERY_ROWS)),
        intent: use_signal(|| None),
        query_generation: use_signal(|| 0),
    };
    let dataset_dependency = dataset.to_owned();
    use_effect(use_reactive((&dataset_dependency,), move |(dataset,)| {
        controller.sync_dataset(&dataset)
    }));
    use_context_provider(|| controller)
}

pub(crate) fn data_controller() -> DataController {
    use_context::<DataController>()
}

impl DataController {
    fn sync_dataset(mut self, dataset: &str) {
        let previous = self.generated_sql.peek().clone();
        let next = default_query_sql(dataset);
        if previous == next {
            return;
        }
        let mut current = self.sql.peek().clone();
        if rebase_generated_sql(&mut current, &previous, &next) {
            self.invalidate_query();
            self.sql.set(current.clone());
            self.query.set(QueryViewModel::idle(
                current,
                self.max_rows.peek().to_owned(),
            ));
        }
        self.generated_sql.set(next);
    }

    pub fn tab(&self) -> DataWorkbenchTab {
        self.tab.read().to_owned()
    }

    pub fn sql(&self) -> String {
        self.sql.read().clone()
    }

    pub fn max_rows(&self) -> usize {
        self.max_rows.read().to_owned()
    }

    pub fn query(&self) -> QueryViewModel {
        self.query.read().clone()
    }

    pub fn intent(&self) -> Option<DataIntent> {
        self.intent.read().clone()
    }

    pub fn present_episode(
        mut self,
        mut studio: Studio,
        episode: EpisodeSummary,
        on_intent: EventHandler<DataIntent>,
    ) {
        if studio.selected_episode.peek().as_deref() != Some(episode.id.as_str()) {
            studio.selected_episode.set(Some(episode.id.clone()));
        }
        self.publish(DataIntent::Episode(episode), on_intent);
    }

    pub fn present_empty(mut self, on_intent: EventHandler<DataIntent>) {
        self.publish(DataIntent::EmptyEpisodes, on_intent);
    }

    pub fn select_tab(
        mut self,
        studio: Studio,
        tab: DataWorkbenchTab,
        on_intent: EventHandler<DataIntent>,
    ) {
        self.tab.set(tab);
        match tab {
            DataWorkbenchTab::Episodes => match studio.focused_episode() {
                Some(episode) => self.present_episode(studio, episode, on_intent),
                None => self.present_empty(on_intent),
            },
            DataWorkbenchTab::Query => self.publish(DataIntent::Query((self.query)()), on_intent),
        }
    }

    pub fn set_sql(mut self, sql: String, on_intent: EventHandler<DataIntent>) {
        self.invalidate_query();
        self.sql.set(sql.clone());
        let model = QueryViewModel::idle(sql, (self.max_rows)());
        self.query.set(model.clone());
        self.publish(DataIntent::Query(model), on_intent);
    }

    pub fn set_max_rows(mut self, rows: usize, on_intent: EventHandler<DataIntent>) {
        self.invalidate_query();
        self.max_rows.set(rows);
        let model = QueryViewModel::idle((self.sql)(), rows);
        self.query.set(model.clone());
        self.publish(DataIntent::Query(model), on_intent);
    }

    pub fn run_query(
        mut self,
        studio: Studio,
        query_available: bool,
        on_intent: EventHandler<DataIntent>,
    ) {
        if !query_available {
            studio.report(
                "Configure a Spiral catalog, project, and dataset in Settings before running Query.",
            );
            return;
        }
        let statement = self.sql.read().trim().to_owned();
        if statement.is_empty() || matches!(self.query.read().state, QueryState::Running) {
            return;
        }

        let rows = (self.max_rows)();
        let generation = self.invalidate_query();
        let running = QueryViewModel {
            sql: statement.clone(),
            max_rows: rows,
            state: QueryState::Running,
        };
        self.query.set(running.clone());
        self.publish(DataIntent::Query(running), on_intent);

        let request = SpiralQueryRequest {
            sql: statement.clone(),
            max_rows: rows,
        };
        // The Data view may be unmounted while the request is in flight. The
        // controller and its generation guard are shell-owned, so completion
        // must outlive the button that launched it.
        dioxus::dioxus_core::spawn_forever(async move {
            let state =
                match api::send_json(Method::POST, "/api/spiral/query", Some(&request)).await {
                    Ok(response) => QueryState::Ready(response),
                    Err(error) => QueryState::Failed(error),
                };
            if self.query_generation.peek().to_owned() != generation {
                return;
            }
            let completed = QueryViewModel {
                sql: statement,
                max_rows: rows,
                state,
            };
            self.query.set(completed.clone());
            if self.tab.peek().to_owned() == DataWorkbenchTab::Query {
                // The launching workbench may have been unmounted while this
                // shell-owned request was running. Commit the retained Data
                // projection directly; cross-feature callbacks are reserved
                // for synchronous episode changes from a mounted view.
                let completed = DataIntent::Query(completed);
                if self.intent.peek().as_ref() != Some(&completed) {
                    self.intent.set(Some(completed));
                }
            }
        });
    }

    fn publish(&mut self, intent: DataIntent, on_intent: EventHandler<DataIntent>) {
        // Cross-feature consequences inspect the previous intent to decide
        // whether Timeline media must be reset. Publish the semantic action
        // first, then commit the Data-owned projection.
        on_intent.call(intent.clone());
        if self.intent.peek().as_ref() != Some(&intent) {
            self.intent.set(Some(intent));
        }
    }

    fn invalidate_query(&mut self) -> u64 {
        let next = self.query_generation.peek().wrapping_add(1);
        self.query_generation.set(next);
        next
    }
}
