//! Spiral dataset discovery and bounded topic inspection.
use super::EpisodeId;
use serde::{Deserialize, Serialize};

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
    /// What an operator calls the dataset: its own name, without the
    /// `dataset_` prefix every native Studio Dataset carries in SQL.
    pub fn display_name(&self) -> String {
        let name = self.path.last().map_or("", String::as_str);
        name.strip_prefix("dataset_")
            .filter(|rest| !rest.is_empty())
            .unwrap_or(name)
            .to_owned()
    }
    pub fn can_preview(&self) -> bool {
        self.path.iter().all(|part| !part.contains('.'))
    }
}

/// One native recording within a dataset.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeSelection {
    dataset: DatasetSelection,
    episode_id: EpisodeId,
}
impl EpisodeSelection {
    pub fn new(dataset: DatasetSelection, episode_id: EpisodeId) -> Self {
        Self {
            dataset,
            episode_id,
        }
    }
    pub fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub fn episode_id(&self) -> &EpisodeId {
        &self.episode_id
    }
}

/// One topic scoped to a native recording ID.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeTopicRequest {
    dataset: DatasetSelection,
    topic: String,
    episode_id: EpisodeId,
}
impl EpisodeTopicRequest {
    pub fn new(dataset: DatasetSelection, topic: String, episode_id: EpisodeId) -> Self {
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
    pub fn episode_id(&self) -> &EpisodeId {
        &self.episode_id
    }
}

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
    /// Whether the stream carries camera video rather than data messages.
    pub fn is_video(&self) -> bool {
        self.schemas
            .iter()
            .any(|schema| schema == "foxglove.CompressedVideo")
    }
    pub fn members(&self) -> u64 {
        self.members
    }
    pub fn samples(&self) -> Option<u64> {
        self.samples
    }
}

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
    fn dataset_paths_validate_names_and_preview_support() {
        let selection =
            DatasetSelection::new(vec!["a.b".into(), "owner's \"data\"".into()]).unwrap();
        assert!(!selection.can_preview());
        assert!(DatasetSelection::new(vec![]).is_err());
        assert!(DatasetSelection::new(vec!["\n".into()]).is_err());
    }

    #[test]
    fn a_dataset_is_called_by_its_own_name() {
        let named = |name: &str| {
            DatasetSelection::new(vec![name.into()])
                .unwrap()
                .display_name()
        };
        assert_eq!(named("dataset_abc130k_sample"), "abc130k_sample");
        assert_eq!(named("recordings"), "recordings");
        assert_eq!(named("dataset_"), "dataset_");
    }
}

/// A real resource at its catalog path; parentage comes from Spiral's namespace.
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
}

/// Encoded Annex B packets.
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
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoPacket {
    timestamp_ns: u64,
    data: String,
    keyframe: bool,
}
impl VideoPacket {
    pub const fn timestamp_ns(&self) -> u64 {
        self.timestamp_ns
    }
    pub fn data(&self) -> &str {
        &self.data
    }
    pub fn keyframe(&self) -> bool {
        self.keyframe
    }
    pub fn new(timestamp_ns: u64, data: String, keyframe: bool) -> Self {
        Self {
            timestamp_ns,
            data,
            keyframe,
        }
    }
}
