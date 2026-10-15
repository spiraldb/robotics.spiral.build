//! What a run trains on, resolved when it starts: the intervals its view
//! keeps — each episode row's `kept` ranges, unnested — and, for every
//! episode, the streams that hold them, named by what is read: each camera's
//! packets the model reads and, for a policy, each joint stream's values.
use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use spiral_robotics_common::datasets::{
    CameraVideo, DatasetInspection, DatasetJointRoles, DatasetStreams, DatasetSummary, EpisodeId,
    EpisodeSelection, NamedValues, RecordedValues, ValueColumns,
};
use spiral_robotics_common::training::{TrainingConfig, TrainingInputSummary, TrainingModel};
use spiral_robotics_storage::{KeptInterval, StorageManager};

use super::service::{
    ActRunData, ActStreams, EpisodeStreams, JointNames, ProgressRunData, ProgressStreams, RunData,
    StreamKey,
};
use crate::OperationError;

/// What one run trains on.
pub(super) struct TrainingInput {
    /// The dataset at the version the run reads, which names its streams.
    dataset: DatasetSummary,
    reads: StreamReads,
    episodes: Vec<KeptEpisode>,
}

impl TrainingInput {
    /// What `config` trains on now: its view's intervals, read from the
    /// dataset version its dataset resolves to, with the cameras its model
    /// reads and, for a policy, the joints its roles name.
    pub(super) async fn resolve(
        storage: &StorageManager,
        config: &TrainingConfig,
    ) -> Result<Self, OperationError> {
        let source = config.source();
        let view = storage
            .curated_views()
            .list()
            .await
            .map_err(OperationError::External)?
            .into_iter()
            .find(|view| view.name().eq_ignore_ascii_case(source.view()))
            .ok_or_else(|| {
                OperationError::NotFound(format!("No view named {} is published.", source.view()))
            })?;
        let intervals = storage
            .queries()
            .kept_intervals(view.statement())
            .await
            .map_err(OperationError::External)?;
        if intervals.is_empty() {
            return Err(OperationError::invalid_input(format!(
                "{} keeps no intervals to train on.",
                source.view()
            )));
        }
        let inspection = storage
            .inspect_dataset(source.dataset().clone())
            .await
            .map_err(OperationError::External)?;
        let reads = StreamReads::of(storage, config, &inspection).await?;
        Ok(Self {
            dataset: inspection.dataset().clone(),
            reads,
            episodes: KeptEpisode::grouped(intervals),
        })
    }

    /// How much this input holds, as the run records it.
    pub(super) fn summary(&self) -> TrainingInputSummary {
        let intervals = self.episodes.iter().map(|episode| episode.intervals.len());
        let nanoseconds: i64 = self
            .episodes
            .iter()
            .flat_map(|episode| &episode.intervals)
            .map(|[start, end]| end - start)
            .sum();
        let summary = TrainingInputSummary::new(
            self.episodes.len(),
            intervals.sum(),
            nanoseconds as f64 / 1e9,
        );
        match &self.reads {
            StreamReads::Act(reads) => summary.with_joints(
                reads.joints.state().to_vec(),
                reads.joints.action().to_vec(),
            ),
            StreamReads::Progress(_) => summary,
        }
    }

    /// The run's data as the training service reads it.
    pub(super) fn run_data(&self) -> RunData {
        let key = |read: &StreamRead, episode: &KeptEpisode| read.key(&self.dataset, &episode.id);
        let keys = |reads: &[StreamRead], episode: &KeptEpisode| {
            reads.iter().map(|read| key(read, episode)).collect()
        };
        match &self.reads {
            StreamReads::Act(reads) => RunData::Act(ActRunData::new(
                self.dataset.selection().display_name().to_owned(),
                reads.joints.clone(),
                self.episodes
                    .iter()
                    .map(|episode| {
                        episode.streams(ActStreams::new(
                            keys(&reads.cameras, episode),
                            keys(&reads.state, episode),
                            keys(&reads.action, episode),
                        ))
                    })
                    .collect(),
            )),
            StreamReads::Progress(camera) => RunData::Progress(ProgressRunData::new(
                self.episodes
                    .iter()
                    .map(|episode| episode.streams(ProgressStreams::new(key(camera, episode))))
                    .collect(),
            )),
        }
    }

