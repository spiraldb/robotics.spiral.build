use super::*;

pub(super) struct ListModelCapabilities;
impl WorkflowTool for ListModelCapabilities {
    fn name(&self) -> &'static str {
        "list_model_capabilities"
    }
    fn description(&self) -> &'static str {
        "Read the installed Spiral AI function catalog: signatures, effects, visibility, and descriptions. Optional model and device arguments use native defaults when omitted."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{},"additionalProperties":false})
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            "Checking model capabilities",
            "Model discovery",
            "check model support",
        )
    }
    fn available(&self, _workflow: &WorkflowSpec) -> bool {
        true
    }
    fn completion(&self, output: &Value) -> ToolCompletion {
        ToolCompletion::count(output, "functions", "native AI functions")
    }
    fn call<'a>(
        &'a self,
        invocation: ToolInvocation<'a>,
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let _: NoArguments = invocation.arguments()?;
            let capabilities = WorkflowClient::current().capabilities().await?;
            Ok(
                json!({"functions": capabilities.rows(), "complete": !capabilities.truncated()})
                    .to_string(),
            )
        })
    }
}
