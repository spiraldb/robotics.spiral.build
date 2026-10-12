//! What a run trains on, resolved when it starts: the intervals its view
//! keeps — each episode row's `kept` ranges, unnested — and, for every
//! episode, the streams that hold them, named by what is read: each camera's
//! packets the model reads and, for a policy, each joint stream's values.
use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use spiral_robotics_common::datasets::{
    CameraVideo, DatasetInspection, DatasetJointRoles, DatasetSelection, EpisodeId,
    EpisodeSelection, NamedValues, RecordedValues, ValueColumns,
};
use spiral_robotics_common::sql::SqlLiteral;
use spiral_robotics_common::training::{TrainingConfig, TrainingInputSummary, TrainingModel};
use spiral_robotics_storage::{KeptInterval, StorageManager};

use super::service::{
    ActRunData, ActStreams, EpisodeStreams, JointNames, ProgressRunData, ProgressStreams, RunData,
    StreamKey,
};
use crate::OperationError;

/// What one run trains on.
pub(super) struct TrainingInput {
    dataset: DatasetVersion,
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
        let dataset = DatasetVersion::of(source.dataset(), &inspection);
        dataset.records_cameras(&inspection, config.model().cameras())?;
        let reads = StreamReads::of(storage, config, &dataset.selection.sql_name()).await?;
        Ok(Self {
            dataset,
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
                self.dataset.selection.display_name(),
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
                        self.dataset.selection.clone(),
                        episode.id.clone(),
                    ),
                    read: read.clone(),
                });
            }
        }
        streams.into_values().collect()
    }
}

/// The dataset version a run reads, which names its streams.
struct DatasetVersion {
    selection: DatasetSelection,
    /// The dataset's resource and version ids.
    identity: (String, String),
}

impl DatasetVersion {
    /// The version of `selection` that `inspection` read.
    fn of(selection: &DatasetSelection, inspection: &DatasetInspection) -> Self {
        Self {
            selection: selection.clone(),
            identity: (
                inspection.dataset().resource_id().to_owned(),
                inspection.dataset().version_id().to_owned(),
            ),
        }
    }

