//! What the browser-owned discovery agents share: one imported episode to
//! read streams from, and the model loop that answers the agent's tools until
//! it finishes with a result that passes validation. The backend only
//! persists each agent's result.
use super::DatasetClient;
use crate::client::completions::{ModelClient, ModelRequest};
use serde_json::{Value, json};
use spiral_robotics_common::{
    datasets::{
        DatasetFormat, DatasetSelection, DatasetStreams, DatasetTopic, EpisodeId, EpisodeSelection,
        EpisodeStream,
    },
    providers::ProviderMessage,
    relations::EpisodeQueryRequest,
};

/// How many model turns an agent gets before it is given up on.
const TURNS: usize = 10;

/// What one tool call came to: an answer for the model, or the agent's
/// result, validated and saved.
pub(super) enum ToolTurn<T> {
    Answer(Result<Value, String>),
    Finished(T),
}

/// One discovery: the tools it offers beside `inspect_stream` and
/// `read_description`, and how it answers them. `finish` validates and saves the result; a rejected result
/// goes back to the model as an actionable error.
pub(super) trait DiscoveryTask {
    /// What the task is called in its failures.
    const NAME: &'static str;
    /// What a finished discovery hands back once saved.
    type Outcome;
    fn tools() -> Vec<Value>;
    async fn answer(
        &self,
        episode: &DiscoveryEpisode,
        name: &str,
        arguments: Value,
    ) -> ToolTurn<Self::Outcome>;
}

/// The first episode of a dataset, which discovery reads: its streams, a few
/// of their messages, where it was imported from and in which format, and
/// the checks a result must pass against them.
pub(super) struct DiscoveryEpisode {
    data: DatasetClient,
    episode: EpisodeSelection,
    source: Option<String>,
    format: DatasetFormat,
    topics: Vec<DatasetTopic>,
}

impl DiscoveryEpisode {
    pub(super) async fn open(
        data: DatasetClient,
        selection: DatasetSelection,
    ) -> Result<Self, String> {
        let first = data
            .run_spiral_query(
                format!(
                    "SELECT id, source_uri, streams FROM {} LIMIT 1",
                    selection.episodes()
                ),
                1,
            )
            .await
            .map_err(|error| error.to_string())?;
        let row = first
            .rows()
            .first()
            .ok_or("Imported dataset has no episodes")?;
        let id = row
            .get("id")
            .and_then(Value::as_u64)
            .map(EpisodeId::new)
            .ok_or("Episode has no identity")?;
        let source = row
            .get("source_uri")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let streams = row
            .get("streams")
            .cloned()
            .map(serde_json::from_value::<Vec<EpisodeStream>>)
            .ok_or("Episode lists no streams")?
            .map_err(|error| format!("Reading the episode's streams: {error}"))?;
        let inspection = data
            .inspect_dataset(selection.clone())
            .await
            .map_err(|error| error.to_string())?;
        let format = inspection.format();
        let topics = inspection.episode_topics(&streams)?;
        let episode = EpisodeSelection::new(selection, id);
        Ok(Self {
            data,
            episode,
            source,
            format,
            topics,
        })
    }

    /// A LeRobot dataset's own description of its data features: each
    /// one's dtype, shape and element names from `meta/info.json`, beside
    /// its per-element min, max, mean and std over every frame from
    /// `meta/stats.json`. `null` for a dataset not imported as LeRobot from
    /// a Hugging Face repository.
    pub(super) async fn lerobot_features(&self) -> Result<Value, String> {
        let Some(root) = self
            .source
            .as_deref()
            .filter(|_| self.format == DatasetFormat::LeRobot)
            .and_then(Self::lerobot_root)
        else {
            return Ok(Value::Null);
        };
        let read = |name: &str| {
            let url = format!("{root}meta/{name}");
            async move {
                let text = self
                    .data
                    .read_robot_description(&url)
                    .await
                    .map_err(|error| error.to_string())?;
                serde_json::from_str::<Value>(&text).map_err(|error| format!("{url}: {error}"))
            }
        };
        Ok(Self::described_features(
            &read("info.json").await?,
            &read("stats.json").await?,
        ))
    }

