//! User-facing project relation map and bounded row preview.

use crate::activities::data::DatasetClient;
use std::collections::{BTreeMap, BTreeSet};

use crate::components::interaction::{Action, ActionPurpose, ActionVariant, RefreshIcon};
use dioxus::prelude::*;
use spiral_robotics_common::relations::{CatalogRelation, RelationCatalog, SpiralQuerySnapshot};

use crate::activities::data::catalog::ManagedTable as Table;
use crate::activities::data::query_result::{QueryExecutionPresentation, QueryState};

const PREVIEW_ROWS: usize = 5;
const MAX_COUNTED_RELATIONS: usize = 1_000;
const MAX_COUNT_QUERY_BYTES: usize = 60 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
struct RelationLink {
    left: String,
    right: String,
    keys: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct ProjectRelations {
    relations: Vec<CatalogRelation>,
    complete: bool,
    limitation: Option<String>,
    countable_relations: usize,
}

struct ProjectRelationScope<'a> {
    namespace: &'a str,
}

impl ProjectRelations {
    fn from_catalog(catalog: &RelationCatalog, namespace: &str) -> Self {
        let scope = ProjectRelationScope { namespace };
        let relations = catalog
            .relations()
            .iter()
            .filter(|relation| scope.includes(relation))
            .cloned()
            .collect::<Vec<_>>();
        let countable_relations = Self::countable_relation_count(&relations);
        let count_limitation = (relations.len() > countable_relations).then(|| {
            format!(
                "Exact counts are limited to the first {countable_relations} visible relations by the bounded query contract."
            )
        });
        let limitation = match (catalog.limitation(), count_limitation) {
            (Some(catalog_limitation), Some(count_limitation)) => {
                Some(format!("{catalog_limitation} {count_limitation}"))
            }
            (Some(catalog_limitation), None) => Some(catalog_limitation.to_owned()),
            (None, count_limitation) => count_limitation,
        };
        Self {
            relations,
            complete: catalog.complete(),
            limitation,
            countable_relations,
        }
    }

    fn count_query(&self) -> Option<(String, usize)> {
        let statements = self
            .relations
            .iter()
            .take(self.countable_relations)
            .enumerate()
            .map(|(index, relation)| Self::count_statement(index, relation))
            .collect::<Vec<_>>();
        (!statements.is_empty())
            .then(|| (statements.join("\nUNION ALL\n"), self.countable_relations))
    }

    fn countable_relation_count(relations: &[CatalogRelation]) -> usize {
        let mut query_bytes = 0_usize;
        let mut relation_count = 0_usize;
        for (index, relation) in relations.iter().enumerate() {
            if relation_count == MAX_COUNTED_RELATIONS {
                break;
            }
            let separator_bytes = if index > 0 { "\nUNION ALL\n".len() } else { 0 };
            let statement_bytes = Self::count_statement(index, relation).len();
            if query_bytes + separator_bytes + statement_bytes > MAX_COUNT_QUERY_BYTES {
                break;
            }
            query_bytes += separator_bytes + statement_bytes;
            relation_count += 1;
        }
        relation_count
    }

    fn count_statement(index: usize, relation: &CatalogRelation) -> String {
        format!(
            "SELECT {index} AS relation_index, COUNT(*) AS row_count FROM {}",
            relation.name()
        )
    }

    fn with_counts(mut self, response: &SpiralQuerySnapshot) -> Self {
        let counts = response
            .rows()
            .iter()
            .filter_map(|row| {
                let index = row
                    .get("relation_index")
                    .and_then(|value| {
                        value
                            .as_u64()
                            .or_else(|| value.as_i64().and_then(|value| value.try_into().ok()))
                    })?
                    .try_into()
                    .ok()?;
                let count = row.get("row_count").and_then(|value| {
                    value
                        .as_u64()
                        .or_else(|| value.as_i64().and_then(|value| value.try_into().ok()))
                })?;
                Some((index, count))
            })
            .collect::<BTreeMap<usize, u64>>();
        self.relations = self
            .relations
            .into_iter()
            .enumerate()
            .map(|(index, relation)| relation.with_row_count(counts.get(&index).copied()))
            .collect();
        self.complete = self.complete
            && self.relations.len() <= self.countable_relations
            && self
                .relations
                .iter()
                .all(|relation| relation.row_count().is_some());
        self
    }

