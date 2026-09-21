//! Browser-owned robot discovery loop over narrow backend data and network tools.
#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

use super::DatasetClient;
use crate::client::model::ModelRequest;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::{
    datasets::{
        DatasetRobot, DatasetSelection, HuggingFaceDatasetInspection, RobotDiscoveryResult,
        RobotStreamInspectionRequest,
    },
    workflow::conversation::ProviderMessage,
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RobotResolution {
    robot: Option<DatasetRobot>,
}

pub(super) struct RobotDiscoveryAgent {
    data: DatasetClient,
}

impl RobotDiscoveryAgent {
    pub(super) fn new(data: DatasetClient) -> Self {
        Self { data }
    }

    pub(super) async fn discover(
        &self,
        import: &HuggingFaceDatasetInspection,
    ) -> Result<(), String> {
        let selection = DatasetSelection::new(vec![format!("dataset_{}", import.dataset_id())])?;
        let context = self
            .data
            .robot_discovery_context(&selection)
            .await
            .map_err(|error| error.to_string())?;
        let repository = import.repository();
        let revision = import.resolved_revision();
        let mut messages = vec![
            ProviderMessage::text("system", include_str!("robot_discovery_prompt.txt")),
            ProviderMessage::text(
                "user",
                json!({
                    "description_url": format!("https://huggingface.co/datasets/{repository}/resolve/{revision}/README.md"),
                    "repository_files_url": format!("https://huggingface.co/api/datasets/{repository}/tree/{revision}"),
                    "episodes": import.files(),
                    "streams": context.topics(),
                })
                .to_string(),
            ),
        ];
        let tools = Self::tools();
        let model = self.data.model_client();
        for _ in 0..10 {
            let response = model
                .complete(
                    ModelRequest::new(messages.clone(), tools.clone()).with_model("gpt-5.6-terra"),
                    |_| {},
                )
                .await?;
            if response.tool_calls().is_empty() {
                return Err("Robot discovery did not produce a result".into());
            }
            messages.push(response.clone());
            for call in response.tool_calls() {
                let function = call.function();
                let arguments = serde_json::from_str::<Value>(function.arguments())
                    .map_err(|error| error.to_string())?;
                let result = match function.name() {
                    "read_description" => self.read_description(&arguments).await,
                    "inspect_stream" => {
                        self.inspect_stream(context.episode().clone(), &arguments)
                            .await
                    }
                    "finish" if arguments.get("robot").is_none() => {
                        Err("The robot result is required; use null for an absent model".into())
                    }
                    "finish" => match serde_json::from_value::<RobotResolution>(arguments) {
                        Ok(resolution) => {
                            let result = RobotDiscoveryResult::new(
                                context.episode().clone(),
                                resolution.robot,
                            );
                            match self.data.save_robot_discovery(&result).await {
                                Ok(()) => return Ok(()),
                                Err(error) => Err(error.to_string()),
                            }
                        }
                        Err(error) => Err(error.to_string()),
                    },
                    _ => Err("Unknown robot discovery tool".into()),
                };
                messages.push(ProviderMessage::tool(
                    call.id().into(),
                    match result {
                        Ok(value) => value.to_string(),
                        Err(error) => json!({"error": error}).to_string(),
                    },
                ));
            }
        }
        Err("Could not resolve the robot description".into())
    }

    async fn read_description(&self, arguments: &Value) -> Result<Value, String> {
        let url = arguments
            .get("url")
            .and_then(Value::as_str)
            .ok_or("A source URL is required")?;
        self.data
            .read_robot_description(url)
            .await
            .map(Value::String)
            .map_err(|error| error.to_string())
    }

    async fn inspect_stream(
        &self,
        episode: spiral_robotics_common::datasets::EpisodeSelection,
        arguments: &Value,
    ) -> Result<Value, String> {
        let topic = arguments
            .get("topic")
            .and_then(Value::as_str)
            .ok_or("A topic is required")?;
        let schema = arguments
            .get("schema")
            .and_then(Value::as_str)
            .ok_or("A schema is required")?;
        self.data
            .inspect_robot_stream(&RobotStreamInspectionRequest::new(episode, topic, schema))
            .await
            .map_err(|error| error.to_string())
    }

    fn tools() -> Vec<Value> {
        let mut result = schemars::schema_for!(RobotResolution);
        result.insert("required".into(), json!(["robot"]));
        vec![
            json!({"type":"function", "function":{"name":"read_description", "description":"Read a public robot description, included file, dataset documentation, or repository file listing. Follow only references supplied by the dataset or its linked model.", "parameters":{"type":"object", "properties":{"url":{"type":"string"}}, "required":["url"], "additionalProperties":false}}}),
            json!({"type":"function", "function":{"name":"inspect_stream", "description":"Read the Arrow schema and three actual messages of an episode stream before mapping it.", "parameters":{"type":"object", "properties":{"topic":{"type":"string"},"schema":{"type":"string"}}, "required":["topic","schema"], "additionalProperties":false}}}),
            json!({"type":"function", "function":{"name":"finish", "description":"Save the resolved visualization, or null when the dataset supplies no usable robot description. Meshes are fetched by the backend; leave meshes empty.", "parameters":result}}),
        ]
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn finish_schema_requires_an_explicit_nullable_result() {
        let tools = RobotDiscoveryAgent::tools();
        let schema = &tools[2]["function"]["parameters"];
        assert_eq!(schema["required"], json!(["robot"]));
        assert!(schema["$defs"]["DatasetRobot"].is_object());
        assert!(
            schema["properties"]["robot"]["anyOf"]
                .as_array()
                .unwrap()
                .iter()
                .any(|option| option["type"] == "null")
        );
        let missing: RobotResolution = serde_json::from_value(json!({"robot":null})).unwrap();
        assert!(missing.robot.is_none());
    }
}
