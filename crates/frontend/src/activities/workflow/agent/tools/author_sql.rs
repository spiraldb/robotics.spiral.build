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
        "Validate SQL with Spiral, then update the workflow draft in memory. SQL is the complete executable definition, including native AI calls. Use inspected catalog signatures and native default models; never invent functions. The native query plan is derived for display. Validation does not execute the query. Use run_workflow to evaluate it and inspect_run to examine the results. Only the user's Save button persists the definition and conversation."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"workflow":{
            "type":"object","properties":{"name":{"type":"string"},"sql":{"type":"string"}},
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
            let workflow: WorkflowSpec = serde_json::from_value(value)
                .map_err(|error| format!("Workflow shape is invalid: {error}"))?;
            invocation.update_query(workflow).await
        })
    }
}
