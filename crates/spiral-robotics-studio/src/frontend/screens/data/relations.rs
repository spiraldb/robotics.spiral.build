//! User-facing project relation map and bounded row preview.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_contracts::api::{
    ProjectDataRelation, ProjectDataRelationsResponse, SpiralQueryRequest, SpiralQueryResponse,
};
use spiral_robotics_ui::{Action, ActionPurpose, ActionVariant, RefreshIcon};

use crate::frontend::api::http as api;
use crate::frontend::screens::data::state::data_controller;
use crate::frontend::screens::data::workbench::{QueryState, QueryViewModel, render_query_result};

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
                api::get::<ProjectDataRelationsResponse>("/api/spiral/project-relations").await
            } else {
                Ok(ProjectDataRelationsResponse::default())
            }
        }
    });
    let catalog_result = catalog.read_unchecked().clone();
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
        .relations
        .iter()
        .map(|relation| relation.name.clone())
        .collect::<Vec<_>>();
    use_effect(use_reactive(
        (&selection_candidates,),
        move |(candidates,)| {
            let current = selected.peek().clone();
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
            let request = SpiralQueryRequest {
                sql: preview_sql(&relation),
                max_rows: PREVIEW_ROWS,
            };
            api::send_json::<_, SpiralQueryResponse>(
                Method::POST,
                "/api/spiral/query",
                Some(&request),
            )
            .await
            .map(Some)
        }
    });
    let preview_result = preview.read_unchecked().clone();
    let selected_name = selected();
    let selected_relation = selected_name.as_ref().and_then(|name| {
        response
            .relations
            .iter()
            .find(|relation| &relation.name == name)
    });
    let links = relation_links(&response.relations);
    let counted_rows = response
        .relations
        .iter()
        .filter_map(|relation| relation.row_count)
        .sum::<u64>();
    let count_complete = response.complete
        && response
            .relations
            .iter()
            .all(|relation| relation.row_count.is_some());
    let catalog_metrics = match catalog_result.as_ref() {
        None if query_available => "Reading tables and counting rows…".to_owned(),
        Some(Err(_)) => "Metrics unavailable".to_owned(),
        _ if count_complete => format!(
            "{} tables · {} rows · {} shared-key links",
            response.relations.len(),
            format_count(counted_rows),
            links.len()
        ),
        _ => format!(
            "{} tables · {} counted rows · {} shared-key links",
            response.relations.len(),
            format_count(counted_rows),
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
                    p { "{catalog_failure_summary(&problem)}" }
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
            } else if response.relations.is_empty() {
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
                            for relation in response.relations.iter().cloned() {
                                {
                                    let relation_name = relation.name.clone();
                                    let active = selected_name.as_deref() == Some(relation.name.as_str());
                                    rsx! {
                                        Action {
                                            key: "{relation.name}",
                                            purpose: ActionPurpose::Select,
                                            variant: ActionVariant::Row,
                                            class: if active { "data-relation-index-row data-relation-index-row-active" } else { "data-relation-index-row" },
                                            aria_pressed: active,
                                            title: "Inspect {relation.name}",
                                            onclick: move |_| selected.set(Some(relation_name.clone())),
                                            span {
                                                strong { "{relation_label(&relation.name)}" }
                                                small { "{relation.columns.len()} columns" }
                                            }
                                            code { "{format_optional_count(relation.row_count)}" }
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
                                        strong { "{relation_label(&relation.name)}" }
                                        p { title: "{relation.name}", "{relation.name}" }
                                    }
                                    span { "{format_optional_count(relation.row_count)} rows · {relation.columns.len()} columns" }
                                    Action {
                                        purpose: ActionPurpose::Apply,
                                        variant: ActionVariant::Outline,
                                        title: "Put this table’s preview SQL in the Query panel",
                                        onclick: {
                                            let relation_name = relation.name.clone();
                                            move |_| controller.set_sql(format!("SELECT * FROM {relation_name} LIMIT 100"))
                                        },
                                        "Load in Query"
                                    }
                                }
                                details { class: "data-relation-columns",
                                    summary { "Schema · {relation.columns.len()} columns" }
                                    ul {
                                        for column in relation.columns.iter() {
                                            li {
                                                code { "{column.name}" }
                                                span { "{column.semantic_type.as_deref().unwrap_or(&column.data_type)}" }
                                                small { if column.nullable { "nullable" } else { "required" } }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "data-relation-preview-body",
                                {render_preview(preview_result.as_ref(), selected_name.as_deref())}
                            }
                        }
                    }
                }
            }
            if let Some(limitation) = response.limitation {
                footer { class: "data-relations-limitation", role: "status", "{limitation}" }
            }
        }
    }
}

