//! Durable state and transitions for the Data screen.
//!
//! The controller outlives the mounted Data workbench. Views render its
//! signals and emit semantic transitions; async query completion is accepted
//! only by the request generation that started it.

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_core::api::{EpisodeSummary, PublicationStatus, SpiralQueryRequest};

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
    publications: Signal<Vec<PublicationStatus>>,
    publication_error: Signal<Option<String>>,
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
        publications: use_signal(Vec::new),
        publication_error: use_signal(|| None),
    };
    let dataset_dependency = dataset.to_owned();
    use_effect(use_reactive((&dataset_dependency,), move |(dataset,)| {
        controller.sync_dataset(&dataset)
    }));
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

    pub(crate) fn tab(&self) -> DataWorkbenchTab {
        self.tab.read().to_owned()
    }

    pub(crate) fn sql(&self) -> String {
        self.sql.read().clone()
    }

    pub(crate) fn max_rows(&self) -> usize {
        self.max_rows.read().to_owned()
    }

    pub(crate) fn query(&self) -> QueryViewModel {
        self.query.read().clone()
    }

    pub(crate) fn intent(&self) -> Option<DataIntent> {
        self.intent.read().clone()
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
            ..PublicationStatus::default()
        });
        dioxus::dioxus_core::spawn_forever(async move {
            let path = format!("/api/spiral/publications/{episode_id}");
            match api::post(&path).await {
                Ok(()) => studio.report_success(format!("Pushing {episode_id} to SpiralDB.")),
                Err(error) => {
                    self.upsert_publication(PublicationStatus {
                        episode_id: episode_id.clone(),
                        state: "failed".to_owned(),
                        spiral: Some(format!("failed: {error}")),
                        updated_ms: 0,
                        ..PublicationStatus::default()
                    });
                    studio.report(format!("Could not push {episode_id} to SpiralDB: {error}"));
                }
            }
        });
    }

    pub(crate) fn present_episode(
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

    pub(crate) fn present_empty(mut self, on_intent: EventHandler<DataIntent>) {
        self.publish(DataIntent::EmptyEpisodes, on_intent);
    }

    pub(crate) fn select_tab(
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

    pub(crate) fn set_sql(mut self, sql: String, on_intent: EventHandler<DataIntent>) {
        self.invalidate_query();
        self.sql.set(sql.clone());
        let model = QueryViewModel::idle(sql, (self.max_rows)());
        self.query.set(model.clone());
        self.publish(DataIntent::Query(model), on_intent);
    }

    pub(crate) fn set_max_rows(mut self, rows: usize, on_intent: EventHandler<DataIntent>) {
        self.invalidate_query();
        self.max_rows.set(rows);
        let model = QueryViewModel::idle((self.sql)(), rows);
        self.query.set(model.clone());
        self.publish(DataIntent::Query(model), on_intent);
    }

    pub(crate) fn run_query(
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
