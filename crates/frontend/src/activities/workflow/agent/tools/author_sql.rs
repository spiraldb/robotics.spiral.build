use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SetWorkflowArguments {
    workflow: Value,
}

pub(super) struct SetWorkflow;
impl WorkflowTool for SetWorkflow {
    fn name(&self) -> &'static str {
        "set_workflow"
    }
    fn description(&self) -> &'static str {
        "Validate SQL with Spiral, then update the workflow draft in memory. SQL defines the input and native AI measurements. An optional comparison output adds the episode selection and full-population aggregation to the same executable workflow. Optionally configure comparison: SQL must provide one row per episode with episode_id and numeric measurement columns, before applying the selection criterion. The dataset catalog supplies duration_seconds and message_count. The description must explain camera, sampling, units, thresholds, and uncertainty. Missing episodes or criterion values remain unknown. The normal run_workflow action publishes comparison aggregates as this workflow run’s output. Compare only presents that output; it has no separate execution. The graph explains the complete execution, including selection and aggregation. Use inspected catalog signatures and native default models; never invent functions. The native query plan is derived for display. Validation does not execute the query. Use run_workflow to evaluate it and inspect_run to examine the results. Set comparison to null when switching back to ordinary SQL rows, including image-evidence queries. Only the user's Save button persists the definition and conversation."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"workflow":{
            "type":"object","properties":{"name":{"type":"string"},"sql":{"type":"string"},
                "comparison": {"type":["object","null"], "properties": {
                    "dataset":{"type":"object","properties":{"path":{"type":"array","items":{"type":"string"}}},"required":["path"],"additionalProperties":false},
                    "description":{"type":"string"},
                    "criterion":{"type":"object","properties":{"column":{"type":"string"},"operator":{"type":"string","enum":["less","less_equal","greater","greater_equal"]},"threshold":{"type":"string"}},"required":["column","operator","threshold"],"additionalProperties":false},
                    "metrics":{"type":"array","minItems":1,"maxItems":6,"items":{"type":"object","properties":{"column":{"type":"string"},"label":{"type":"string"},"unit":{"type":"string"}},"required":["column","label","unit"],"additionalProperties":false}},
                    "baseline":{"type":"string","enum":["all"]},"baseline_workflow":{"type":["string","null"]}
                },"required":["dataset","description","criterion","metrics","baseline"],"additionalProperties":false}},
            "required":["name","sql"],"additionalProperties":false
        }},"required":["workflow"],"additionalProperties":false})
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new("Planning SQL", "SQL update", "update the workflow")
    }
    fn completion(&self, output: &Value) -> ToolCompletion {
        ToolCompletion::query_updated(output)
    }
    fn call<'a>(
        &'a self,
        invocation: ToolInvocation<'a>,
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let arguments: SetWorkflowArguments = invocation.arguments()?;
            let mut value = arguments.workflow;
            let object = value
                .as_object_mut()
                .ok_or("set_workflow requires a workflow object.")?;
            // The editor owns identity and saved version; the model authors content only.
            object.insert("id".into(), json!(invocation.expected.id()));
            object.insert("revision".into(), json!(invocation.expected.revision()));
            if !object.contains_key("comparison")
                && let Some(comparison) = invocation.expected.comparison()
            {
                object.insert(
                    "comparison".into(),
                    serde_json::to_value(comparison).map_err(|error| error.to_string())?,
                );
            }
            let workflow: WorkflowSpec = serde_json::from_value(value)
                .map_err(|error| format!("Workflow shape is invalid: {error}"))?;
            if let Some(comparison) = workflow.comparison() {
                comparison.validate()?;
            }
            invocation.update_query(workflow).await
        })
    }
}