    /// Where a LeRobot dataset keeps its `meta/` directory, from the
    /// `hf://datasets/<repository>@<revision>/<root>` repository root its
    /// episodes were read from.
    fn lerobot_root(source: &str) -> Option<String> {
        let (repository, rest) = source.strip_prefix("hf://datasets/")?.split_once('@')?;
        let (revision, root) = rest.split_once('/')?;
        if !(root.is_empty() || root.ends_with('/')) {
            return None;
        }
        Some(format!(
            "https://huggingface.co/datasets/{repository}/resolve/{revision}/{root}"
        ))
    }

    /// Every feature `info` lists that is not a camera, with its statistics
    /// from `stats` beside it.
    fn described_features(info: &Value, stats: &Value) -> Value {
        let features = info["features"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, feature)| !matches!(feature["dtype"].as_str(), Some("video" | "image")))
            .map(|(name, feature)| {
                let mut described = json!({
                    "dtype": feature["dtype"],
                    "shape": feature["shape"],
                    "names": feature["names"],
                });
                for statistic in ["min", "max", "mean", "std"] {
                    described[statistic] = stats[name.as_str()][statistic].clone();
                }
                (name.clone(), described)
            })
            .collect::<serde_json::Map<_, _>>();
        json!({
            "robot_type": info["robot_type"],
            "fps": info["fps"],
            "features": features,
        })
    }

    /// Where the dataset's own documentation is, when its episodes were
    /// imported from a Hugging Face dataset repository.
    pub(super) fn documentation(&self) -> Value {
        let Some((repository, revision)) = self
            .source
            .as_deref()
            .and_then(|source| source.strip_prefix("hf://datasets/"))
            .and_then(|rest| {
                let (repository, rest) = rest.split_once('@')?;
                Some((repository, rest.split('/').next()?))
            })
        else {
            return Value::Null;
        };
        Self::documentation_at(repository, revision)
    }

    /// Where a Hugging Face dataset repository keeps its documentation and
    /// its file listing at `revision`.
    pub(super) fn documentation_at(repository: &str, revision: &str) -> Value {
        json!({
            "description_url": format!("https://huggingface.co/datasets/{repository}/resolve/{revision}/README.md"),
            "repository_files_url": format!("https://huggingface.co/api/datasets/{repository}/tree/{revision}"),
        })
    }

    pub(super) fn topics(&self) -> &[DatasetTopic] {
        &self.topics
    }

    pub(super) fn dataset(&self) -> &DatasetSelection {
        self.episode.dataset()
    }

    /// Talk `task` through to its result: answer each turn's tool calls, and
    /// stop once one of them finishes.
    pub(super) async fn converse<T: DiscoveryTask>(
        &self,
        task: &T,
        mut messages: Vec<ProviderMessage>,
    ) -> Result<T::Outcome, String> {
        let mut tools = vec![
            json!({"type":"function", "function":{"name":"inspect_stream", "description":"Read the Arrow schema and three actual messages of an episode stream before mapping it. A stream recorded without a schema name, such as a LeRobot feature, is named by its topic and a null schema.", "parameters":{"type":"object", "properties":{"topic":{"type":"string"},"schema":{"type":["string","null"]}}, "required":["topic","schema"], "additionalProperties":false}}}),
            json!({"type":"function", "function":{"name":"read_description", "description":"Read a public robot description, included file, dataset documentation, or repository file listing. Follow only references supplied by the dataset or its linked model.", "parameters":{"type":"object", "properties":{"url":{"type":"string"}}, "required":["url"], "additionalProperties":false}}}),
        ];
        tools.extend(T::tools());
        for _ in 0..TURNS {
            let response = ModelClient::complete(ModelRequest::new(
                messages.clone(),
                tools.clone(),
                "gpt-5.6-terra",
            ))
            .await?;
            if response.tool_calls().is_empty() {
                return Err(format!("{} did not produce a result", T::NAME));
            }
            messages.push(response.clone());
            for call in response.tool_calls() {
                let function = call.function();
                let answer = match serde_json::from_str::<Value>(function.arguments()) {
                    Err(error) => Err(error.to_string()),
                    Ok(arguments) if function.name() == "inspect_stream" => {
                        self.inspect_stream(&arguments).await
                    }
                    Ok(arguments) if function.name() == "read_description" => {
                        self.read_description(&arguments).await
                    }
                    Ok(arguments) => match task.answer(self, function.name(), arguments).await {
                        ToolTurn::Finished(outcome) => return Ok(outcome),
                        ToolTurn::Answer(answer) => answer,
                    },
                };
                messages.push(ProviderMessage::tool(
                    call.id().into(),
                    match answer {
                        Ok(value) => value.to_string(),
                        Err(error) => json!({"error": error}).to_string(),
                    },
                ));
            }
        }
        Err(format!("{} ran out of turns", T::NAME))
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

    async fn inspect_stream(&self, arguments: &Value) -> Result<Value, String> {
        let topic = arguments
            .get("topic")
            .and_then(Value::as_str)
            .ok_or("A topic is required")?;
        let schema = arguments.get("schema").and_then(Value::as_str);
        self.stream_evidence(topic, schema).await
    }

    /// The Arrow schema and first three messages of one data stream: the
    /// one `schema` names on `topic`, or with no schema, the topic's only
    /// stream.
    pub(super) async fn stream_evidence(
        &self,
        topic: &str,
        schema: Option<&str>,
    ) -> Result<Value, String> {
        let found = DatasetStreams::new(&self.topics).recorded(topic, schema)?;
        if found.is_video() {
            return Err("Choose a joint data stream".into());
        }
        let sql = format!(
            "SELECT value FROM {} WHERE id = $1 ORDER BY log_time, ordinal LIMIT 3",
            self.dataset().messages(found.revision())
        );
        let result = self
            .data
            .run_episode_query(&EpisodeQueryRequest::new(self.episode.clone(), sql, 3))
            .await
            .map_err(|error| error.to_string())?;
        let evidence = json!({"schema": result.columns(), "samples": result.rows()});
        if evidence.to_string().len() > 128 * 1024 {
            return Err("Stream samples are too large for discovery".into());
        }
        Ok(evidence)
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn a_lerobot_episode_source_leads_to_its_dataset_meta() {
        assert_eq!(
            DiscoveryEpisode::lerobot_root("hf://datasets/org/sim-bottles@897b/").as_deref(),
            Some("https://huggingface.co/datasets/org/sim-bottles/resolve/897b/")
        );
        assert_eq!(
            DiscoveryEpisode::lerobot_root("hf://datasets/org/repo@main/sub/").as_deref(),
            Some("https://huggingface.co/datasets/org/repo/resolve/main/sub/")
        );
        assert_eq!(
            DiscoveryEpisode::lerobot_root("hf://datasets/org/repo@main/a.mcap"),
            None
        );
        assert_eq!(
            DiscoveryEpisode::lerobot_root("s3://bucket/data/chunk-000/f.parquet"),
            None
        );
    }

    #[wasm_bindgen_test]
    fn lerobot_features_keep_data_features_with_their_ranges() {
        let info = json!({"robot_type": "bimanual_sim", "fps": 30, "codebase_version": "v3.0", "features": {
            "state": {"dtype": "float32", "shape": [2], "names": ["state"]},
            "top_camera": {"dtype": "video", "shape": [480, 640, 3], "names": ["height", "width", "channel"]},
        }});
        let stats = json!({
            "state": {"min": [-1.5, 0.0], "max": [1.5, 1.0], "mean": [0.1, 0.6], "std": [0.4, 0.3], "count": [900]},
            "top_camera": {"min": [[[0.0]]], "max": [[[1.0]]]},
        });
        assert_eq!(
            DiscoveryEpisode::described_features(&info, &stats),
            json!({"robot_type": "bimanual_sim", "fps": 30, "features": {
                "state": {"dtype": "float32", "shape": [2], "names": ["state"],
                    "min": [-1.5, 0.0], "max": [1.5, 1.0], "mean": [0.1, 0.6], "std": [0.4, 0.3]},
            }})
        );
    }
}
