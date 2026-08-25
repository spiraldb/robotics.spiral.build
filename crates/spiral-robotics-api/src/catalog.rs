//! User-visible catalog vocabulary shared by the Data and Training activities.
//!
//! Physical Arrow schemas and ADBC operations live in `spiral-robotics-storage`;
//! this module only publishes the managed relation names and query templates
//! that cross the frontend/backend boundary.

include!(concat!(env!("OUT_DIR"), "/storage_schema_version.rs"));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedTable {
    Datasets,
    Episodes,
    EpisodeStreams,
    JointSamples,
    VideoAssets,
    TrainingCheckpoints,
    Robots,
    StudioSettings,
    TrainingRuns,
    RolloutRuns,
    WorkflowRuns,
}

impl ManagedTable {
    pub const ALL: [Self; 11] = [
        Self::Datasets,
        Self::Episodes,
        Self::EpisodeStreams,
        Self::JointSamples,
        Self::VideoAssets,
        Self::TrainingCheckpoints,
        Self::Robots,
        Self::StudioSettings,
        Self::TrainingRuns,
        Self::RolloutRuns,
        Self::WorkflowRuns,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Datasets => "datasets",
            Self::Episodes => "episodes",
            Self::EpisodeStreams => "episode_streams",
            Self::JointSamples => "joint_samples",
            Self::VideoAssets => "video_assets",
            Self::TrainingCheckpoints => "training_checkpoints",
            Self::Robots => "robots",
            Self::StudioSettings => "studio_settings",
            Self::TrainingRuns => "training_runs",
            Self::RolloutRuns => "rollout_runs",
            Self::WorkflowRuns => "workflow_runs",
        }
    }

    pub fn qualified_name(self, namespace: &str) -> String {
        format!("{namespace}.{}", self.name())
    }

    pub const fn columns(self) -> &'static [&'static str] {
        match self {
            Self::Datasets => &["dataset_id", "name", "created_ms", "description"],
            Self::Episodes => &[
                "episode_id",
                "dataset_id",
                "schema_version",
                "started_ms",
                "sample_hz",
                "source",
                "note",
                "frames",
                "duration_ns",
                "max_follow_error_degrees",
                "discarded",
                "metadata_json",
            ],
            Self::EpisodeStreams => &[
                "episode_id",
                "stream_id",
                "modality",
                "robot_type",
                "joint_names",
                "camera_id",
                "width",
                "height",
                "fps",
            ],
            Self::JointSamples => &[
                "episode_id",
                "stream_id",
                "sample_index",
                "episode_time_ns",
                "state_valid",
                "action_valid",
                "state_position",
                "action_position",
                "raw_position",
                "velocity",
                "effort",
                "follow_error_degrees",
            ],
            Self::VideoAssets => &[
                "episode_id",
                "stream_id",
                "segment_index",
                "timeline_offset_ns",
                "camera_id",
                "payload",
                "byte_size",
                "width",
                "height",
                "fps",
                "frames",
                "media_relation",
            ],
            Self::TrainingCheckpoints => &[
                "run_id",
                "run_name",
                "policy",
                "step",
                "created_ms",
                "validation_loss",
                "config_json",
                "checkpoint",
            ],
            Self::Robots => &["robot_id", "config_json", "updated_ms"],
            Self::StudioSettings => &["key", "value_json", "updated_ms"],
            Self::TrainingRuns | Self::RolloutRuns | Self::WorkflowRuns => {
                &["run_id", "snapshot_json", "updated_ms"]
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryPreset {
    Datasets,
    Episodes,
    EpisodeStreams,
    JointSamples,
    VideoAssets,
    TrainingCheckpoints,
    Robots,
    StudioSettings,
    TrainingRuns,
    RolloutRuns,
    WorkflowRuns,
}

impl QueryPreset {
    pub const ALL: [Self; 11] = [
        Self::Datasets,
        Self::Episodes,
        Self::EpisodeStreams,
        Self::JointSamples,
        Self::VideoAssets,
        Self::TrainingCheckpoints,
        Self::Robots,
        Self::StudioSettings,
        Self::TrainingRuns,
        Self::RolloutRuns,
        Self::WorkflowRuns,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Datasets => "Datasets",
            Self::Episodes => "Episodes",
            Self::EpisodeStreams => "Streams",
            Self::JointSamples => "Joint samples",
            Self::VideoAssets => "Video assets",
            Self::TrainingCheckpoints => "Checkpoints",
            Self::Robots => "Robots",
            Self::StudioSettings => "Studio settings",
            Self::TrainingRuns => "Training runs",
            Self::RolloutRuns => "Rollout runs",
            Self::WorkflowRuns => "Workflow runs",
        }
    }

    pub const fn table(self) -> ManagedTable {
        match self {
            Self::Datasets => ManagedTable::Datasets,
            Self::Episodes => ManagedTable::Episodes,
            Self::EpisodeStreams => ManagedTable::EpisodeStreams,
            Self::JointSamples => ManagedTable::JointSamples,
            Self::VideoAssets => ManagedTable::VideoAssets,
            Self::TrainingCheckpoints => ManagedTable::TrainingCheckpoints,
            Self::Robots => ManagedTable::Robots,
            Self::StudioSettings => ManagedTable::StudioSettings,
            Self::TrainingRuns => ManagedTable::TrainingRuns,
            Self::RolloutRuns => ManagedTable::RolloutRuns,
            Self::WorkflowRuns => ManagedTable::WorkflowRuns,
        }
    }

    pub fn sql(self, namespace: &str) -> String {
        let relation = self.table().qualified_name(namespace);
        match self {
            Self::Datasets => {
                format!("SELECT * FROM {relation} ORDER BY created_ms DESC LIMIT 100")
            }
            Self::Episodes => {
                format!("SELECT * FROM {relation} ORDER BY started_ms DESC LIMIT 100")
            }
            Self::EpisodeStreams => {
                format!("SELECT * FROM {relation} ORDER BY episode_id DESC, stream_id LIMIT 100")
            }
            Self::JointSamples => format!(
                "SELECT * FROM {relation} ORDER BY episode_id DESC, stream_id, sample_index LIMIT 100"
            ),
            Self::VideoAssets => format!(
                "SELECT episode_id, stream_id, segment_index, camera_id, byte_size, width, height, fps, frames FROM {relation} ORDER BY episode_id DESC, stream_id, segment_index LIMIT 100"
            ),
            Self::TrainingCheckpoints => format!(
                "SELECT run_id, run_name, policy, step, created_ms, validation_loss FROM {relation} ORDER BY created_ms DESC LIMIT 100"
            ),
            Self::Robots => format!("SELECT * FROM {relation} ORDER BY robot_id LIMIT 100"),
            Self::StudioSettings => format!("SELECT * FROM {relation} ORDER BY key LIMIT 100"),
            Self::TrainingRuns | Self::RolloutRuns | Self::WorkflowRuns => {
                format!("SELECT * FROM {relation} ORDER BY updated_ms DESC LIMIT 100")
            }
        }
    }
}

pub fn is_dataset_id(value: &str) -> bool {
    spiral_robotics_core::identity::DatasetId::parse(value)
        .is_ok_and(|identity| identity.as_str() == value)
}

pub mod column {
    pub const STATE_VALID: &str = "state_valid";
    pub const ACTION_VALID: &str = "action_valid";
}

pub fn default_training_sql_for_dataset(namespace: &str, dataset_id: &str) -> String {
    let datasets = ManagedTable::Datasets.qualified_name(namespace);
    let samples = ManagedTable::JointSamples.qualified_name(namespace);
    let episodes = ManagedTable::Episodes.qualified_name(namespace);
    let streams = ManagedTable::EpisodeStreams.qualified_name(namespace);
    let dataset_id = dataset_id.replace('\'', "''");
    format!(
        "SELECT j.episode_id, j.stream_id, j.sample_index, j.episode_time_ns,\n       e.sample_hz, e.frames AS episode_frames, s.robot_type, s.joint_names,\n       j.state_position AS state,\n       j.action_position AS action\nFROM {samples} AS j\nJOIN {episodes} AS e ON e.episode_id = j.episode_id\nJOIN {datasets} AS d ON d.dataset_id = e.dataset_id\nJOIN {streams} AS s ON s.episode_id = j.episode_id AND s.stream_id = j.stream_id\nWHERE e.schema_version = {SCHEMA_VERSION}\n  AND d.dataset_id = '{dataset_id}'\n  AND NOT e.discarded\n  AND s.modality = 'joint'\n  AND (\n      LOWER(s.robot_type) LIKE '%follower%'\n      OR (s.robot_type IS NULL AND s.stream_id = 'follower')\n  )\n  AND j.state_valid\n  AND j.action_valid"
    )
}
