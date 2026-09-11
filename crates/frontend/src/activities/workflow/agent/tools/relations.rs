use super::*;

use super::super::{MAX_WORKFLOW_AGENT_QUERY_ROWS, SpiralQuerySnapshotAgentExt};
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeRelationsArguments {
    relations: Vec<String>,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RunQueryArguments {
    sql: String,
    max_rows: Option<u64>,
}

pub(super) struct ListRelations;
impl WorkflowTool for ListRelations {
    fn name(&self) -> &'static str {
        "list_relations"
    }
    fn description(&self) -> &'static str {
        "List qualified Spiral relations in the active project."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{},"additionalProperties":false})
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            "Inspecting Spiral relations",
            "Relation discovery",
            "find relations",
        )
    }
    fn completion(&self, output: &Value) -> ToolCompletion {
        ToolCompletion::count(output, "relations", "Spiral relations")
    }
    fn call<'a>(
        &'a self,
        invocation: ToolInvocation<'a>,
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let _: NoArguments = invocation.arguments()?;
            let response = WorkflowClient::current()
                .query::<spiral_robotics_common::relations::RelationCatalog>(HttpRequest::new(
                    "GET",
                    "/api/relations",
                ))
                .await?;
            Ok(json!({
                "relations": response.relations().iter().map(|relation| relation.name()).collect::<Vec<_>>(),
                "complete": response.complete(),
                "limitation": response.limitation(),
            }).to_string())
        })
    }
}

pub(super) struct DescribeRelations;
impl WorkflowTool for DescribeRelations {
    fn name(&self) -> &'static str {
        "describe_relations"
    }
    fn description(&self) -> &'static str {
        "Describe fields and semantic types for qualified Spiral relations."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"relations":{"type":"array","items":{"type":"string"},"maxItems":20}},"required":["relations"],"additionalProperties":false})
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            "Reading relation schemas",
            "Schema inspection",
            "inspect relation fields",
        )
    }
    fn completion(&self, output: &Value) -> ToolCompletion {
        ToolCompletion::count(output, "relations", "relation schemas")
    }
    fn call<'a>(
        &'a self,
        invocation: ToolInvocation<'a>,
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let arguments: DescribeRelationsArguments = invocation.arguments()?;
            if arguments.relations.len() > 20 {
                return Err("Describe at most 20 relations per tool call.".into());
            }
            let discovered = WorkflowClient::current()
                .query::<spiral_robotics_common::relations::RelationCatalog>(HttpRequest::new(
                    "GET",
                    "/api/relations",
                ))
                .await?;
            let relations = discovered
                .relations()
                .iter()
                .filter(|relation| {
                    arguments
                        .relations
                        .iter()
                        .any(|name| name == relation.name())
                })
                .collect::<Vec<_>>();
            Ok(json!({"relations":relations}).to_string())
        })
    }
}

pub(super) struct RunQuery;
impl WorkflowTool for RunQuery {
    fn name(&self) -> &'static str {
        "run_query"
    }
    fn description(&self) -> &'static str {
        "Run a bounded read-only Spiral query for inspection only."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"sql":{"type":"string"},"max_rows":{"type":"integer","minimum":1,"maximum":50}},"required":["sql","max_rows"],"additionalProperties":false})
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            "Running bounded query",
            "Bounded query",
            "inspect query results",
        )
    }
    fn completion(&self, output: &Value) -> ToolCompletion {
        ToolCompletion::query_preview(output)
    }
    fn call<'a>(
        &'a self,
        invocation: ToolInvocation<'a>,
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let arguments: RunQueryArguments = invocation.arguments()?;
            let max_rows = arguments.max_rows.unwrap_or(20);
            if max_rows == 0 || max_rows > MAX_WORKFLOW_AGENT_QUERY_ROWS as u64 {
                return Err(format!(
                    "run_query max_rows must be between 1 and {MAX_WORKFLOW_AGENT_QUERY_ROWS}."
                ));
            }
            let response: spiral_robotics_common::relations::SpiralQuerySnapshot =
                WorkflowClient::current()
                    .query(
                        HttpRequest::new("POST", "/api/queries")
                            .json(&json!({"statement":arguments.sql, "max_rows":max_rows})),
                    )
                    .await?;
            Ok(response.serialize_for_agent_prompt())
        })
    }
}
