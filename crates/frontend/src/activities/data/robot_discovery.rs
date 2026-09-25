//! Browser-owned robot discovery: the model loop, stream evidence, mesh
//! fetching, and validation. The backend only persists the result.
#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

use super::DatasetClient;
use crate::client::completions::ModelRequest;
use base64::Engine as _;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::{
    datasets::{
        DatasetRobot, DatasetRobotSave, DatasetSelection, DatasetTopic, EpisodeId,
        EpisodeSelection, HuggingFaceDatasetInspection,
    },
    providers::ProviderMessage,
    relations::EpisodeQueryRequest,
    sql::SqlLiteral,
};
use spiral_robotics_viewport::RobotModel;
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

/// Mesh bytes one robot may carry, across all of its files.
const MAX_MESH_BYTES: usize = 32 * 1024 * 1024;

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
        let episode = DiscoveryEpisode::open(self.data.clone(), selection).await?;
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
                    "streams": episode.topics(),
                })
                .to_string(),
            ),
        ];
        let tools = Self::tools();
        for _ in 0..10 {
            let response = crate::client::completions::ModelClient::complete(
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
                    "inspect_stream" => Self::inspect_stream(&episode, &arguments).await,
                    "finish" if arguments.get("robot").is_none() => {
                        Err("The robot result is required; use null for an absent model".into())
                    }
                    "finish" => match serde_json::from_value::<RobotResolution>(arguments) {
                        Ok(resolution) => match self.save(&episode, resolution.robot).await {
                            Ok(()) => return Ok(()),
                            Err(error) => Err(error),
                        },
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
        episode: &DiscoveryEpisode,
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
        episode.stream_evidence(topic, schema).await
    }

    async fn save(
        &self,
        episode: &DiscoveryEpisode,
        robot: Option<DatasetRobot>,
    ) -> Result<(), String> {
        let robot = match robot {
            Some(robot) => Some(episode.prepare(robot).await?),
            None => None,
        };
        self.data
            .save_dataset_robot(&DatasetRobotSave::new(episode.dataset().clone(), robot))
            .await
            .map_err(|error| error.to_string())
    }

    fn tools() -> Vec<Value> {
        let mut result = schemars::schema_for!(RobotResolution);
        result.insert("required".into(), json!(["robot"]));
        vec![
            json!({"type":"function", "function":{"name":"read_description", "description":"Read a public robot description, included file, dataset documentation, or repository file listing. Follow only references supplied by the dataset or its linked model.", "parameters":{"type":"object", "properties":{"url":{"type":"string"}}, "required":["url"], "additionalProperties":false}}}),
            json!({"type":"function", "function":{"name":"inspect_stream", "description":"Read the Arrow schema and three actual messages of an episode stream before mapping it.", "parameters":{"type":"object", "properties":{"topic":{"type":"string"},"schema":{"type":"string"}}, "required":["topic","schema"], "additionalProperties":false}}}),
            json!({"type":"function", "function":{"name":"finish", "description":"Save the resolved visualization, or null when the dataset supplies no usable robot description. Meshes are fetched when saving; leave meshes empty.", "parameters":result}}),
        ]
    }
}

/// The imported episode discovery reads: its streams, a few of their
/// messages, and the checks a resolved robot must pass against them.
struct DiscoveryEpisode {
    data: DatasetClient,
    episode: EpisodeSelection,
    topics: Vec<DatasetTopic>,
}

impl DiscoveryEpisode {
    async fn open(data: DatasetClient, selection: DatasetSelection) -> Result<Self, String> {
        let first = data
            .run_spiral_query(
                format!(
                    "SELECT id FROM spql.mcap.episodes({}) LIMIT 1",
                    SqlLiteral::text(&selection.sql_name())
                ),
                1,
            )
            .await
            .map_err(|error| error.to_string())?;
        let id = first
            .rows()
            .first()
            .ok_or("Imported dataset has no episodes")?
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Episode has no identity")?;
        let episode = EpisodeSelection::new(selection, EpisodeId::try_from(id)?);
        let topics = data
            .episode_topics(episode.clone())
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self {
            data,
            episode,
            topics,
        })
    }

    fn topics(&self) -> &[DatasetTopic] {
        &self.topics
    }

    fn dataset(&self) -> &DatasetSelection {
        self.episode.dataset()
    }

    /// The Arrow schema and first three messages of one joint stream.
    async fn stream_evidence(&self, topic: &str, schema: &str) -> Result<Value, String> {
        let mut matching = self.topics.iter().filter(|candidate| {
            candidate.name() == topic && candidate.schemas().iter().any(|name| name == schema)
        });
        let found = matching.next().ok_or("Unknown topic/schema")?;
        if matching.next().is_some() {
            return Err("Joint stream has ambiguous schema revisions".into());
        }
        if found
            .schemas()
            .iter()
            .any(|name| name == "foxglove.CompressedVideo")
        {
            return Err("Choose a joint data stream".into());
        }
        let revision = found.revision().ok_or("Missing topic revision")?;
        let sql = format!(
            "SELECT value FROM spql.mcap.read_topic({}, {}, {}) WHERE id = $1 ORDER BY log_time, chunk_start_offset, sample_ordinal LIMIT 3",
            SqlLiteral::text(&self.dataset().sql_name()),
            SqlLiteral::text(found.name()),
            SqlLiteral::text(revision.variant())
        );
        let result = self
            .data
            .run_episode_query(&EpisodeQueryRequest::new(self.episode.clone(), sql, 3))
            .await
            .map_err(|error| error.to_string())?;
        let evidence = json!({"schema": result.columns(), "samples": result.rows()});
        if evidence.to_string().len() > 128 * 1024 {
            return Err("Stream samples are too large for robot discovery".into());
        }
        Ok(evidence)
    }

    /// Fetch the robot's meshes and check that it loads and that every binding
    /// reads a position from this episode.
    async fn prepare(&self, mut robot: DatasetRobot) -> Result<DatasetRobot, String> {
        robot.validate()?;
        let meshes = self.meshes(robot.urdf()).await?;
        let model = RobotModel::parse(robot.urdf(), &meshes)?;
        let expected = model.independent_joints().collect::<BTreeSet<_>>();
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
            let key = (binding.topic(), binding.schema());
            let evidence = match samples.entry(key) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(self.stream_evidence(key.0, key.1).await?),
            };
            if !evidence["samples"].as_array().is_some_and(|rows| {
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
            meshes
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

    async fn meshes(&self, urdf: &str) -> Result<BTreeMap<String, Vec<u8>>, String> {
        let mut meshes = BTreeMap::new();
        let mut total = 0;
        for filename in RobotModel::mesh_files(urdf)? {
            if !Self::public_model_host(&filename) {
                return Err("Use a public Hugging Face or GitHub source URL".into());
            }
            let bytes = self
                .data
                .read_robot_asset(&filename, MAX_MESH_BYTES - total)
                .await
                .map_err(|error| format!("Reading {filename}: {error}"))?;
            total += bytes.len();
            meshes.insert(filename, bytes);
        }
        Ok(meshes)
    }

    /// Meshes come only from public HTTPS model hosts on the default port.
    fn public_model_host(url: &str) -> bool {
        let Some(rest) = url.strip_prefix("https://") else {
            return false;
        };
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        matches!(
            host,
            "huggingface.co" | "raw.githubusercontent.com" | "api.github.com" | "github.com"
        ) || host.ends_with(".hf.co")
            || host.ends_with(".huggingface.co")
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

    #[wasm_bindgen_test]
    fn meshes_stay_on_public_model_hosts() {
        for url in [
            "http://huggingface.co/model",
            "https://localhost/model",
            "https://127.0.0.1/model",
            "https://huggingface.co.evil.test/model",
            "https://user:secret@github.com/model",
            "https://github.com:8000/model",
        ] {
            assert!(!DiscoveryEpisode::public_model_host(url), "{url}");
        }
        assert!(DiscoveryEpisode::public_model_host(
            "https://raw.githubusercontent.com/team/model/commit/arm.urdf"
        ));
    }
}
