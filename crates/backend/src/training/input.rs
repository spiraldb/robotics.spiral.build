//! What a run trains on, resolved when it starts: the intervals its view
//! keeps and, for every episode, the streams that hold them — each camera's
//! packets and each joint stream's values — named by what is read.
use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use spiral_robotics_common::datasets::{
    CameraVideo, DatasetJointRoles, DatasetSelection, EpisodeId, EpisodeSelection, NamedValues,
    RecordedValues, ValueColumns,
};
use spiral_robotics_common::sql::SqlLiteral;
use spiral_robotics_common::training::{TrainingConfig, TrainingInputSummary};
use spiral_robotics_storage::{KeptInterval, StorageManager};

use super::service::{EpisodeStreamKeys, EpisodeStreams, JointNames, RunData, StreamKey};
use crate::OperationError;

/// What one run trains on.
pub(super) struct TrainingInput {
    dataset: DatasetVersion,
    reads: StreamReads,
    episodes: Vec<KeptEpisode>,
}

impl TrainingInput {
    /// What `config` trains on now: its view's intervals, read from the
    /// dataset version its dataset resolves to, with the joints its roles
    /// name.
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
        let roles = storage
            .dataset_joint_roles(source.dataset().clone())
            .await
            .map_err(OperationError::External)?
            .ok_or_else(|| {
                OperationError::invalid_input(
                    "This dataset's joint roles are not resolved yet; open it in Data first.",
                )
            })?;
        let dataset = DatasetVersion {
            selection: source.dataset().clone(),
            identity: (
                inspection.dataset().resource_id().to_owned(),
                inspection.dataset().version_id().to_owned(),
            ),
        };
        let reads = StreamReads::new(
            &dataset.selection.sql_name(),
            config.observation().cameras(),
            &roles,
        )
        .map_err(OperationError::InvalidInput)?;
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
        TrainingInputSummary::new(
            self.episodes.len(),
            intervals.sum(),
            nanoseconds as f64 / 1e9,
        )
        .with_joints(
            self.reads.joints.state().to_vec(),
            self.reads.joints.action().to_vec(),
        )
    }

    /// The run's data as the training service reads it.
    pub(super) fn run_data(&self) -> RunData {
        let episodes = self
            .episodes
            .iter()
            .map(|episode| {
                let keys = |reads: &[StreamRead]| {
                    reads
                        .iter()
                        .map(|read| read.key(&self.dataset, &episode.id))
                        .collect()
                };
                EpisodeStreams::new(
                    episode.id.as_str().to_owned(),
                    episode.intervals.clone(),
                    EpisodeStreamKeys::new(
                        keys(&self.reads.cameras),
                        keys(&self.reads.state),
                        keys(&self.reads.action),
                    ),
                )
            })
            .collect();
        RunData::new(
            self.dataset.selection.display_name(),
            self.reads.joints.clone(),
            episodes,
        )
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
}

/// The statements reading one episode's streams, `$1` its id, and what each
/// state and action value is.
struct StreamReads {
    cameras: Vec<StreamRead>,
    state: Vec<StreamRead>,
    action: Vec<StreamRead>,
    joints: JointNames,
}

impl StreamReads {
    /// The reads of `cameras`, and of the values a policy trained on
    /// `roles` reads and commands, in dataset `dataset`.
    fn new(dataset: &str, cameras: &[String], roles: &DatasetJointRoles) -> Result<Self, String> {
        let cameras = cameras
            .iter()
            .map(|camera| StreamRead {
                statement: CameraVideo::new(dataset, camera).recorded_packets(),
                shape: StreamShape::Camera,
            })
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

    fn all(&self) -> impl Iterator<Item = &StreamRead> {
        self.cameras.iter().chain(&self.state).chain(&self.action)
    }
}

/// One statement reading an episode's stream, and the shape it is uploaded in.
#[derive(Clone)]
pub(super) struct StreamRead {
    statement: String,
    shape: StreamShape,
}

impl StreamRead {
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
        let reads = StreamReads::new("dataset_abc", &["/top-camera".into()], &roles).unwrap();
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
