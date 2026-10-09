//! What the browser-owned discovery agents share: one imported episode to
//! read streams from, and the model loop that answers the agent's tools until
//! it finishes with a result that passes validation. The backend only
//! persists each agent's result.
use super::DatasetClient;
use crate::client::completions::{ModelClient, ModelRequest};
use serde_json::{Value, json};
use spiral_robotics_common::{
    datasets::{DatasetSelection, DatasetTopic, EpisodeId, EpisodeSelection},
    providers::ProviderMessage,
    relations::EpisodeQueryRequest,
    sql::SqlLiteral,
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
/// of their messages, where it was imported from, and the checks a result
/// must pass against them.
pub(super) struct DiscoveryEpisode {
    data: DatasetClient,
    episode: EpisodeSelection,
    source: Option<String>,
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
                    "SELECT id, source.uri AS source_uri FROM spql.mcap.episodes({}) LIMIT 1",
                    SqlLiteral::text(&selection.sql_name())
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
            .and_then(Value::as_str)
            .ok_or("Episode has no identity")?;
        let source = row
            .get("source_uri")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let episode = EpisodeSelection::new(selection, EpisodeId::try_from(id)?);
        let topics = data
            .episode_topics(episode.clone())
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self {
            data,
            episode,
            source,
            topics,
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
        let mut matching = self
            .topics
            .iter()
            .filter(|candidate| candidate.is_recorded_as(topic, schema));
        let found = matching.next().ok_or("Unknown topic/schema")?;
        if matching.next().is_some() {
            return Err("Joint stream has ambiguous schema revisions".into());
        }
        if found.is_video() {
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
            return Err("Stream samples are too large for discovery".into());
        }
        Ok(evidence)
    }
}