    /// Every stream the run reads, once each.
    pub(super) fn streams(&self) -> Vec<PendingStream> {
        let mut streams = BTreeMap::new();
        for episode in &self.episodes {
            for read in self.reads.all() {
                let key = read.key(&self.dataset, &episode.id);
                streams.entry(key.clone()).or_insert_with(|| PendingStream {
                    key,
                    episode: EpisodeSelection::new(
                        self.dataset.selection().clone(),
                        episode.id.clone(),
                    ),
                    read: read.clone(),
                });
            }
        }
        streams.into_values().collect()
    }
}

/// One episode and the intervals its view keeps, as `[start_ns, end_ns]`.
struct KeptEpisode {
    id: EpisodeId,
    intervals: Vec<[i64; 2]>,
}

impl KeptEpisode {
    /// `intervals`, in episode order, grouped by episode.
    fn grouped(intervals: Vec<KeptInterval>) -> Vec<Self> {
        let mut episodes: Vec<Self> = Vec::new();
        for interval in intervals {
            let span = [interval.start_ns(), interval.end_ns()];
            match episodes.last_mut() {
                Some(episode) if &episode.id == interval.episode_id() => {
                    episode.intervals.push(span);
                }
                _ => episodes.push(Self {
                    id: interval.episode_id().clone(),
                    intervals: vec![span],
                }),
            }
        }
        episodes
    }

    /// This episode's intervals with `streams`, as the service reads them.
    fn streams<S>(&self, streams: S) -> EpisodeStreams<S> {
        EpisodeStreams::new(self.id.as_str().to_owned(), self.intervals.clone(), streams)
    }
}

/// The statements reading one episode's streams, `$1` its id, in the shape
/// the run's model family reads them.
enum StreamReads {
    Act(ActReads),
    /// A progress model's one camera.
    Progress(StreamRead),
}

impl StreamReads {
    /// The reads `config`'s model makes of the dataset `inspection` read, a
    /// policy's with the joints the dataset's roles name.
    async fn of(
        storage: &StorageManager,
        config: &TrainingConfig,
        inspection: &DatasetInspection,
    ) -> Result<Self, OperationError> {
        match config.model() {
            TrainingModel::Act(act) => {
                let roles = storage
                    .dataset_joint_roles(inspection.dataset())
                    .await
                    .map_err(OperationError::External)?
                    .ok_or_else(|| {
                        OperationError::invalid_input(
                            "This dataset's joint roles are not resolved yet; open it in Data first.",
                        )
                    })?;
                ActReads::new(inspection, act.observation().cameras(), &roles)
                    .map(Self::Act)
                    .map_err(OperationError::InvalidInput)
            }
            TrainingModel::Progress(progress) => StreamRead::camera(inspection, progress.camera())
                .map(Self::Progress)
                .map_err(OperationError::InvalidInput),
        }
    }

    fn all(&self) -> Vec<&StreamRead> {
        match self {
            Self::Act(reads) => reads
                .cameras
                .iter()
                .chain(&reads.state)
                .chain(&reads.action)
                .collect(),
            Self::Progress(camera) => vec![camera],
        }
    }
}

/// An ACT policy's reads: each camera's, each state and action value
/// stream's, and what each state and action value is.
struct ActReads {
    cameras: Vec<StreamRead>,
    state: Vec<StreamRead>,
    action: Vec<StreamRead>,
    joints: JointNames,
}

impl ActReads {
    /// The reads of `cameras`, and of the values a policy trained on
    /// `roles` reads and commands, in the dataset `inspection` read.
    fn new(
        inspection: &DatasetInspection,
        cameras: &[String],
        roles: &DatasetJointRoles,
    ) -> Result<Self, String> {
        let cameras = cameras
            .iter()
            .map(|camera| StreamRead::camera(inspection, camera))
            .collect::<Result<_, _>>()?;
        let joints = roles.policy_joints()?;
        let reads = |values: &[NamedValues<'_>]| {
            values
                .iter()
                .map(|values| StreamRead::values(inspection, values.values()))
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(Self {
            cameras,
            state: reads(joints.state())?,
            action: reads(joints.action())?,
            joints: JointNames::new(joints.state_names(), joints.action_names()),
        })
    }
}

/// One statement reading an episode's stream, and the shape it is uploaded in.
#[derive(Clone)]
pub(super) struct StreamRead {
    statement: String,
    shape: StreamShape,
}

impl StreamRead {
    /// The packets the camera stream of topic `camera` recorded, as every
    /// run of either family reads them, so one camera's stream is uploaded
    /// once for both. A topic the dataset records no video of is refused.
    fn camera(inspection: &DatasetInspection, camera: &str) -> Result<Self, String> {
        let stream = DatasetStreams::new(inspection.topics()).camera(camera)?;
        Ok(Self {
            statement: CameraVideo::new(inspection.dataset().selection(), stream.revision())
                .recorded_packets(),
            shape: StreamShape::Camera,
        })
    }

