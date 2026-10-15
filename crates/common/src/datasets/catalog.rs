//! Studio's datasets: each one the tables under `datasets.<id>` that Studio
//! writes itself ([datasets](../../../../docs/datasets.md)), and what reading
//! them reports.
use super::EpisodeId;
use crate::curation::AnswerColumn;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};

/// One Studio dataset, named by its id: the tables under `datasets.<id>`.
/// Every statement names a dataset's tables through it, so none spells a
/// table name itself.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "DatasetName")]
pub struct DatasetSelection {
    id: String,
}

/// A dataset's id as it travels, checked before it names any table.
#[derive(Deserialize)]
struct DatasetName {
    id: String,
}

impl TryFrom<DatasetName> for DatasetSelection {
    type Error = String;

    fn try_from(name: DatasetName) -> Result<Self, Self::Error> {
        Self::new(name.id)
    }
}

impl DatasetSelection {
    /// The namespace every dataset's tables live under.
    const NAMESPACE: &str = "datasets";
    const MAX_BYTES: usize = 96;

    /// The dataset named `id`: 1–96 letters, numbers, `_` or `-`.
    pub fn new(id: impl Into<String>) -> Result<Self, String> {
        let id = id.into();
        if id.is_empty()
            || id.len() > Self::MAX_BYTES
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(format!(
                "{id:?} is not a dataset name: use 1–96 letters, numbers, '_' or '-'"
            ));
        }
        Ok(Self { id })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    /// What an operator calls the dataset.
    pub fn display_name(&self) -> &str {
        &self.id
    }
    /// Its namespace as SQL names it, `datasets.<id>`: what tooltips and
    /// footers show where SQL is the subject.
    pub fn label(&self) -> String {
        format!("{}.{}", Self::NAMESPACE, self.id)
    }

    /// One row per episode.
    pub fn episodes(&self) -> DatasetTable {
        self.table(&["episodes"])
    }
    /// One row per recorded stream.
    pub fn topics(&self) -> DatasetTable {
        self.table(&["topics"])
    }
    /// An MCAP dataset's source files, one per episode.
    pub fn files(&self) -> DatasetTable {
        self.table(&["files"])
    }
    /// One answer column's table: a row per episode asked, keyed by
    /// `episode_id` ([`AnswerColumn`]).
    pub fn answers(&self, column: &AnswerColumn) -> DatasetTable {
        self.table(&["answers", column.as_str()])
    }
    /// The messages of one stream.
    pub fn messages(&self, stream: &TopicRevision) -> DatasetTable {
        self.table(&["messages", stream.stream()])
    }

    /// The dataset a catalog path belongs to, and the path's part under the
    /// dataset's namespace: `["datasets", <id>, "episodes"]` is `episodes` of
    /// `<id>`. A path outside the convention belongs to none.
    pub fn of_catalog_path(path: &[String]) -> Option<(Self, &[String])> {
        match path {
            [namespace, id, rest @ ..] if namespace == Self::NAMESPACE && !rest.is_empty() => {
                Some((Self::new(id.clone()).ok()?, rest))
            }
            _ => None,
        }
    }

    fn table(&self, parts: &[&str]) -> DatasetTable {
        DatasetTable {
            path: [Self::NAMESPACE, self.id.as_str()]
                .into_iter()
                .chain(parts.iter().copied())
                .map(str::to_owned)
                .collect(),
        }
    }
}

/// One table of a dataset. It displays as a statement spells it, each part
/// quoted; ingestion names it by [`Self::target`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatasetTable {
    path: Vec<String>,
}

impl DatasetTable {
    /// Its parts in the catalog.
    pub fn path(&self) -> &[String] {
        &self.path
    }
    /// Its dotted name, as ingestion addresses a table: no part holds a dot.
    pub fn target(&self) -> String {
        self.path.join(".")
    }
}

impl Display for DatasetTable {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let quoted = self
            .path
            .iter()
            .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
            .collect::<Vec<_>>();
        formatter.write_str(&quoted.join("."))
    }
}

/// One recording within a dataset.
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

/// One topic of one episode.
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