    fn relations(&self) -> &[CatalogRelation] {
        &self.relations
    }

    const fn complete(&self) -> bool {
        self.complete
    }

    fn limitation(&self) -> Option<&str> {
        self.limitation.as_deref()
    }
}

impl ProjectRelationScope<'_> {
    fn includes(&self, relation: &CatalogRelation) -> bool {
        let name = relation.name();
        if name == self.namespace
            || name == format!("{}_video_sources", self.namespace)
            || name == format!("{}.video", self.namespace)
            || name == format!("{}.video.assets", self.namespace)
            || name.starts_with(&format!("{}.features_", self.namespace))
        {
            return false;
        }
        ![
            Table::Robots,
            Table::StudioSettings,
            Table::TrainingCheckpoints,
            Table::TrainingRuns,
            Table::TrainingRunMetrics,
            Table::TrainingRunCheckpoints,
            Table::RolloutRuns,
            Table::WorkflowRuns,
            Table::FeatureRegistry,
        ]
        .into_iter()
        .any(|table| name == table.qualified_name(self.namespace))
    }
}

#[component]
pub(crate) fn RelationsPanel(namespace: String, query_available: bool) -> Element {
    let controller = crate::activities::data::state::QueryController::current();
    let mut selected = use_signal(|| None::<String>);
    let count_namespace = namespace.clone();
    let sessions = crate::shell::project::ProjectSessionSignals::current();
    let mut counts = use_resource(move || {
        let available = sessions.catalog_available();
        let catalog = RelationCatalogState::current().relation_catalog();
        let namespace = count_namespace.clone();
        async move {
            if !available {
                return std::future::pending().await;
            }
            let Some(catalog) = query_available.then_some(catalog).flatten() else {
                return Ok(None);
            };
            let relations = ProjectRelations::from_catalog(&catalog, &namespace);
            let Some((statement, max_rows)) = relations.count_query() else {
                return Ok(None);
            };
            DatasetClient::current()
                .run_spiral_query(statement, max_rows)
                .await
                .map_err(|error| error.to_string())
                .map(Some)
        }
    });
    let catalog_result = RelationCatalogState::current().relation_catalog_result();
    let catalog_response = catalog_result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|catalog| ProjectRelations::from_catalog(catalog, &namespace))
        .unwrap_or_default();
    let catalog_error = catalog_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .cloned();
    let count_result = counts.read_unchecked().to_owned();
    let count_error = count_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .cloned();
    let response = count_result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .and_then(Option::as_ref)
        .map(|counts| catalog_response.clone().with_counts(counts))
        .unwrap_or(catalog_response);
    let selection_candidates = response
        .relations()
        .iter()
        .map(|relation| relation.name().to_owned())
        .collect::<Vec<_>>();
    use_effect(use_reactive(
        (&selection_candidates,),
        move |(candidates,)| {
            let current = selected.peek().to_owned();
            if current
                .as_ref()
                .is_some_and(|name| candidates.contains(name))
            {
                return;
            }
            selected.set(candidates.first().cloned());
        },
    ));

    let mut preview = use_resource(move || {
        let available = sessions.catalog_available();
        let relation = selected();
        async move {
            if !available {
                return std::future::pending().await;
            }
            let Some(relation) = relation else {
                return Ok(None);
            };
            if !query_available {
                return Ok(None);
            }
            DatasetClient::current()
                .run_spiral_query(relation.preview_sql(), PREVIEW_ROWS)
                .await
                .map_err(|error| error.to_string())
                .map(Some)
        }
    });
    let preview_result = preview.read_unchecked().to_owned();
    let selected_name = selected();
    let selected_relation = selected_name.as_ref().and_then(|name| {
        response
            .relations()
            .iter()
            .find(|relation| relation.name() == name)
    });
    let links = response.relations().relation_links();
    let counted_rows = response
        .relations()
        .iter()
        .filter_map(|relation| relation.row_count())
        .sum::<u64>();
    let count_complete = response.complete()
        && response
            .relations()
            .iter()
            .all(|relation| relation.row_count().is_some());
    let catalog_metrics = match catalog_result.as_ref() {
        None if query_available => "Reading table schemas…".to_owned(),
        Some(Err(_)) => "Metrics unavailable".to_owned(),
        _ if count_error.is_some() => format!(
            "{} tables · exact counts unavailable · {} shared-key links",
            response.relations().len(),
            links.len()
        ),
        _ if count_result.is_none() && !response.relations().is_empty() => format!(
            "{} tables · counting rows… · {} shared-key links",
            response.relations().len(),
            links.len()
        ),
        _ if count_complete => format!(
            "{} tables · {} rows · {} shared-key links",
            response.relations().len(),
            counted_rows.grouped_count(),
            links.len()
        ),
        _ => format!(
            "{} tables · {} counted rows · {} shared-key links",
            response.relations().len(),
            counted_rows.grouped_count(),
            links.len()
        ),
    };

    rsx! {
        section { class: "data-relations", "aria-label": "Project data relations",
            header { class: "data-relations-summary",
                div {
                    strong { "Project data" }
                    p { "{catalog_metrics}" }
                }
                Action {
                    purpose: ActionPurpose::Refresh,
                    variant: ActionVariant::Icon,
                    aria_label: "Refresh project data relations",
                    title: "Refresh table schemas, row counts, relationships, and the active preview",
                    disabled: catalog_result.is_none() && query_available,
                    onclick: move |_| {
                        RelationCatalogState::refresh();
                        preview.restart();
                    },
                    RefreshIcon {}
                }
            }

            if !query_available {
                div { class: "data-relations-message", role: "status",
                    strong { "Configure SpiralDB to inspect project data" }
                    p { "Open Settings and configure a Spiral catalog, project, and namespace." }
                }
            } else if let Some(problem) = catalog_error {
                div { class: "data-relations-message data-relations-message-error", role: "alert",
                    strong { "Project data is unavailable" }
                    p { "{problem.catalog_failure_summary()}" }
                    Action {
                        purpose: ActionPurpose::Retry,
                        variant: ActionVariant::Outline,
                        onclick: move |_| RelationCatalogState::refresh(),
                        "Retry"
                    }
                }
            } else if catalog_result.is_none() {
                div { class: "data-relations-loading", role: "status",
                    div { class: "data-relations-loading-line" }
                    div { class: "data-relations-loading-line data-relations-loading-line-short" }
                    span { "Reading table schemas…" }
                }
            } else if response.relations().is_empty() {
                div { class: "data-relations-message", role: "status",
                    strong { "No project tables yet" }
                    p { "Record an episode or add data to make the project’s tables visible here." }
                }
            } else {
                div { class: "data-relations-workspace",
                    aside { class: "data-relations-index", "aria-label": "Project tables",
                        div { class: "data-relations-index-heading",
                            span { "Tables" }
                            small { "{namespace}" }
                        }
                        nav {
                            for relation in response.relations().iter().cloned() {
                                {
                                    let relation_name = relation.name().to_owned();
                                    let active = selected_name.as_deref() == Some(relation.name());
                                    rsx! {
                                        Action {
                                            key: "{relation.name()}",
                                            purpose: ActionPurpose::Select,
                                            variant: ActionVariant::Row,
                                            class: if active { "data-relation-index-row data-relation-index-row-active" } else { "data-relation-index-row" },
                                            aria_pressed: active,
                                            title: "Inspect {relation.name()}",
                                            onclick: move |_| selected.set(Some(relation_name.clone())),
                                            span {
                                                strong { "{relation.name().relation_label()}" }
                                                small { "{relation.columns().len()} columns" }
                                            }
                                            code { "{relation.row_count().grouped_count()}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    main { class: "data-relations-inspector",
                        section { class: "data-relationship-map", "aria-label": "Shared-key relationships",
                            header {
                                div {
                                    strong { "Shared keys" }
                                    p { "Matching ID fields show likely joins, not declared database constraints." }
                                }
                                small { "{links.len()} links" }
                            }
                            if links.is_empty() {
                                p { class: "data-relationship-empty", "No matching ID fields connect these tables." }
                            } else {
                                ol { class: "data-relationship-list",
                                    for link in links.iter().cloned() {
                                        {
                                            let left_name = link.left.clone();
                                            let right_name = link.right.clone();
                                            let keys_label = link.keys.join(" + ");
                                            let related = selected_name.as_ref().is_some_and(|selected| {
                                                selected == &link.left || selected == &link.right
                                            });
                                            rsx! {
                                                li { class: if related { "data-relationship-row data-relationship-row-active" } else { "data-relationship-row" },
                                                    Action {
                                                        purpose: ActionPurpose::Select,
                                                        variant: ActionVariant::Link,
                                                        title: "Inspect {link.left}",
                                                        onclick: move |_| selected.set(Some(left_name.clone())),
                                                        "{link.left}"
                                                    }
                                                    span { class: "data-relationship-keys",
                                                        span { "aria-hidden": "true" }
                                                        code { "{keys_label}" }
                                                        span { "aria-hidden": "true" }
                                                    }
                                                    Action {
                                                        purpose: ActionPurpose::Select,
                                                        variant: ActionVariant::Link,
                                                        title: "Inspect {link.right}",
                                                        onclick: move |_| selected.set(Some(right_name.clone())),
                                                        "{link.right}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        section { class: "data-relation-preview", "aria-label": "Selected table preview",
                            if let Some(relation) = selected_relation {
                                header { class: "data-relation-preview-heading",
                                    div {
                                        strong { "{relation.name().relation_label()}" }
                                        p { title: "{relation.name()}", "{relation.name()}" }
                                    }
                                    span { "{relation.row_count().grouped_count()} rows · {relation.columns().len()} columns" }
                                    Action {
                                        purpose: ActionPurpose::Apply,
                                        variant: ActionVariant::Outline,
                                        title: "Put this table’s preview SQL in the Query panel",
                                        onclick: {
                                            let relation_name = relation.name().to_owned();
                                            move |_| controller.set_sql(format!("SELECT * FROM {relation_name} LIMIT 100"))
                                        },
                                        "Load in Query"
                                    }
                                }
                                details { class: "data-relation-columns",
                                    summary { "Schema · {relation.columns().len()} columns" }
                                    ul {
                                        for column in relation.columns().iter() {
                                            li {
                                                code { "{column.name()}" }
                                                span { "{column.semantic_type().unwrap_or(column.data_type())}" }
                                                small { if column.nullable() { "nullable" } else { "required" } }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "data-relation-preview-body",
                                {preview_result.as_ref().render_preview(selected_name.as_deref())}
                            }
                        }
                    }
                }
            }
            if let Some(limitation) = response.limitation() {
                footer { class: "data-relations-limitation", role: "status", "{limitation}" }
            }
            if count_error.is_some() {
                footer { class: "data-relations-limitation", role: "status",
                    span { "Exact row counts could not be loaded in the combined query." }
                    Action {
                        purpose: ActionPurpose::Retry,
                        variant: ActionVariant::Link,
                        onclick: move |_| counts.restart(),
                        "Retry counts"
                    }
                }
            }
        }
    }
}

trait RelationPreview {
    fn render_preview(self, relation: Option<&str>) -> Element;
}

impl RelationPreview for Option<&Result<Option<SpiralQuerySnapshot>, String>> {
    fn render_preview(self, relation: Option<&str>) -> Element {
        let Some(relation) = relation else {
            return rsx! {
                div { class: "data-relation-preview-state", "Select a table to preview its rows." }
            };
        };
        match self {
            None => rsx! {
                div { class: "data-relation-preview-state", role: "status", "Loading five rows…" }
            },
            Some(Err(error)) => rsx! {
                div { class: "data-relation-preview-state data-relation-preview-error", role: "alert",
                    strong { "Preview unavailable" }
                    p { "{error.preview_failure_summary()}" }
                }
            },
            Some(Ok(None)) => rsx! {
                div { class: "data-relation-preview-state", "Select a table to preview its rows." }
            },
            Some(Ok(Some(response))) => {
                let model = QueryExecutionPresentation::new(
                    relation.preview_sql(),
                    PREVIEW_ROWS,
                    QueryState::Ready(response.clone()),
                );
                model.render()
            }
        }
    }
}

trait RelationName {
    fn preview_sql(&self) -> String;
    fn relation_label(&self) -> &str;
    fn is_relationship_key(&self) -> bool;
}

impl RelationName for str {
    fn preview_sql(&self) -> String {
        format!("SELECT * FROM {self} LIMIT {PREVIEW_ROWS}")
    }

    fn relation_label(&self) -> &str {
        self.rsplit('.').next().unwrap_or(self)
    }

    fn is_relationship_key(&self) -> bool {
        self.ends_with("_id")
    }
}

trait RelationCollection {
    fn relation_links(&self) -> Vec<RelationLink>;
}

impl RelationCollection for [CatalogRelation] {
    fn relation_links(&self) -> Vec<RelationLink> {
        let mut links = Vec::new();
        for (index, left) in self.iter().enumerate() {
            let left_keys = left
                .columns()
                .iter()
                .map(|column| column.name())
                .filter(|name| name.is_relationship_key())
                .collect::<BTreeSet<_>>();
            for right in self.iter().skip(index + 1) {
                let mut keys = right
                    .columns()
                    .iter()
                    .map(|column| column.name())
                    .filter(|name| left_keys.contains(name))
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>();
                keys.sort();
                if !keys.is_empty() {
                    links.push(RelationLink {
                        left: left.name().to_owned(),
                        right: right.name().to_owned(),
                        keys,
                    });
                }
            }
        }
        links.sort_by(|left, right| {
            right
                .keys
                .len()
                .cmp(&left.keys.len())
                .then_with(|| left.left.cmp(&right.left))
                .then_with(|| left.right.cmp(&right.right))
        });
        links
    }
}

trait RelationFailurePresentation {
    fn catalog_failure_summary(&self) -> &'static str;
    fn preview_failure_summary(&self) -> &'static str;
}

impl RelationFailurePresentation for str {
    fn catalog_failure_summary(&self) -> &'static str {
        if self.contains("catalog is not ready") || self.contains("catalog is unavailable") {
            "The project catalog is not ready. Check SpiralDB in Settings, then retry."
        } else {
            "Studio could not read this project’s tables. Retry, or check the SpiralDB connection in Settings."
        }
    }

    fn preview_failure_summary(&self) -> &'static str {
        if self.contains("catalog is not ready") || self.contains("catalog is unavailable") {
            "The project catalog is not ready. Check SpiralDB in Settings, then retry."
        } else {
            "Studio could not preview this table. Refresh the relations view or check the SpiralDB connection in Settings."
        }
    }
}

trait RelationCountPresentation {
    fn grouped_count(self) -> String;
}

impl RelationCountPresentation for u64 {
    fn grouped_count(self) -> String {
        let digits = self.to_string();
        let mut rendered = String::with_capacity(digits.len() + digits.len() / 3);
        for (index, character) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                rendered.push(',');
            }
            rendered.push(character);
        }
        rendered
    }
}

impl RelationCountPresentation for Option<u64> {
    fn grouped_count(self) -> String {
        self.map(RelationCountPresentation::grouped_count)
            .unwrap_or_else(|| "—".to_owned())
    }
}

/// Completed schema reads are local to the workspace, refreshed by catalog mutations.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct RelationCatalogState {
    result: ReadSignal<Option<Result<spiral_robotics_common::relations::RelationCatalog, String>>>,
}
impl RelationCatalogState {
    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn use_provider() -> Self {
        let sessions = crate::shell::project::ProjectSessionSignals::current();
        let available = use_memo(move || sessions.catalog_available());
        let request = crate::client::queries::QueryPolicy::on_demand(
            crate::client::queries::QueryDomain::Catalog,
        )
        .use_when(
            move || *available.read(),
            || async {
                super::DatasetClient::current()
                    .relation_catalog()
                    .await
                    .map_err(|error| error.to_string())
            },
        );
        use_context_provider(|| Self {
            result: request.value(),
        })
    }
    pub(crate) fn signal(
        self,
    ) -> ReadSignal<Option<Result<spiral_robotics_common::relations::RelationCatalog, String>>>
    {
        self.result
    }
    pub(super) fn relation_catalog_result(
        self,
    ) -> Option<Result<spiral_robotics_common::relations::RelationCatalog, String>> {
        self.result.read().clone()
    }
    fn relation_catalog(self) -> Option<spiral_robotics_common::relations::RelationCatalog> {
        self.relation_catalog_result().and_then(Result::ok)
    }
    pub(crate) fn refresh() {
        crate::client::queries::QueryRefresh::current()
            .invalidate(&[crate::client::queries::QueryDomain::Catalog]);
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_common::relations::QueryColumn;

    use super::*;

    fn relation(name: &str, columns: &[&str]) -> CatalogRelation {
        CatalogRelation::new(
            name,
            "TABLE",
            columns
                .iter()
                .map(|name| QueryColumn::new(*name, "Utf8", false))
                .collect(),
        )
        .with_row_count(Some(1))
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn relationships_use_shared_identity_fields_without_claiming_every_column() {
        let links = [
            relation("robot.datasets", &["dataset_id", "name"]),
            relation("robot.episodes", &["dataset_id", "episode_id"]),
            relation(
                "robot.samples",
                &["episode_id", "stream_id", "sample_index"],
            ),
        ]
        .relation_links();

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].keys, ["dataset_id"]);
        assert_eq!(links[1].keys, ["episode_id"]);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn counts_are_grouped_for_fast_scanning() {
        assert_eq!(0_u64.grouped_count(), "0");
        assert_eq!(999_u64.grouped_count(), "999");
        assert_eq!(12_345_678_u64.grouped_count(), "12,345,678");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn project_relation_counts_use_one_read_only_statement() {
        let catalog = RelationCatalog::new(
            vec![
                relation("robotics.episodes", &["episode_id"]),
                relation("customer.annotations", &["episode_id"]),
            ],
            true,
        );
        let (statement, max_rows) = ProjectRelations::from_catalog(&catalog, "robotics")
            .count_query()
            .expect("two visible relations produce a count query");

        assert_eq!(max_rows, 2);
        assert_eq!(statement.matches("COUNT(*)").count(), 2);
        assert_eq!(statement.matches("UNION ALL").count(), 1);
        assert!(!statement.contains(';'));
        spiral_robotics_common::sql::ReadOnlyQuery::new(
            &statement,
            spiral_robotics_common::sql::ReadOnlyQueryMode::Preview,
        )
        .authorize()
        .expect("the combined count query must pass the shared read-only policy");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn project_relation_scope_hides_studio_plumbing_but_keeps_external_relations() {
        let scope = ProjectRelationScope {
            namespace: "robotics",
        };
        for hidden in [
            "robotics.robots",
            "robotics.studio_settings",
            "robotics.training_checkpoints",
            "robotics.training_runs",
            "robotics.rollout_runs",
            "robotics.workflow_runs",
            "robotics.feature_registry",
            "robotics.video.assets",
        ] {
            assert!(!scope.includes(&relation(hidden, &["id"])), "{hidden}");
        }
        assert!(scope.includes(&relation("robotics.episodes", &["episode_id"])));
        assert!(scope.includes(&relation("customer.annotations", &["episode_id"])));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn backend_errors_are_replaced_with_recovery_copy() {
        assert_eq!(
            "500: driver exploded at /private/path".catalog_failure_summary(),
            "Studio could not read this project’s tables. Retry, or check the SpiralDB connection in Settings."
        );
        assert_eq!(
            "500: driver exploded at /private/path".preview_failure_summary(),
            "Studio could not preview this table. Refresh the relations view or check the SpiralDB connection in Settings."
        );
    }
}
