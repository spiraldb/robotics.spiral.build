//! Spiral dataset discovery and bounded topic inspection.
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetSelection {
    path: Vec<String>,
}
impl DatasetSelection {
    pub fn new(path: Vec<String>) -> Result<Self, String> {
        if path.is_empty()
            || path.len() > 32
            || path.iter().any(|part| {
                part.is_empty() || part.len() > 256 || part.chars().any(char::is_control)
            })
        {
            return Err("Dataset path must contain 1–32 nonempty names".to_owned());
        }
        Ok(Self { path })
    }
    pub fn path(&self) -> &[String] {
        &self.path
    }
    pub fn label(&self) -> String {
        self.path.join(".")
    }
    pub fn sql_name(&self) -> String {
        self.label()
    }
    pub fn can_preview(&self) -> bool {
        self.path.iter().all(|part| !part.contains('.'))
    }
    pub fn episodes_sql(&self) -> String {
        format!(
            "SELECT * FROM spql.mcap.episodes('{}') LIMIT 100",
            self.sql_name().replace('\'', "''")
        )
    }
    pub fn topic_sql(&self, topic: &str) -> String {
        format!(
            "SELECT * FROM spql.mcap.read_topic('{}', '{}') LIMIT 100",
            self.sql_name().replace('\'', "''"),
            topic.replace('\'', "''")
        )
    }
}

/// One native recording within a dataset.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeSelection {
    dataset: DatasetSelection,
    episode_id: String,
}
impl EpisodeSelection {
    pub fn new(dataset: DatasetSelection, episode_id: String) -> Self {
        Self {
            dataset,
            episode_id,
        }
    }
    pub fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub fn episode_id(&self) -> &str {
        &self.episode_id
    }
}

/// One topic scoped to a native recording ID.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeTopicRequest {
    dataset: DatasetSelection,
    topic: String,
    episode_id: String,
}
impl EpisodeTopicRequest {
    pub fn new(dataset: DatasetSelection, topic: String, episode_id: String) -> Self {
        Self {
            dataset,
            topic,
            episode_id,
        }
    }
    pub fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub fn topic(&self) -> &str {
        &self.topic
    }
    pub fn episode_id(&self) -> &str {
        &self.episode_id
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetSummary {
    selection: DatasetSelection,
    resource_id: String,
    version_id: String,
}
impl DatasetSummary {
    pub fn new(selection: DatasetSelection, identity: (String, String)) -> Self {
        Self {
            selection,
            resource_id: identity.0,
            version_id: identity.1,
        }
    }
    pub fn selection(&self) -> &DatasetSelection {
        &self.selection
    }
    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }
    pub fn version_id(&self) -> &str {
        &self.version_id
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetTopic {
    name: String,
    schemas: Vec<String>,
    members: u64,
    samples: Option<u64>,
    #[serde(default)]
    revision: Option<super::TopicRevision>,
}
impl DatasetTopic {
    pub fn new(name: String, schemas: Vec<String>) -> Self {
        Self {
            name,
            schemas,
            members: 0,
            samples: None,
            revision: None,
        }
    }
    pub fn with_coverage(mut self, members: u64, samples: Option<u64>) -> Self {
        self.members = members;
        self.samples = samples;
        self
    }
    pub fn with_revision(mut self, revision: super::TopicRevision) -> Self {
        self.revision = Some(revision);
        self
    }
    pub fn revision(&self) -> Option<&super::TopicRevision> {
        self.revision.as_ref()
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn schemas(&self) -> &[String] {
        &self.schemas
    }
    pub fn members(&self) -> u64 {
        self.members
    }
    pub fn samples(&self) -> Option<u64> {
        self.samples
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetInspection {
    dataset: DatasetSummary,
    members: Option<u64>,
    topics: Vec<DatasetTopic>,
}
impl DatasetInspection {
    pub fn new(dataset: DatasetSummary, members: Option<u64>, topics: Vec<DatasetTopic>) -> Self {
        Self {
            dataset,
            members,
            topics,
        }
    }
    pub fn dataset(&self) -> &DatasetSummary {
        &self.dataset
    }
    pub fn members(&self) -> Option<u64> {
        self.members
    }
    pub fn topics(&self) -> &[DatasetTopic] {
        &self.topics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dataset_paths_and_topics_cannot_escape_preview_sql() {
        let selection =
            DatasetSelection::new(vec!["a.b".into(), "owner's \"data\"".into()]).unwrap();
        assert!(!selection.can_preview());
        assert!(
            selection
                .topic_sql("/camera' --")
                .contains("'/camera'' --'")
        );
        assert!(selection.episodes_sql().contains("owner''s"));
        assert!(DatasetSelection::new(vec![]).is_err());
        assert!(DatasetSelection::new(vec!["\n".into()]).is_err());
    }
}

/// A real resource at its catalog path; parentage comes from Spiral's namespace.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectResource {
    selection: DatasetSelection,
    kind: String,
    identity: (String, String),
}
impl ProjectResource {
    pub fn new(selection: DatasetSelection, kind: String, identity: (String, String)) -> Self {
        Self {
            selection,
            kind,
            identity,
        }
    }
    pub fn selection(&self) -> &DatasetSelection {
        &self.selection
    }
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn resource_id(&self) -> &str {
        &self.identity.0
    }
    pub fn dataset(&self) -> Option<DatasetSummary> {
        (self.kind == "dataset")
            .then(|| DatasetSummary::new(self.selection.clone(), self.identity.clone()))
    }
    pub fn table_sql(&self) -> String {
        let name = self
            .selection
            .path()
            .iter()
            .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(".");
        format!("SELECT * FROM {name} LIMIT 100")
    }
}

/// Encoded Annex B packets; nanoseconds travel as strings to preserve integer precision in JS.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeVideo {
    codec: String,
    packets: Vec<VideoPacket>,
}
impl EpisodeVideo {
    pub fn new(codec: String, packets: Vec<VideoPacket>) -> Self {
        Self { codec, packets }
    }
    pub fn codec(&self) -> &str {
        &self.codec
    }
    pub fn packets(&self) -> &[VideoPacket] {
        &self.packets
    }

    pub fn relative_to(mut self, origin_ns: u64) -> Result<Self, String> {
        for packet in &mut self.packets {
            let timestamp = packet
                .timestamp_ns
                .parse::<u64>()
                .map_err(|_| "Invalid video timestamp")?;
            packet.timestamp_ns = timestamp
                .checked_sub(origin_ns)
                .ok_or("Camera timestamp precedes recording start")?
                .to_string();
        }
        Ok(self)
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoPacket {
    timestamp_ns: String,
    data: String,
    keyframe: bool,
}
impl VideoPacket {
    pub fn timestamp_ns(&self) -> &str {
        &self.timestamp_ns
    }
    pub fn data(&self) -> &str {
        &self.data
    }
    pub fn keyframe(&self) -> bool {
        self.keyframe
    }
    pub fn new(timestamp_ns: u64, data: String, keyframe: bool) -> Self {
        Self {
            timestamp_ns: timestamp_ns.to_string(),
            data,
            keyframe,
        }
    }
}