    /// Whether the dataset `inspection` read records every one of
    /// `cameras` as video.
    fn records_cameras(
        &self,
        inspection: &DatasetInspection,
        cameras: &[String],
    ) -> Result<(), OperationError> {
        let missing = cameras.iter().find(|camera| {
            !inspection
                .topics()
                .iter()
                .any(|topic| topic.is_video() && topic.name() == camera.as_str())
        });
        match missing {
            Some(camera) => Err(OperationError::invalid_input(format!(
                "{} records no camera {camera}.",
                self.selection.display_name()
            ))),
            None => Ok(()),
        }
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
    /// The reads `config`'s model makes of dataset `dataset`, a policy's
    /// with the joints the dataset's roles name.
    async fn of(
        storage: &StorageManager,
        config: &TrainingConfig,
        dataset: &str,
    ) -> Result<Self, OperationError> {
        match config.model() {
            TrainingModel::Act(act) => {
                let inspection = storage
                    .inspect_dataset(config.source().dataset().clone())
                    .await
                    .map_err(OperationError::External)?;
                let roles = storage
                    .dataset_joint_roles(inspection.dataset())
                    .await
                    .map_err(OperationError::External)?
                    .ok_or_else(|| {
                        OperationError::invalid_input(
                            "This dataset's joint roles are not resolved yet; open it in Data first.",
                        )
                    })?;
                ActReads::new(dataset, act.observation().cameras(), &roles)
                    .map(Self::Act)
                    .map_err(OperationError::InvalidInput)
            }
            TrainingModel::Progress(progress) => Ok(Self::Progress(StreamRead::camera(
                dataset,
                progress.camera(),
            ))),
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
    /// `roles` reads and commands, in dataset `dataset`.
    fn new(dataset: &str, cameras: &[String], roles: &DatasetJointRoles) -> Result<Self, String> {
        let cameras = cameras
            .iter()
            .map(|camera| StreamRead::camera(dataset, camera))
            .collect();
        let joints = roles.policy_joints()?;
        let reads = |values: &[NamedValues<'_>]| {
            values
                .iter()
                .map(|values| StreamRead::values(dataset, values.values()))
                .collect()
        };
        Ok(Self {
            cameras,
            state: reads(joints.state()),
            action: reads(joints.action()),
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
    /// The packets `camera` recorded, as every run of either family reads
    /// them, so one camera's stream is uploaded once for both.
    fn camera(dataset: &str, camera: &str) -> Self {
        Self {
            statement: CameraVideo::new(dataset, camera).recorded_packets(),
            shape: StreamShape::Camera,
        }
    }

    /// The numbers `values` name, one row per message: `log_time_ns`, then
    /// `v0`, `v1`, … as float64.
    fn values(dataset: &str, values: &RecordedValues) -> Self {
        let columns = ValueColumns::new(values, "v");
        let read = format!(
            "spql.mcap.read_topic({}, {}) WHERE id = $1",
            SqlLiteral::text(dataset),
            SqlLiteral::text(values.topic())
        );
        let numbers = columns
            .names()
            .iter()
            .map(|name| format!("CAST({name} AS DOUBLE) AS {name}"))
            .collect::<Vec<_>>()
            .join(", ");
        Self {
            statement: format!(
                "WITH {} SELECT CAST(log_time AS BIGINT) AS log_time_ns, {numbers} FROM joint ORDER BY log_time",
                columns.ctes("joint", &read).join(", ")
            ),
            shape: StreamShape::Values(columns.count()),
        }
    }

    fn key(&self, dataset: &DatasetVersion, episode: &EpisodeId) -> StreamKey {
        StreamKey::of(
            (&dataset.identity.0, &dataset.identity.1),
            episode.as_str(),
            &self.statement,
        )
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
    use spiral_robotics_common::datasets::{DatasetProducer, DatasetSummary, DatasetTopic};

    fn roles(value: serde_json::Value) -> DatasetJointRoles {
        serde_json::from_value(value).unwrap()
    }

    fn values(topic: &str, field: &str, indices: &[usize]) -> serde_json::Value {
        json!({"topic": topic, "schema": "RobotState", "field": [field], "element": [], "indices": indices})
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
        let reads = ActReads::new("dataset_abc", &["/top-camera".into()], &roles).unwrap();
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
            "WITH joint AS (SELECT id, log_time, spql.list.get(\"value\".\"position\", 0) AS v0 FROM spql.mcap.read_topic('dataset_abc', '/left-ee-action') WHERE id = $1) SELECT CAST(log_time AS BIGINT) AS log_time_ns, CAST(v0 AS DOUBLE) AS v0 FROM joint ORDER BY log_time"
        );
        assert_eq!(reads.cameras[0].shape(), StreamShape::Camera);
    }

    /// A progress run's camera is the stream a policy run of the same
    /// camera uploaded, so it uploads nothing new.
    #[test]
    fn a_progress_camera_is_the_stream_a_policy_reads() {
        let roles = roles(json!({"arms": [
            {"name": "left", "state": values("/left-arm-state", "position", &[0]),
             "action": values("/left-arm-action", "position", &[0]), "gripper": null}
        ]}));
        let policy = ActReads::new("dataset_abc", &["/top-camera".into()], &roles).unwrap();
        let progress = StreamReads::Progress(StreamRead::camera("dataset_abc", "/top-camera"));
        let dataset = DatasetVersion {
            selection: DatasetSelection::new(vec!["dataset_abc".into()]).unwrap(),
            identity: ("resource".into(), "v1".into()),
        };
        let episode = EpisodeId::fixture(1);
        assert_eq!(
            progress
                .all()
                .iter()
                .map(|read| read.key(&dataset, &episode))
                .collect::<Vec<_>>(),
            [policy.cameras[0].key(&dataset, &episode)]
        );
    }

    #[test]
    fn every_camera_a_model_reads_must_be_recorded_video() {
        let selection = DatasetSelection::new(vec!["dataset_abc".into()]).unwrap();
        let inspection = DatasetInspection::new(
            DatasetSummary::new(selection.clone(), ("resource".into(), "v1".into())),
            DatasetProducer::Mcap,
            None,
            vec![
                DatasetTopic::new(
                    "/top-camera".into(),
                    vec!["foxglove.CompressedVideo".into()],
                ),
                DatasetTopic::new("/joints".into(), vec!["JointState".into()]),
            ],
        );
        let dataset = DatasetVersion::of(&selection, &inspection);
        assert!(
            dataset
                .records_cameras(&inspection, &["/top-camera".into()])
                .is_ok()
        );
        for missing in ["/joints", "/wrist-camera"] {
            assert!(
                dataset
                    .records_cameras(&inspection, &[missing.into()])
                    .is_err()
            );
        }
    }

    /// A LeRobot dataset's camera is a video feature, and its joints the
    /// named members of `observation.state` and `action`, read by name.
    #[test]
    fn a_lerobot_policy_reads_its_video_features_and_named_joints() {
        let selection = DatasetSelection::new(vec!["dataset_so101".into()]).unwrap();
        let feature = |name: &str, encoding: &str| {
            DatasetTopic::new(name.into(), vec![]).with_encodings(vec![encoding.into()])
        };
        let inspection = DatasetInspection::new(
            DatasetSummary::new(selection.clone(), ("resource".into(), "v1".into())),
            DatasetProducer::LeRobot,
            Some(50),
            vec![
                feature("observation.images.up", "mp4"),
                feature("observation.state", "parquet"),
            ],
        );
        let dataset = DatasetVersion::of(&selection, &inspection);
        assert!(
            dataset
                .records_cameras(&inspection, &["observation.images.up".into()])
                .is_ok()
        );
        assert!(
            dataset
                .records_cameras(&inspection, &["observation.state".into()])
                .is_err()
        );
        let named = |topic: &str, members: &[&str]| json!({"topic": topic, "schema": null, "field": [], "element": [], "indices": [], "members": members});
        let gripper = |topic: &str| json!({"topic": topic, "schema": null, "field": ["gripper.pos"], "element": [], "indices": []});
        let roles = roles(json!({"arms": [{"name": "so101",
            "state": named("observation.state", &["shoulder_pan.pos", "elbow_flex.pos"]),
            "action": named("action", &["shoulder_pan.pos", "elbow_flex.pos"]),
            "gripper": {"state": gripper("observation.state"), "action": gripper("action"), "open": 100.0, "closed": 0.0}}]}));
        let reads =
            ActReads::new("dataset_so101", &["observation.images.up".into()], &roles).unwrap();
        assert_eq!(
            reads.joints.state(),
            ["so101.j1", "so101.j2", "so101.gripper"]
        );
        assert_eq!(
            reads.state[0].statement(),
            "WITH joint AS (SELECT id, log_time, \"value\".\"shoulder_pan.pos\" AS v0, \"value\".\"elbow_flex.pos\" AS v1 FROM spql.mcap.read_topic('dataset_so101', 'observation.state') WHERE id = $1) SELECT CAST(log_time AS BIGINT) AS log_time_ns, CAST(v0 AS DOUBLE) AS v0, CAST(v1 AS DOUBLE) AS v1 FROM joint ORDER BY log_time"
        );
        assert_eq!(
            reads.action[1].statement(),
            "WITH joint AS (SELECT id, log_time, \"value\".\"gripper.pos\" AS v0 FROM spql.mcap.read_topic('dataset_so101', 'action') WHERE id = $1) SELECT CAST(log_time AS BIGINT) AS log_time_ns, CAST(v0 AS DOUBLE) AS v0 FROM joint ORDER BY log_time"
        );
        assert_eq!(
            reads.cameras[0].statement(),
            "SELECT CAST(r.log_time AS BIGINT) AS log_time_ns, r.value.data AS payload FROM spql.mcap.read_topic('dataset_so101', 'observation.images.up') r WHERE r.id = $1 ORDER BY r.chunk_start_offset, r.sample_ordinal"
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
