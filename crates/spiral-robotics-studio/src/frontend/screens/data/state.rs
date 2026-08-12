//! Durable state and transitions for the Data screen.
//!
//! The controller outlives the mounted Data workbench. Views render its
//! signals and emit semantic transitions; async query completion is accepted
//! only by the request generation that started it.

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_core::api::{PublicationStatus, SpiralQueryRequest};

use crate::frontend::api::http as api;
use crate::frontend::app::notifications::MessageSource;
use crate::frontend::app::state::Studio;
use crate::frontend::screens::data::workbench::{
    DEFAULT_QUERY_ROWS, QueryState, QueryTab, QueryViewModel, default_query_sql,
};
use crate::frontend::screens::sql_draft::rebase_generated_sql;

#[derive(Clone, Copy)]
pub(crate) struct DataController {
    sql: Signal<String>,
    generated_sql: Signal<String>,
    max_rows: Signal<usize>,
    query_tabs: Signal<Vec<QueryTab>>,
    active_query: Signal<Option<u64>>,
    next_query_id: Signal<u64>,
    publications: Signal<Vec<PublicationStatus>>,
    publication_error: Signal<Option<String>>,
}

/// Create the Data controller once at the shell boundary and publish it to the
/// activity view. None of this state depends on the workbench being mounted.
pub(crate) fn use_data_controller(namespace: &str, dataset_id: &str) -> DataController {
    let initial_sql = default_query_sql(namespace, dataset_id);
    let sql_value = initial_sql.clone();
    let controller = DataController {
        sql: use_signal(move || sql_value),
        generated_sql: use_signal(move || initial_sql),
        max_rows: use_signal(|| DEFAULT_QUERY_ROWS),
        query_tabs: use_signal(Vec::new),
        active_query: use_signal(|| None),
        next_query_id: use_signal(|| 1),
        publications: use_signal(Vec::new),
        publication_error: use_signal(|| None),
    };
    let namespace_dependency = namespace.to_owned();
    let dataset_dependency = dataset_id.to_owned();
    use_effect(use_reactive(
        (&namespace_dependency, &dataset_dependency),
        move |(namespace, dataset_id)| controller.sync_dataset(&namespace, &dataset_id),
    ));
    #[cfg(all(feature = "web", target_arch = "wasm32"))]
    {
        let mut publication_controller = controller;
        use_future(move || async move {
            loop {
                publication_controller.refresh_publications().await;
                gloo_timers::future::TimeoutFuture::new(1_000).await;
            }
        });
    }
    use_context_provider(|| controller)
}

pub(crate) fn data_controller() -> DataController {
    use_context::<DataController>()
}

impl DataController {
    fn sync_dataset(mut self, namespace: &str, dataset_id: &str) {
        let previous = self.generated_sql.peek().clone();
        let next = default_query_sql(namespace, dataset_id);
        if previous == next {
            return;
        }
        let mut current = self.sql.peek().clone();
        if rebase_generated_sql(&mut current, &previous, &next) {
            self.sql.set(current);
        }
        self.generated_sql.set(next);
    }

    pub(crate) fn sql(&self) -> String {
        self.sql.read().clone()
    }

    pub(crate) fn max_rows(&self) -> usize {
        self.max_rows.read().to_owned()
    }

    pub(crate) fn query_tabs(&self) -> Vec<QueryTab> {
        self.query_tabs.read().clone()
    }

    pub(crate) fn active_query(&self) -> Option<u64> {
        self.active_query.read().to_owned()
    }

    pub(crate) fn select_query(mut self, id: u64) {
        if self.query_tabs.peek().iter().any(|tab| tab.id == id) {
            self.active_query.set(Some(id));
        }
    }

    pub(crate) fn close_query(mut self, id: u64) {
        let mut tabs = self.query_tabs.peek().clone();
        let mut active = self.active_query.peek().to_owned();
        if !close_query_tab(&mut tabs, &mut active, id) {
            return;
        }
        self.query_tabs.set(tabs);
        self.active_query.set(active);
    }

    pub(crate) fn publications(&self) -> Vec<PublicationStatus> {
        self.publications.read().clone()
    }

    pub(crate) fn publication_error(&self) -> Option<String> {
        self.publication_error.read().clone()
    }

