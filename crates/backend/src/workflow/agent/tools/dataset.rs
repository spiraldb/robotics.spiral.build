use super::*;

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EpisodeFilterEdit {
    name: String,
    scoring: spiral_robotics_common::workflow::episode_filter::CameraScoring,
    rule: spiral_robotics_common::workflow::episode_filter::EpisodeFilterRule,
}
impl EpisodeFilterEdit {
    fn apply(self, workflow: &mut WorkflowSpec) -> Result<(), String> {
        workflow.set_name(self.name);
        workflow.set_episode_filter(self.scoring, self.rule)
    }
}

pub(super) struct InspectDataset;
impl WorkflowTool for InspectDataset {
    fn name(&self) -> &'static str {
        "inspect_dataset"
    }
    fn description(&self) -> &'static str {
        "Inspect the selected native dataset, real topics/schema variants, and supported camera bindings. No model execution or media decoding."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object", "properties":{}, "additionalProperties":false})
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
            let _: NoArguments = invocation.arguments()?;
            let workflow = invocation.workflow();
            let source = workflow
                .dataset()
                .ok_or("Choose a dataset in the workflow selector first.")?;
            let inspection = invocation
                .storage()
                .inspect_dataset(source.selection().clone())
                .await?;
            let cameras = invocation.storage().workflow_cameras(&inspection).await?;
            Ok(json!({
                "dataset": inspection,
                "cameras": cameras,
                "camera_input": "foxglove.CompressedVideo value.data and value.format. Spiral supports packet video through spql.video.from_packets, spql.video.frames, and spql.video.decode. The typed episode-filter draft is not yet connected to that execution path.",
            }).to_string())
        })
    }
}

pub(super) struct SetEpisodeFilter;
impl WorkflowTool for SetEpisodeFilter {
    fn name(&self) -> &'static str {
        "set_episode_filter"
    }
    fn description(&self) -> &'static str {
        "Save or refine the selected dataset's episode-filter graph. Camera binding must come from inspect_dataset. Preserves workflow and node identities; never executes. Specify the full scoring and decision rule."
    }
    fn parameters(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(EpisodeFilterEdit))
            .expect("episode filter tool schema")
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            "Saving episode filter",
            "Episode filter",
            "save the episode filter",
        )
    }
    fn available(&self, _workflow: &WorkflowSpec) -> bool {
        true
    }
    fn completion(&self, output: &Value) -> ToolCompletion {
        ToolCompletion::graph_saved(output)
    }
    fn call<'a>(&'a self, invocation: ToolInvocation<'a>) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let edit: EpisodeFilterEdit = invocation.arguments()?;
            let mut workflow = invocation.workflow();
            edit.apply(&mut workflow)?;
            invocation.save(workflow).await
        })
    }
}
