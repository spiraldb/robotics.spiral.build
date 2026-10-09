//! Browser-owned robot discovery: the robot description, mesh fetching and
//! validation, over the shared discovery loop. The backend only persists the
//! result.
#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

use super::DatasetClient;
use super::discovery::{DiscoveryEpisode, DiscoveryTask, ToolTurn};
use base64::Engine as _;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::{
    datasets::{DatasetRobot, DatasetRobotSave, DatasetSelection, HuggingFaceDatasetInspection},
    providers::ProviderMessage,
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
        let mut request =
            DiscoveryEpisode::documentation_at(import.repository(), import.resolved_revision());
        request["episodes"] = json!(import.files());
        request["streams"] = json!(episode.topics());
        let messages = vec![
            ProviderMessage::text("system", include_str!("robot_discovery_prompt.txt")),
            ProviderMessage::text("user", request.to_string()),
        ];
        episode.converse(self, messages).await
    }

    async fn save(
        &self,
        episode: &DiscoveryEpisode,
        robot: Option<DatasetRobot>,
    ) -> Result<(), String> {
        let robot = match robot {
            Some(robot) => Some(self.prepare(episode, robot).await?),
            None => None,
        };
        self.data
            .save_dataset_robot(&DatasetRobotSave::new(episode.dataset().clone(), robot))
            .await
            .map_err(|error| error.to_string())
    }

    /// Fetch the robot's meshes and check that it loads and that every binding
    /// reads a position from the episode.
    async fn prepare(
        &self,
        episode: &DiscoveryEpisode,
        mut robot: DatasetRobot,
    ) -> Result<DatasetRobot, String> {
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
                Entry::Vacant(entry) => {
                    entry.insert(episode.stream_evidence(key.0, Some(key.1)).await?)
                }
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

impl DiscoveryTask for RobotDiscoveryAgent {
    const NAME: &'static str = "Robot discovery";
    type Outcome = ();

    fn tools() -> Vec<Value> {
        let mut result = schemars::schema_for!(RobotResolution);
        result.insert("required".into(), json!(["robot"]));
        vec![
            json!({"type":"function", "function":{"name":"finish", "description":"Save the resolved visualization, or null when the dataset supplies no usable robot description. Meshes are fetched when saving; leave meshes empty.", "parameters":result}}),
        ]
    }

    async fn answer(
        &self,
        episode: &DiscoveryEpisode,
        name: &str,
        arguments: Value,
    ) -> ToolTurn<()> {
        ToolTurn::Answer(match name {
            "finish" if arguments.get("robot").is_none() => {
                Err("The robot result is required; use null for an absent model".into())
            }
            "finish" => match serde_json::from_value::<RobotResolution>(arguments) {
                Ok(resolution) => match self.save(episode, resolution.robot).await {
                    Ok(()) => return ToolTurn::Finished(()),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error.to_string()),
            },
            _ => Err("Unknown robot discovery tool".into()),
        })
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn finish_schema_requires_an_explicit_nullable_result() {
        let tools = RobotDiscoveryAgent::tools();
        let schema = &tools[0]["function"]["parameters"];
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
            assert!(!RobotDiscoveryAgent::public_model_host(url), "{url}");
        }
        assert!(RobotDiscoveryAgent::public_model_host(
            "https://raw.githubusercontent.com/team/model/commit/arm.urdf"
        ));
    }
}