    /// The numbers `values` name, one row per message of the stream they
    /// were recorded in: `log_time_ns`, then `v0`, `v1`, … as float64.
    fn values(inspection: &DatasetInspection, values: &RecordedValues) -> Result<Self, String> {
        let stream =
            DatasetStreams::new(inspection.topics()).recorded(values.topic(), values.schema())?;
        let columns = ValueColumns::new(values, "v");
        let read = format!(
            "{} WHERE id = $1",
            inspection.dataset().selection().messages(stream.revision())
        );
        let numbers = columns
            .names()
            .iter()
            .map(|name| format!("CAST({name} AS DOUBLE) AS {name}"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(Self {
            statement: format!(
                "WITH {} SELECT CAST(log_time AS BIGINT) AS log_time_ns, {numbers} FROM joint ORDER BY log_time",
                columns.cte("joint", &read)
            ),
            shape: StreamShape::Values(columns.count()),
        })
    }

    /// The key of this read of `episode` of `dataset` at its version.
    fn key(&self, dataset: &DatasetSummary, episode: &EpisodeId) -> StreamKey {
        StreamKey::of(dataset.identity(), episode.as_str(), &self.statement)
    }

    pub(super) fn statement(&self) -> &str {
        &self.statement
    }

    pub(super) const fn shape(&self) -> StreamShape {
        self.shape
    }
}

/// How a stream is laid out on the service (`docs/training.md#streams`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum StreamShape {
    /// H.264 Annex-B access units: `log_time_ns`, `payload`.
    Camera,
    /// `log_time_ns`, then this many float64 values.
    Values(usize),
}

impl StreamShape {
    pub(super) fn schema(self) -> SchemaRef {
        let time = Field::new("log_time_ns", DataType::Int64, false);
        Arc::new(match self {
            Self::Camera => Schema::new(vec![time, Field::new("payload", DataType::Binary, false)])
                .with_metadata(
                    [("codec", "h264"), ("framing", "annex_b")]
                        .into_iter()
                        .map(|(key, value)| (key.to_owned(), value.to_owned()))
                        .collect(),
                ),
            Self::Values(count) => Schema::new(
                std::iter::once(time)
                    .chain(
                        (0..count)
                            .map(|value| Field::new(format!("v{value}"), DataType::Float64, true)),
                    )
                    .collect::<Vec<_>>(),
            ),
        })
    }
}

/// One stream to upload: its key, and the episode read that fills it.
pub(super) struct PendingStream {
    key: StreamKey,
    episode: EpisodeSelection,
    read: StreamRead,
}

impl PendingStream {
    pub(super) fn key(&self) -> &StreamKey {
        &self.key
    }
    pub(super) fn episode(&self) -> &EpisodeSelection {
        &self.episode
    }
    pub(super) fn read(&self) -> &StreamRead {
        &self.read
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spiral_robotics_common::datasets::{
        DatasetFormat, DatasetSelection, DatasetTopic, TopicRevision,
    };

    fn roles(value: serde_json::Value) -> DatasetJointRoles {
        serde_json::from_value(value).unwrap()
    }

    fn values(topic: &str, field: &str, indices: &[usize]) -> serde_json::Value {
        json!({"topic": topic, "schema": "RobotState", "field": [field], "element": [], "indices": indices})
    }

    /// `topic`'s stream `stream`, recorded as `encoding` with `schema`.
    fn stream(topic: &str, stream: &str, encoding: &str, schema: Option<&str>) -> DatasetTopic {
        DatasetTopic::new(
            topic.into(),
            TopicRevision::new(stream).unwrap(),
            encoding.into(),
        )
        .with_schema(schema.map(str::to_owned))
    }

    /// The dataset `id`, at version `v1` of its episodes, recording `topics`.
    fn inspection(id: &str, format: DatasetFormat, topics: Vec<DatasetTopic>) -> DatasetInspection {
        let dataset = DatasetSelection::new(id).unwrap();
        DatasetInspection::new(
            DatasetSummary::new(dataset, format, ("resource".into(), "v1".into())),
            1,
            topics,
        )
    }

    /// An MCAP dataset recording a top camera, a joint stream and the
    /// state and action of a left and a right arm and the left gripper.
    fn bench() -> DatasetInspection {
        let video = Some("foxglove.CompressedVideo");
        let mut topics = vec![
            stream("/top-camera", "top_camera_00000001", "protobuf", video),
            stream("/joints", "joints_00000002", "protobuf", Some("JointState")),
        ];
        for (topic, name) in [
            ("/left-arm-state", "left_arm_state_00000003"),
            ("/left-arm-action", "left_arm_action_00000004"),
            ("/left-ee-state", "left_ee_state_00000005"),
            ("/left-ee-action", "left_ee_action_00000006"),
            ("/right-arm-state", "right_arm_state_00000007"),
            ("/right-arm-action", "right_arm_action_00000008"),
        ] {
            topics.push(stream(topic, name, "protobuf", Some("RobotState")));
        }
        inspection("abc", DatasetFormat::Mcap, topics)
    }

    #[test]
    fn each_arm_reads_its_joints_then_its_gripper() {
        let roles = roles(json!({"arms": [
            {"name": "left", "state": values("/left-arm-state", "position", &[0, 1]),
             "action": values("/left-arm-action", "position", &[0, 1]),
             "gripper": {"state": values("/left-ee-state", "position", &[0]),
                         "action": values("/left-ee-action", "position", &[0]),
                         "open": 1.0, "closed": 0.0}},
            {"name": "right", "state": values("/right-arm-state", "position", &[0, 1]),
             "action": values("/right-arm-action", "position", &[0, 1]), "gripper": null}
        ]}));
        let reads = ActReads::new(&bench(), &["/top-camera".into()], &roles).unwrap();
        assert_eq!(
            reads.joints.state(),
            ["left.j1", "left.j2", "left.gripper", "right.j1", "right.j2"]
        );
        assert_eq!(reads.joints.action(), reads.joints.state());
        assert_eq!(
            reads
                .state
                .iter()
                .map(StreamRead::shape)
                .collect::<Vec<_>>(),
            [
                StreamShape::Values(2),
                StreamShape::Values(1),
                StreamShape::Values(2)
            ]
        );
        assert_eq!(
            reads.action[1].statement(),
            "WITH joint AS (SELECT id, log_time, array_element(\"value\".\"position\", 1) AS v0 FROM \"datasets.abc.messages.left_ee_action_00000006\" WHERE id = $1) SELECT CAST(log_time AS BIGINT) AS log_time_ns, CAST(v0 AS DOUBLE) AS v0 FROM joint ORDER BY log_time"
        );
        assert_eq!(reads.cameras[0].shape(), StreamShape::Camera);
        // A value read names its topic's stream by the schema it was recorded
        // with.
        let unrecorded = roles_reading("/left-arm-state", "OtherState");
        assert!(ActReads::new(&bench(), &["/top-camera".into()], &unrecorded).is_err());
    }

    /// Roles whose one arm's state and action are `topic` recorded as
    /// `schema`.
    fn roles_reading(topic: &str, schema: &str) -> DatasetJointRoles {
        let values = json!({"topic": topic, "schema": schema, "field": ["position"], "element": [], "indices": [0]});
        roles(json!({"arms": [
            {"name": "left", "state": values, "action": values, "gripper": null}
        ]}))
    }

    /// A progress run's camera is the stream a policy run of the same
    /// camera uploaded, so it uploads nothing new.
    #[test]
    fn a_progress_camera_is_the_stream_a_policy_reads() {
        let bench = bench();
        let roles = roles_reading("/left-arm-state", "RobotState");
        let policy = ActReads::new(&bench, &["/top-camera".into()], &roles).unwrap();
        let progress = StreamReads::Progress(StreamRead::camera(&bench, "/top-camera").unwrap());
        let episode = EpisodeId::fixture(1);
        assert_eq!(
            progress
                .all()
                .iter()
                .map(|read| read.key(bench.dataset(), &episode))
                .collect::<Vec<_>>(),
            [policy.cameras[0].key(bench.dataset(), &episode)]
        );
    }

    #[test]
    fn every_camera_a_model_reads_must_be_recorded_video() {
        let bench = bench();
        assert_eq!(
            StreamRead::camera(&bench, "/top-camera")
                .unwrap()
                .statement(),
            "SELECT CAST(r.log_time AS BIGINT) AS log_time_ns, r.value.data AS payload FROM \"datasets.abc.messages.top_camera_00000001\" r WHERE r.id = $1 ORDER BY r.ordinal"
        );
        for missing in ["/joints", "/wrist-camera"] {
            assert!(StreamRead::camera(&bench, missing).is_err(), "{missing}");
        }
    }

    /// A LeRobot dataset's camera is a video feature, and its joints the
    /// named members of `observation.state` and `action`, read by name.
    #[test]
    fn a_lerobot_policy_reads_its_video_features_and_named_joints() {
        let so101 = inspection(
            "so101",
            DatasetFormat::LeRobot,
            vec![
                stream(
                    "observation.images.up",
                    "observation_images_up_0000000a",
                    "h264",
                    None,
                ),
                stream(
                    "observation.state",
                    "observation_state_0000000b",
                    "parquet",
                    None,
                ),
                stream("action", "action_0000000c", "parquet", None),
            ],
        );
        assert!(StreamRead::camera(&so101, "observation.state").is_err());
        let named = |topic: &str, members: &[&str]| json!({"topic": topic, "schema": null, "field": [], "element": [], "indices": [], "members": members});
        let gripper = |topic: &str| json!({"topic": topic, "schema": null, "field": ["gripper.pos"], "element": [], "indices": []});
        let roles = roles(json!({"arms": [{"name": "so101",
            "state": named("observation.state", &["shoulder_pan.pos", "elbow_flex.pos"]),
            "action": named("action", &["shoulder_pan.pos", "elbow_flex.pos"]),
            "gripper": {"state": gripper("observation.state"), "action": gripper("action"), "open": 100.0, "closed": 0.0}}]}));
        let reads = ActReads::new(&so101, &["observation.images.up".into()], &roles).unwrap();
        assert_eq!(
            reads.joints.state(),
            ["so101.j1", "so101.j2", "so101.gripper"]
        );
        assert_eq!(
            reads.state[0].statement(),
            "WITH joint AS (SELECT id, log_time, \"value\".\"shoulder_pan.pos\" AS v0, \"value\".\"elbow_flex.pos\" AS v1 FROM \"datasets.so101.messages.observation_state_0000000b\" WHERE id = $1) SELECT CAST(log_time AS BIGINT) AS log_time_ns, CAST(v0 AS DOUBLE) AS v0, CAST(v1 AS DOUBLE) AS v1 FROM joint ORDER BY log_time"
        );
        assert_eq!(
            reads.action[1].statement(),
            "WITH joint AS (SELECT id, log_time, \"value\".\"gripper.pos\" AS v0 FROM \"datasets.so101.messages.action_0000000c\" WHERE id = $1) SELECT CAST(log_time AS BIGINT) AS log_time_ns, CAST(v0 AS DOUBLE) AS v0 FROM joint ORDER BY log_time"
        );
        assert_eq!(
            reads.cameras[0].statement(),
            "SELECT CAST(r.log_time AS BIGINT) AS log_time_ns, r.value.data AS payload FROM \"datasets.so101.messages.observation_images_up_0000000a\" r WHERE r.id = $1 ORDER BY r.ordinal"
        );
    }

    #[test]
    fn camera_streams_carry_their_codec_and_value_streams_their_count() {
        let camera = StreamShape::Camera.schema();
        assert_eq!(camera.metadata()["codec"], "h264");
        assert_eq!(camera.field(1).data_type(), &DataType::Binary);
        let values = StreamShape::Values(2).schema();
        assert_eq!(
            values
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>(),
            ["log_time_ns", "v0", "v1"]
        );
    }
}