    pub(crate) fn push_to_spiral(mut self, studio: Studio, episode_id: String) {
        self.upsert_publication(PublicationStatus {
            episode_id: episode_id.clone(),
            state: "queued".to_owned(),
            spiral: Some("queued".to_owned()),
            updated_ms: 0,
        });
        dioxus::dioxus_core::spawn_forever(async move {
            let path = format!("/api/spiral/publications/{episode_id}");
            match api::post(&path).await {
                Ok(()) => studio.report_success_from(
                    MessageSource::Data,
                    format!("Pushing {episode_id} to SpiralDB."),
                ),
                Err(error) => {
                    self.upsert_publication(PublicationStatus {
                        episode_id: episode_id.clone(),
                        state: "failed".to_owned(),
                        spiral: Some(format!("failed: {error}")),
                        updated_ms: 0,
                    });
                    studio.report_from(
                        MessageSource::Data,
                        format!("Could not push {episode_id} to SpiralDB: {error}"),
                    );
                }
            }
        });
    }

    pub(crate) fn set_sql(mut self, sql: String) {
        self.sql.set(sql);
    }

    pub(crate) fn set_max_rows(mut self, rows: usize) {
        self.max_rows.set(rows);
    }

    pub(crate) fn run_query(mut self, studio: Studio, query_available: bool) {
        if !query_available {
            studio.report_warning_from(
                MessageSource::Data,
                "Configure a Spiral catalog, project, and namespace in Settings before running Query.",
            );
            return;
        }
        let statement = self.sql.read().trim().to_owned();
        if statement.is_empty() {
            return;
        }

        let rows = (self.max_rows)();
        let id = self.next_query_id.peek().to_owned();
        self.next_query_id.set(id.wrapping_add(1));
        let title = query_title(&statement, id);
        let running = QueryTab {
            id,
            title,
            model: QueryViewModel {
                sql: statement.clone(),
                max_rows: rows,
                state: QueryState::Running,
            },
        };
        let mut tabs = self.query_tabs.peek().clone();
        tabs.push(running);
        self.query_tabs.set(tabs);
        self.active_query.set(Some(id));

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
                    Err(error) => {
                        studio.report_from(MessageSource::Data, format!("Query failed: {error}"));
                        QueryState::Failed(error)
                    }
                };
            let mut tabs = self.query_tabs.peek().clone();
            if let Some(tab) = tabs.iter_mut().find(|tab| tab.id == id) {
                tab.model = QueryViewModel {
                    sql: statement,
                    max_rows: rows,
                    state,
                };
                self.query_tabs.set(tabs);
            }
        });
    }

    #[cfg(all(feature = "web", target_arch = "wasm32"))]
    async fn refresh_publications(&mut self) {
        match api::get::<Vec<PublicationStatus>>("/api/publications").await {
            Ok(publications) => {
                self.publications.set(publications);
                self.publication_error.set(None);
            }
            Err(error) => self.publication_error.set(Some(error)),
        }
    }

    fn upsert_publication(&mut self, status: PublicationStatus) {
        let mut publications = self.publications.peek().clone();
        if let Some(existing) = publications
            .iter_mut()
            .find(|existing| existing.episode_id == status.episode_id)
        {
            *existing = status;
        } else {
            publications.push(status);
        }
        self.publications.set(publications);
    }
}

fn close_query_tab(tabs: &mut Vec<QueryTab>, active: &mut Option<u64>, id: u64) -> bool {
    let Some(index) = tabs.iter().position(|tab| tab.id == id) else {
        return false;
    };
    tabs.remove(index);
    if *active == Some(id) {
        *active = index
            .checked_sub(1)
            .and_then(|index| tabs.get(index))
            .or_else(|| tabs.get(index))
            .map(|tab| tab.id);
    }
    true
}

fn query_title(sql: &str, id: u64) -> String {
    let normalized = sql.split_whitespace().collect::<Vec<_>>();
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

#[cfg(test)]
mod tests {
    use crate::frontend::screens::data::workbench::{QueryState, QueryTab, QueryViewModel};

    use super::{close_query_tab, query_title};

    #[test]
    fn query_tabs_name_the_first_relation_without_hiding_identity() {
        assert_eq!(
            query_title("SELECT * FROM robotics.episodes", 3),
            "episodes · 3"
        );
        assert_eq!(query_title("EXPLAIN SELECT 1", 4), "Query 4");
    }

    #[test]
    fn closing_the_active_result_selects_a_neighbor_without_touching_others() {
        let tab = |id| QueryTab {
            id,
            title: format!("Query {id}"),
            model: QueryViewModel {
                sql: format!("SELECT {id}"),
                max_rows: 100,
                state: QueryState::Running,
            },
        };
        let mut tabs = vec![tab(1), tab(2), tab(3)];
        let mut active = Some(2);

        assert!(close_query_tab(&mut tabs, &mut active, 2));
        assert_eq!(active, Some(1));
        assert_eq!(tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(tabs[1].model.sql, "SELECT 3");
    }
}