/// How a dataset's episodes were made, fixed when it is created.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DatasetFormat {
    /// One episode per MCAP file, each topic the file's channels.
    Mcap,
    /// One episode per episode of a LeRobot v3 dataset, each feature a topic.
    LeRobot,
}

/// A dataset at one version: what Studio keeps about a dataset, such as its
/// robot and joint roles, is filed under this identity, as the browser read
/// it. The identity is the dataset's `episodes` table's resource and version,
/// which changes exactly when episodes are added.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetSummary {
    selection: DatasetSelection,
    format: DatasetFormat,
    resource_id: String,
    version_id: String,
}
impl DatasetSummary {
    pub fn new(
        selection: DatasetSelection,
        format: DatasetFormat,
        identity: (String, String),
    ) -> Self {
        Self {
            selection,
            format,
            resource_id: identity.0,
            version_id: identity.1,
        }
    }
    pub fn selection(&self) -> &DatasetSelection {
        &self.selection
    }
    pub fn format(&self) -> DatasetFormat {
        self.format
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

/// One stream's table under `messages`: a topic in one schema and Arrow
/// type, as the `topics` table names it.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "StreamName")]
pub struct TopicRevision {
    stream: String,
}

#[derive(Deserialize)]
struct StreamName {
    stream: String,
}

impl TryFrom<StreamName> for TopicRevision {
    type Error = String;

    fn try_from(name: StreamName) -> Result<Self, Self::Error> {
        Self::new(name.stream)
    }
}

impl TopicRevision {
    const MAX_BYTES: usize = 96;

    /// The stream named `stream`: lowercase letters, digits and `_`.
    pub fn new(stream: impl Into<String>) -> Result<Self, String> {
        let stream = stream.into();
        if stream.is_empty()
            || stream.len() > Self::MAX_BYTES
            || !stream
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(format!("{stream:?} is not a stream name"));
        }
        Ok(Self { stream })
    }
    pub fn stream(&self) -> &str {
        &self.stream
    }
}

/// One recorded stream of a dataset: a row of its `topics` table.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetTopic {
    name: String,
    revision: TopicRevision,
    schema: Option<String>,
    /// How its messages were recorded: `protobuf` or `json` for an MCAP
    /// channel, `parquet` for a LeRobot feature, `h264` for a LeRobot camera.
    encoding: String,
    members: u64,
    samples: u64,
}
impl DatasetTopic {
    pub fn new(name: String, revision: TopicRevision, encoding: String) -> Self {
        Self {
            name,
            revision,
            schema: None,
            encoding,
            members: 0,
            samples: 0,
        }
    }
    /// The native schema its messages were recorded with.
    pub fn with_schema(mut self, schema: Option<String>) -> Self {
        self.schema = schema;
        self
    }
    /// How many episodes hold its messages, and how many messages.
    pub fn with_coverage(mut self, members: u64, samples: u64) -> Self {
        self.members = members;
        self.samples = samples;
        self
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn revision(&self) -> &TopicRevision {
        &self.revision
    }
    /// The native schema name its messages were recorded with; a LeRobot
    /// feature has none.
    pub fn schema(&self) -> Option<&str> {
        self.schema.as_deref()
    }
    pub fn encoding(&self) -> &str {
        &self.encoding
    }
    /// Whether the stream carries camera video rather than data messages:
    /// recorded as `foxglove.CompressedVideo`, or a LeRobot camera, which
    /// Studio keeps as the same H.264 access units.
    pub fn is_video(&self) -> bool {
        self.schema() == Some("foxglove.CompressedVideo") || self.encoding == "h264"
    }
    /// Whether it is the stream `schema` names on its topic: the one of that
    /// schema, or, with no schema, the topic's stream without one.
    pub fn is_recorded_as(&self, topic: &str, schema: Option<&str>) -> bool {
        self.name == topic && self.schema() == schema
    }
    pub fn members(&self) -> u64 {
        self.members
    }
    pub fn samples(&self) -> u64 {
        self.samples
    }
}

/// A dataset at one version and every stream it records.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetInspection {
    dataset: DatasetSummary,
    members: u64,
    topics: Vec<DatasetTopic>,
}
impl DatasetInspection {
    pub fn new(dataset: DatasetSummary, members: u64, topics: Vec<DatasetTopic>) -> Self {
        Self {
            dataset,
            members,
            topics,
        }
    }
    pub fn dataset(&self) -> &DatasetSummary {
        &self.dataset
    }
    pub fn format(&self) -> DatasetFormat {
        self.dataset.format()
    }
    /// How many episodes it holds.
    pub fn members(&self) -> u64 {
        self.members
    }
    pub fn topics(&self) -> &[DatasetTopic] {
        &self.topics
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dataset_names_every_table_under_its_namespace() {
        let dataset = DatasetSelection::new("pick-place").unwrap();
        assert_eq!(dataset.label(), "datasets.pick-place");
        assert_eq!(
            dataset.episodes().to_string(),
            "\"datasets\".\"pick-place\".\"episodes\""
        );
        let column = AnswerColumn::try_from("score_progress_3f9a2c".to_owned()).unwrap();
        assert_eq!(
            dataset.answers(&column).to_string(),
            "\"datasets\".\"pick-place\".\"answers\".\"score_progress_3f9a2c\""
        );
        let stream = TopicRevision::new("studio_joints_leader_3fa2c1d0").unwrap();
        assert_eq!(
            dataset.messages(&stream).to_string(),
            "\"datasets\".\"pick-place\".\"messages\".\"studio_joints_leader_3fa2c1d0\""
        );
    }

    #[test]
    fn a_dataset_name_cannot_reach_outside_its_namespace() {
        for name in ["", "a.b", "a\"b", "a b", &"x".repeat(97)] {
            assert!(DatasetSelection::new(name).is_err(), "{name:?}");
        }
        let parsed: Result<DatasetSelection, _> = serde_json::from_str(r#"{"id":"a.b"}"#);
        assert!(parsed.is_err());
        assert!(TopicRevision::new("Upper").is_err());
        assert!(TopicRevision::new("a.b").is_err());
    }

    #[test]
    fn a_catalog_path_belongs_to_the_dataset_it_names() {
        let path = |parts: &[&str]| {
            parts
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>()
        };
        let episodes = path(&["datasets", "default", "episodes"]);
        let (dataset, rest) = DatasetSelection::of_catalog_path(&episodes).unwrap();
        assert_eq!(dataset.id(), "default");
        assert_eq!(rest, ["episodes"]);
        assert!(DatasetSelection::of_catalog_path(&path(&["datasets", "default"])).is_none());
        assert!(DatasetSelection::of_catalog_path(&path(&["dataset_default"])).is_none());
    }

    #[test]
    fn a_format_and_a_selection_travel_as_plain_json() {
        assert_eq!(
            serde_json::to_value(DatasetFormat::LeRobot).unwrap(),
            serde_json::json!("lerobot")
        );
        assert_eq!(
            serde_json::to_value(DatasetSelection::new("default").unwrap()).unwrap(),
            serde_json::json!({"id": "default"})
        );
    }

    #[test]
    fn a_camera_is_compressed_video_or_an_h264_stream() {
        let stream = TopicRevision::new("top_00000000").unwrap();
        let mcap = DatasetTopic::new("/top".into(), stream.clone(), "protobuf".into())
            .with_schema(Some("foxglove.CompressedVideo".into()));
        assert!(mcap.is_video());
        let feature = |encoding: &str| {
            DatasetTopic::new(
                "observation.images.up".into(),
                stream.clone(),
                encoding.into(),
            )
        };
        assert!(feature("h264").is_video());
        assert!(!feature("parquet").is_video());
        // A stream is named by its schema, or by its topic alone when it has
        // none.
        assert!(mcap.is_recorded_as("/top", Some("foxglove.CompressedVideo")));
        assert!(!mcap.is_recorded_as("/top", None));
        assert!(feature("parquet").is_recorded_as("observation.images.up", None));
        assert!(!feature("parquet").is_recorded_as("observation.images.up", Some("x")));
    }
}
