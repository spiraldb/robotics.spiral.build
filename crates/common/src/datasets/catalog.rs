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

/// A dataset at one version: what Studio keeps about a dataset, such as its
/// robot and joint roles, is filed under this identity, as the browser read
/// it.
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
    /// The resource and version what Studio keeps about this dataset is
    /// filed under.
    pub fn identity(&self) -> (&str, &str) {
        (&self.resource_id, &self.version_id)
    }
}

/// The Spiral producer that made a Dataset's members from its source files,
/// which decides what `spql.mcap.episodes` lists for each episode and what
/// its topics record. Spiral's `spql.mcap` functions read these two.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum DatasetProducer {
    /// One member per MCAP file, each topic the file's channels.
    #[serde(rename = "spql.mcap.dataset")]
    Mcap,
    /// One member per episode of a LeRobot v3 dataset, each feature a topic.
    #[serde(rename = "spql.lerobot.dataset")]
    LeRobot,
}

impl DatasetProducer {
    /// The producer Spiral registered under `id`, or why Studio cannot read
    /// a Dataset it made.
    pub fn from_id(id: &str) -> Result<Self, String> {
        match id {
            "spql.mcap.dataset" => Ok(Self::Mcap),
            "spql.lerobot.dataset" => Ok(Self::LeRobot),
            other => Err(format!(
                "Studio reads datasets made by spql.mcap.dataset or spql.lerobot.dataset, not {other}"
            )),
        }
    }
    /// The id Spiral registers it under.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Mcap => "spql.mcap.dataset",
            Self::LeRobot => "spql.lerobot.dataset",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetTopic {
    name: String,
    schemas: Vec<String>,
    /// How the producer recorded the topic's messages: `protobuf` for an
    /// MCAP channel, `mp4` for a LeRobot camera, `parquet` for a LeRobot
    /// feature.
    #[serde(default)]
    encodings: Vec<String>,
    members: u64,
    samples: Option<u64>,
    revision: Option<super::TopicRevision>,
}
impl DatasetTopic {
    pub fn new(name: String, schemas: Vec<String>) -> Self {
        Self {
            name,
            schemas,
            encodings: Vec::new(),
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
    /// The message encodings its producer recorded.
    pub fn with_encodings(mut self, encodings: Vec<String>) -> Self {
        self.encodings = encodings;
        self
    }
    pub fn revision(&self) -> Option<&super::TopicRevision> {
        self.revision.as_ref()
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The native schema names its messages were recorded with; a LeRobot
    /// feature has none.
    pub fn schemas(&self) -> &[String] {
        &self.schemas
    }
    /// Whether the stream carries camera video rather than data messages:
    /// recorded as `foxglove.CompressedVideo`, or a LeRobot video feature,
    /// which Spiral keeps as the same H.264 packets.
    pub fn is_video(&self) -> bool {
        self.schemas
            .iter()
            .any(|schema| schema == "foxglove.CompressedVideo")
            || self.encodings.iter().any(|encoding| encoding == "mp4")
    }
    /// Whether it is the stream `schema` names on its topic: the one of that
    /// schema, or, with no schema, the topic's only recording.
    pub fn is_recorded_as(&self, topic: &str, schema: Option<&str>) -> bool {
        self.name == topic
            && match schema {
                Some(schema) => self.schemas.iter().any(|recorded| recorded == schema),
                None => self.schemas.is_empty(),
            }
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
    producer: DatasetProducer,
    members: Option<u64>,
    topics: Vec<DatasetTopic>,
}
impl DatasetInspection {
    pub fn new(
        dataset: DatasetSummary,
        producer: DatasetProducer,
        members: Option<u64>,
        topics: Vec<DatasetTopic>,
    ) -> Self {
        Self {
            dataset,
            producer,
            members,
            topics,
        }
    }
    pub fn dataset(&self) -> &DatasetSummary {
        &self.dataset
    }
    /// Which producer made its members.
    pub fn producer(&self) -> DatasetProducer {
        self.producer
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

    #[test]
    fn a_producer_is_named_by_its_spiral_id() {
        for producer in [DatasetProducer::Mcap, DatasetProducer::LeRobot] {
            assert_eq!(DatasetProducer::from_id(producer.id()), Ok(producer));
            assert_eq!(
                serde_json::to_value(producer).unwrap(),
                serde_json::json!(producer.id())
            );
        }
        assert!(
            DatasetProducer::from_id("spql.parquet.dataset")
                .unwrap_err()
                .contains("spql.parquet.dataset")
        );
    }

    #[test]
    fn a_camera_is_compressed_video_or_a_lerobot_video_feature() {
        let mcap = DatasetTopic::new("/top".into(), vec!["foxglove.CompressedVideo".into()]);
        assert!(mcap.is_video());
        let feature = |encoding: &str| {
            DatasetTopic::new("observation.images.up".into(), vec![])
                .with_encodings(vec![encoding.into()])
        };
        assert!(feature("mp4").is_video());
        assert!(!feature("parquet").is_video());
        // A stream is named by its schema, or by its topic alone when it has
        // none.
        assert!(mcap.is_recorded_as("/top", Some("foxglove.CompressedVideo")));
        assert!(!mcap.is_recorded_as("/top", None));
        assert!(feature("parquet").is_recorded_as("observation.images.up", None));
        assert!(!feature("parquet").is_recorded_as("observation.images.up", Some("x")));
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
