//! Analyze an imported repository and save its robot alongside its episodes.
use crate::workflow::provider::{OpenAiProvider, ProviderConfig};
use base64::Engine as _;
use serde_json::{Value, json};
use spiral_robotics_common::{
    datasets::{
        DatasetRobot, DatasetSelection, DatasetTopic, EpisodeSelection,
        HuggingFaceDatasetInspection,
    },
    relations::EpisodeQueryRequest,
    workflow::conversation::ProviderMessage,
};
use spiral_robotics_storage::StorageManager;
use spiral_robotics_viewport::RobotModel;
use std::{collections::BTreeMap, time::Duration};
use url::Url;

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RobotResolution {
    robot: Option<DatasetRobot>,
}

pub(super) struct RobotVisualization<'a> {
    storage: &'a StorageManager,
    episode: EpisodeSelection,
    client: reqwest::Client,
}

impl<'a> RobotVisualization<'a> {
    fn new(storage: &'a StorageManager, episode: EpisodeSelection) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() < 5 && Self::allowed(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            storage,
            episode,
            client,
        })
    }

    pub(super) async fn analyze_import(
        storage: &'a StorageManager,
        import: &HuggingFaceDatasetInspection,
    ) -> Result<(), String> {
        let selection = DatasetSelection::new(vec![format!("dataset_{}", import.dataset_id())])?;
        let dataset = storage.inspect_dataset(selection.clone()).await?;
        let metadata = storage
            .query(
                &format!(
                    "SELECT id FROM spql.mcap.episodes('{}') LIMIT 1",
                    selection.sql_name().replace('\'', "''")
                ),
                1,
            )
            .await?;
        let row = metadata
            .rows()
            .first()
            .ok_or("Imported dataset has no episodes")?;
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Episode has no identity")?;
        let resolver = Self::new(storage, EpisodeSelection::new(selection, id.into()))?;
        let robot = resolver.infer(import).await?;
        let dataset = dataset.dataset();
        storage
            .save_dataset_robot(
                (dataset.resource_id(), dataset.version_id()),
                robot.as_ref(),
            )
            .await
    }

    async fn infer(
        &self,
        import: &HuggingFaceDatasetInspection,
    ) -> Result<Option<DatasetRobot>, String> {
        let repository = import.repository();
        let revision = import.resolved_revision();
        let readme =
            format!("https://huggingface.co/datasets/{repository}/resolve/{revision}/README.md");
        let topics = self.storage.episode_topics(self.episode.clone()).await?;
        let provider = OpenAiProvider::new(
            ProviderConfig::from_env()?
                .ok_or("Robot visualization provider is not configured")?
                .for_terra(),
        )?;
        let tools = Self::tools();
        let mut messages = vec![
            ProviderMessage::text("system", include_str!("robot_visualization_prompt.txt")),
            ProviderMessage::text("user", json!({"description_url": readme, "repository_files_url": format!("https://huggingface.co/api/datasets/{repository}/tree/{revision}"), "episodes": import.files(), "streams": topics}).to_string()),
        ];
        for _ in 0..10 {
            let response = provider.call_tools(&messages, &tools).await?;
            if response.tool_calls().is_empty() {
                return Err("Robot discovery did not produce a result".into());
            }
            messages.push(ProviderMessage::assistant(
                Some(response.content().into()),
                response.tool_calls().to_vec(),
            ));
            for call in response.tool_calls() {
                let function = call.function();
                let arguments = serde_json::from_str::<Value>(function.arguments())
                    .map_err(|error| error.to_string())?;
                let result = match function.name() {
                    "read_description" => match arguments.get("url").and_then(Value::as_str) {
                        Some(url) => self.read_text(url).await.map(Value::String),
                        None => Err("A source URL is required".into()),
                    },
                    "inspect_stream" => self.stream_evidence(&topics, &arguments).await,
                    "finish" if arguments.get("robot").is_none() => {
                        Err("The robot result is required; use null for an absent model".into())
                    }
                    "finish" => match serde_json::from_value::<RobotResolution>(arguments) {
                        Ok(RobotResolution { robot: None }) => return Ok(None),
                        Ok(RobotResolution { robot: Some(robot) }) => {
                            match self.prepare(robot, &topics).await {
                                Ok(robot) => return Ok(Some(robot)),
                                Err(error) => Err(error),
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

    async fn stream_evidence(
        &self,
        topics: &[DatasetTopic],
        arguments: &Value,
    ) -> Result<Value, String> {
        let mut matching = topics.iter().filter(|topic| {
            Some(topic.name()) == arguments.get("topic").and_then(Value::as_str)
                && topic.schemas().iter().any(|schema| {
                    Some(schema.as_str()) == arguments.get("schema").and_then(Value::as_str)
                })
        });
        let topic = matching.next().ok_or("Unknown topic/schema")?;
        if matching.next().is_some() {
            return Err("Joint stream has ambiguous schema revisions".into());
        }
        if topic
            .schemas()
            .iter()
            .any(|schema| schema == "foxglove.CompressedVideo")
        {
            return Err("Choose a joint data stream".into());
        }
        let revision = topic.revision().ok_or("Missing topic revision")?;
        let sql = format!(
            "SELECT value FROM spql.mcap.read_topic('{}', '{}', '{}') WHERE id = $1 ORDER BY log_time, chunk_start_offset, sample_ordinal LIMIT 3",
            self.episode.dataset().sql_name().replace('\'', "''"),
            topic.name().replace('\'', "''"),
            revision.variant().replace('\'', "''")
        );
        let result = self
            .storage
            .query_for_episode(&EpisodeQueryRequest::new(self.episode.clone(), sql, 3))
            .await?;
        let evidence = json!({"schema": result.columns(), "samples": result.rows()});
        if evidence.to_string().len() > 128 * 1024 {
            return Err("Stream samples are too large for robot discovery".into());
        }
        Ok(evidence)
    }

    async fn prepare(
        &self,
        mut robot: DatasetRobot,
        topics: &[DatasetTopic],
    ) -> Result<DatasetRobot, String> {
        robot.validate()?;
        let mut bytes = BTreeMap::new();
        let mut total = 0;
        for filename in RobotModel::mesh_files(robot.urdf())? {
            let data = self.read_bytes(&filename).await?;
            total += data.len();
            if total > 32 * 1024 * 1024 {
                return Err("Robot meshes exceed 32 MiB".into());
            }
            bytes.insert(filename, data);
        }
        let model = RobotModel::parse(robot.urdf(), &bytes)?;
        let expected = model
            .independent_joints()
            .collect::<std::collections::BTreeSet<_>>();
        let mapped = robot
            .bindings()
            .iter()
            .map(|binding| binding.joint())
            .collect();
        if expected != mapped {
            return Err("Bindings must match the model's independently moving joints".into());
        }
        let mut samples = BTreeMap::new();
        for binding in robot.bindings() {
            let key = (binding.topic().to_owned(), binding.schema().to_owned());
            if !samples.contains_key(&key) {
                let arguments = json!({"topic":binding.topic(),"schema":binding.schema()});
                samples.insert(key.clone(), self.stream_evidence(topics, &arguments).await?);
            }
            if !samples[&key]["samples"].as_array().is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.get("value")
                        .and_then(|value| binding.read_position(value))
                        .is_some()
                })
            }) {
                return Err(format!(
                    "Binding for {} does not read a numeric position from the supplied samples",
                    binding.joint()
                ));
            }
        }
        robot.set_meshes(
            bytes
                .into_iter()
                .map(|(name, bytes)| {
                    (
                        name,
                        base64::engine::general_purpose::STANDARD.encode(bytes),
                    )
                })
                .collect(),
        );
        Ok(robot)
    }

    async fn read_text(&self, url: &str) -> Result<String, String> {
        let bytes = self.read_bytes(url).await?;
        if bytes.len() > 512 * 1024 {
            return Err("Description file exceeds 512 KiB".into());
        }
        String::from_utf8(bytes).map_err(|_| "Source is not a text description".into())
    }

    async fn read_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let url = Url::parse(url).map_err(|error| error.to_string())?;
        if !Self::allowed(&url) {
            return Err("Use a public Hugging Face or GitHub source URL".into());
        }
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let mut response = response
            .error_for_status()
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
            if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
                return Err("Robot asset exceeds 32 MiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    fn tools() -> Vec<Value> {
        // Require an explicit result while preserving null as a valid answer.
        let mut result = schemars::schema_for!(RobotResolution);
        result.insert("required".into(), json!(["robot"]));
        vec![
            json!({"type":"function", "function":{"name":"read_description", "description":"Read a public robot description, included file, dataset documentation, or repository file listing. Follow only references supplied by the dataset or its linked model.", "parameters":{"type":"object", "properties":{"url":{"type":"string"}}, "required":["url"], "additionalProperties":false}}}),
            json!({"type":"function", "function":{"name":"inspect_stream", "description":"Read the Arrow schema and three actual messages of an episode stream before mapping it.", "parameters":{"type":"object", "properties":{"topic":{"type":"string"},"schema":{"type":"string"}}, "required":["topic","schema"], "additionalProperties":false}}}),
            json!({"type":"function", "function":{"name":"finish", "description":"Save the resolved visualization, or null when the dataset supplies no usable robot description. Meshes are fetched by the backend; leave meshes empty.", "parameters":result}}),
        ]
    }
}

impl RobotVisualization<'_> {
    fn allowed(url: &Url) -> bool {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && url.host_str().is_some_and(|host| {
                matches!(
                    host,
                    "huggingface.co"
                        | "raw.githubusercontent.com"
                        | "api.github.com"
                        | "github.com"
                ) || host.ends_with(".hf.co")
                    || host.ends_with(".huggingface.co")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_fetches_stay_on_public_model_hosts() {
        for uri in [
            "http://huggingface.co/model",
            "https://localhost/model",
            "https://127.0.0.1/model",
            "https://huggingface.co.evil.test/model",
            "https://user:secret@github.com/model",
            "https://github.com:8000/model",
        ] {
            assert!(
                !RobotVisualization::allowed(&Url::parse(uri).unwrap()),
                "{uri}"
            );
        }
        assert!(RobotVisualization::allowed(
            &Url::parse("https://raw.githubusercontent.com/team/model/commit/arm.urdf").unwrap()
        ));
    }

    #[test]
    fn finish_schema_keeps_referenced_definitions_at_its_root() {
        let tools = RobotVisualization::tools();
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
