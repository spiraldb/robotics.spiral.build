//! User-facing project relation map and bounded row preview.

use std::collections::BTreeSet;

use crate::components::interaction::{Action, ActionPurpose, ActionVariant, RefreshIcon};
use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_api::relations::{
    ProjectDataRelation, ProjectDataRelationsResponse, SpiralQueryRequest, SpiralQueryResponse,
};

use crate::activities::data::query_result::{QueryExecutionPresentation, QueryState};
use crate::activities::data::state::data_controller;
use crate::client::http as api;

const PREVIEW_ROWS: usize = 5;

#[derive(Clone, Debug, Eq, PartialEq)]
struct RelationLink {
    left: String,
    right: String,
    keys: Vec<String>,
}

#[component]
pub(crate) fn RelationsPanel(namespace: String, query_available: bool) -> Element {
    let controller = data_controller();
    let mut selected = use_signal(|| None::<String>);
    let mut catalog = use_resource(move || {
        let _generation = controller.relation_generation();
        async move {
            if query_available {
                api::Http::get::<ProjectDataRelationsResponse>("/api/spiral/project-relations")
                    .await
            } else {
                Ok(ProjectDataRelationsResponse::default())
            }
        }
    });
    let catalog_result = catalog.read_unchecked().to_owned();
    let response = catalog_result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .cloned()
        .unwrap_or_default();
    let catalog_error = catalog_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .cloned();
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
        let relation = selected();
        async move {
            let Some(relation) = relation else {
                return Ok(None);
            };
            if !query_available {
                return Ok(None);
            }
            let request = SpiralQueryRequest::new(relation.preview_sql(), PREVIEW_ROWS);
            api::Http::send_json::<_, SpiralQueryResponse>(
                Method::POST,
                "/api/spiral/query",
                Some(&request),
            )
            .await
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
        None if query_available => "Reading tables and counting rows…".to_owned(),
        Some(Err(_)) => "Metrics unavailable".to_owned(),
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
                        controller.refresh_relations();
                        catalog.restart();
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
                        onclick: move |_| catalog.restart(),
                        "Retry"
                    }
                }
            } else if catalog_result.is_none() {
                div { class: "data-relations-loading", role: "status",
                    div { class: "data-relations-loading-line" }
                    div { class: "data-relations-loading-line data-relations-loading-line-short" }
                    span { "Reading schemas and exact row counts…" }
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
        }
    }
}

trait RelationPreview {
    fn render_preview(self, relation: Option<&str>) -> Element;
}

impl RelationPreview for Option<&Result<Option<SpiralQueryResponse>, String>> {
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

impl RelationCollection for [ProjectDataRelation] {
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

#[cfg(test)]
mod tests {
    use spiral_robotics_api::relations::QueryColumn;

    use super::*;

    fn relation(name: &str, columns: &[&str]) -> ProjectDataRelation {
        ProjectDataRelation::new(
            name,
            "TABLE",
            columns
                .iter()
                .map(|name| QueryColumn::new(*name, "Utf8", false))
                .collect(),
        )
        .with_row_count(Some(1))
    }

    #[test]
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

    #[test]
    fn counts_are_grouped_for_fast_scanning() {
        assert_eq!(0_u64.grouped_count(), "0");
        assert_eq!(999_u64.grouped_count(), "999");
        assert_eq!(12_345_678_u64.grouped_count(), "12,345,678");
    }

    #[test]
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
