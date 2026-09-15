use super::super::super::function_discovery::NativeFunctionDiscovery;
use super::*;
pub(super) struct InspectFunctions;
impl WorkflowTool for InspectFunctions {
    fn name(&self) -> &'static str {
        "inspect_functions"
    }
    fn description(&self) -> &'static str {
        NativeFunctionDiscovery::description()
    }
    fn parameters(&self) -> Value {
        NativeFunctionDiscovery::parameters()
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            "Reading function documentation",
            "Function discovery",
            "inspect functions",
        )
    }
    fn call<'a>(
        &'a self,
        invocation: ToolInvocation<'a>,
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        Box::pin(async move { NativeFunctionDiscovery::inspect(invocation.arguments).await })
    }
}
