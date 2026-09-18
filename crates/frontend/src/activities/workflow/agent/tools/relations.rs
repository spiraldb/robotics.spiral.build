use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeRelationsArguments {
    relations: Vec<String>,
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
            let response = WorkflowClient::current().relations().await?;
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
            let discovered = WorkflowClient::current().relations().await?;
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
