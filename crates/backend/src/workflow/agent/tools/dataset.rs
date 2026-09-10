use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DatasetArguments {
    path: Option<Vec<String>>,
}

pub(super) struct InspectDataset;
impl WorkflowTool for InspectDataset {
    fn name(&self) -> &'static str {
        "inspect_dataset"
    }
    fn description(&self) -> &'static str {
        "List datasets when path is omitted, or inspect a dataset path and its camera topics, schema variants, and packet columns. No inference or media decoding."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object", "properties":{"path":{"type":"array","items":{"type":"string"}}}, "additionalProperties":false})
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            "Inspecting dataset cameras",
            "Dataset inspection",
            "inspect dataset cameras",
        )
    }
    fn available(&self, _workflow: &WorkflowSpec) -> bool {
        true
    }
    fn call<'a>(&'a self, invocation: ToolInvocation<'a>) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let arguments: DatasetArguments = invocation.arguments()?;
            let Some(path) = arguments.path else {
                return Ok(json!({"datasets": invocation.storage().datasets().await?}).to_string());
            };
            let selection = spiral_robotics_common::datasets::DatasetSelection::new(path)?;
            let inspection = invocation.storage().inspect_dataset(selection).await?;
            let cameras = invocation.storage().workflow_cameras(&inspection).await?;
            Ok(json!({
                "dataset": inspection,
                "cameras": cameras,
                "camera_input": "foxglove.CompressedVideo value.data and value.format. Spiral supports packet video through spql.video.from_packets, spql.video.frames, and spql.video.decode.",
            }).to_string())
        })
    }
}
