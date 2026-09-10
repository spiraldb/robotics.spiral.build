use super::*;

pub(super) struct ListModelCapabilities;
impl WorkflowTool for ListModelCapabilities {
    fn name(&self) -> &'static str {
        "list_model_capabilities"
    }
    fn description(&self) -> &'static str {
        "List executable Workflow Model capabilities and input contracts."
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
        ToolCompletion::count(output, "models", "model capabilities")
    }
    fn call<'a>(&'a self, invocation: ToolInvocation<'a>) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let _: NoArguments = invocation.arguments()?;
            Ok(json!({
                "models": [{
                    "id": "siglip-base",
                    "label": "SigLIP base",
                    "max_features": 32,
                    "score_semantics": "text/image matching score, not measured brightness or a calibrated probability",
                    "native_episode_authoring": true,
                    "native_episode_execution": false,
                    "device_policy": "Metal when available; CPU fallback"
                }]
            }).to_string())
        })
    }
}
