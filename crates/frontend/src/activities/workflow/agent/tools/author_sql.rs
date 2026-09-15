use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SetWorkflowArguments {
    workflow: AuthoredWorkflow,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoredWorkflow {
    name: String,
    sql: String,
}
impl AuthoredWorkflow {
    fn into_spec(self, expected: &WorkflowSpec) -> Result<WorkflowSpec, String> {
        let mut workflow = expected.clone();
        workflow.set_name(self.name);
        workflow.set_sql(self.sql);
        Ok(workflow)
    }
}

pub(super) struct SetWorkflow;
impl WorkflowTool for SetWorkflow {
    fn name(&self) -> &'static str {
        "set_workflow"
    }
    fn description(&self) -> &'static str {
        "Validate and update the workflow SQL. Return the rows, images, statistics, or episode decisions the user requested, preserving useful identifiers and missing-evidence status. Do not configure comparison, baseline, dashboard panels, or a special output type. A separate comparison agent interprets workflow results when the user opens Compare. Planning does not execute; use run_workflow to evaluate, inspect_run to inspect, and show_results to present the final query. Save remains explicit."
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
            let workflow = arguments.workflow.into_spec(invocation.expected)?;
            invocation.update_query(workflow).await
        })
    }
}
