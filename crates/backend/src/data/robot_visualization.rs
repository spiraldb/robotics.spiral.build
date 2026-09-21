//! Native network, dataset inspection, validation, and persistence for robot discovery.
use base64::Engine as _;
use serde_json::{Value, json};
use spiral_robotics_common::{
    datasets::{
        DatasetRobot, DatasetSelection, DatasetTopic, EpisodeSelection, RobotDiscoveryContext,
        RobotDiscoveryResult, RobotStreamInspectionRequest,
    },
    relations::EpisodeQueryRequest,
};
use spiral_robotics_storage::StorageManager;
use spiral_robotics_viewport::RobotModel;
use std::{collections::BTreeMap, time::Duration};
use url::Url;

pub(super) struct RobotVisualization<'a> {
    storage: &'a StorageManager,
    episode: EpisodeSelection,
    client: reqwest::Client,
}

impl<'a> RobotVisualization<'a> {
    fn new(storage: &'a StorageManager, episode: EpisodeSelection) -> Result<Self, String> {
        Ok(Self {
            storage,
            episode,
            client: Self::client()?,
        })
    }

    fn client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() < 5 && Self::allowed(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map_err(|error| error.to_string())
    }

    pub(super) async fn context(
        storage: &'a StorageManager,
        selection: DatasetSelection,
    ) -> Result<RobotDiscoveryContext, String> {
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
        let episode = EpisodeSelection::new(selection, id.into());
        let topics = storage.episode_topics(episode.clone()).await?;
        Ok(RobotDiscoveryContext::new(episode, topics))
    }

    pub(super) async fn inspect_stream(
        storage: &'a StorageManager,
        request: RobotStreamInspectionRequest,
    ) -> Result<Value, String> {
        let resolver = Self::new(storage, request.episode().clone())?;
        let topics = storage.episode_topics(request.episode().clone()).await?;
        resolver
            .stream_evidence(&topics, request.topic(), request.schema())
            .await
    }

    pub(super) async fn save(
        storage: &'a StorageManager,
        result: RobotDiscoveryResult,
    ) -> Result<(), String> {
        let (episode, robot) = result.into_parts();
        let selection = episode.dataset().clone();
        let inspection = storage.inspect_dataset(selection).await?;
        let dataset = inspection.dataset();
        let robot = match robot {
            Some(robot) => {
                let resolver = Self::new(storage, episode.clone())?;
                let topics = storage.episode_topics(episode).await?;
                Some(resolver.prepare(robot, &topics).await?)
            }
            None => None,
        };
        storage
            .save_dataset_robot(
                (dataset.resource_id(), dataset.version_id()),
                robot.as_ref(),
            )
            .await
    }

    async fn stream_evidence(
        &self,
        topics: &[DatasetTopic],
        requested_topic: &str,
        requested_schema: &str,
    ) -> Result<Value, String> {
        let mut matching = topics.iter().filter(|topic| {
            topic.name() == requested_topic
                && topic
                    .schemas()
                    .iter()
                    .any(|schema| schema == requested_schema)
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
            let data = Self::read_bytes(&self.client, &filename).await?;
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
                samples.insert(
                    key.clone(),
                    self.stream_evidence(topics, binding.topic(), binding.schema())
                        .await?,
                );
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

    async fn read_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
        let url = Url::parse(url).map_err(|error| error.to_string())?;
        if !Self::allowed(&url) {
            return Err("Use a public Hugging Face or GitHub source URL".into());
        }
        let response = client
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
}