fn render_preview(
    result: Option<&Result<Option<SpiralQueryResponse>, String>>,
    relation: Option<&str>,
) -> Element {
    let Some(relation) = relation else {
        return rsx! {
            div { class: "data-relation-preview-state", "Select a table to preview its rows." }
        };
    };
    match result {
        None => rsx! {
            div { class: "data-relation-preview-state", role: "status", "Loading five rows…" }
        },
        Some(Err(error)) => rsx! {
            div { class: "data-relation-preview-state data-relation-preview-error", role: "alert",
                strong { "Preview unavailable" }
                p { "{preview_failure_summary(error)}" }
            }
        },
        Some(Ok(None)) => rsx! {
            div { class: "data-relation-preview-state", "Select a table to preview its rows." }
        },
        Some(Ok(Some(response))) => {
            let model = QueryViewModel {
                sql: preview_sql(relation),
                max_rows: PREVIEW_ROWS,
                state: QueryState::Ready(response.clone()),
            };
            render_query_result(&model)
        }
    }
}

fn preview_sql(relation: &str) -> String {
    format!("SELECT * FROM {relation} LIMIT {PREVIEW_ROWS}")
}

fn catalog_failure_summary(error: &str) -> &'static str {
    if error.contains("catalog is not ready") || error.contains("catalog is unavailable") {
        "The project catalog is not ready. Check SpiralDB in Settings, then retry."
    } else {
        "Studio could not read this project’s tables. Retry, or check the SpiralDB connection in Settings."
    }
}

fn preview_failure_summary(error: &str) -> &'static str {
    if error.contains("catalog is not ready") || error.contains("catalog is unavailable") {
        "The project catalog is not ready. Check SpiralDB in Settings, then retry."
    } else {
        "Studio could not preview this table. Refresh the relations view or check the SpiralDB connection in Settings."
    }
}

fn relation_links(relations: &[ProjectDataRelation]) -> Vec<RelationLink> {
    let mut links = Vec::new();
    for (index, left) in relations.iter().enumerate() {
        let left_keys = left
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .filter(relationship_key)
            .collect::<BTreeSet<_>>();
        for right in relations.iter().skip(index + 1) {
            let mut keys = right
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .filter(|name| left_keys.contains(name))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            keys.sort();
            if !keys.is_empty() {
                links.push(RelationLink {
                    left: left.name.clone(),
                    right: right.name.clone(),
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

fn relationship_key(name: &&str) -> bool {
    name.ends_with("_id")
}

fn relation_label(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn format_optional_count(count: Option<u64>) -> String {
    count.map(format_count).unwrap_or_else(|| "—".to_owned())
}

fn format_count(count: u64) -> String {
    let digits = count.to_string();
    let mut rendered = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            rendered.push(',');
        }
        rendered.push(character);
    }
    rendered
}

#[cfg(test)]
mod tests {
    use spiral_robotics_contracts::api::QueryColumn;

    use super::*;

    fn relation(name: &str, columns: &[&str]) -> ProjectDataRelation {
        ProjectDataRelation {
            name: name.to_owned(),
            relation_type: "TABLE".to_owned(),
            columns: columns
                .iter()
                .map(|name| QueryColumn {
                    name: (*name).to_owned(),
                    data_type: "Utf8".to_owned(),
                    nullable: false,
                    semantic_type: None,
                })
                .collect(),
            row_count: Some(1),
        }
    }

    #[test]
    fn relationships_use_shared_identity_fields_without_claiming_every_column() {
        let links = relation_links(&[
            relation("robot.datasets", &["dataset_id", "name"]),
            relation("robot.episodes", &["dataset_id", "episode_id"]),
            relation(
                "robot.samples",
                &["episode_id", "stream_id", "sample_index"],
            ),
        ]);

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].keys, ["dataset_id"]);
        assert_eq!(links[1].keys, ["episode_id"]);
    }

    #[test]
    fn counts_are_grouped_for_fast_scanning() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(12_345_678), "12,345,678");
    }

    #[test]
    fn backend_errors_are_replaced_with_recovery_copy() {
        assert_eq!(
            catalog_failure_summary("500: driver exploded at /private/path"),
            "Studio could not read this project’s tables. Retry, or check the SpiralDB connection in Settings."
        );
        assert_eq!(
            preview_failure_summary("500: driver exploded at /private/path"),
            "Studio could not preview this table. Refresh the relations view or check the SpiralDB connection in Settings."
        );
    }
}
