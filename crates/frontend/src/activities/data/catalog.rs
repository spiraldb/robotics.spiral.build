//! User-visible catalog vocabulary owned by the Data and Training activities.
//!
//! Physical Arrow schemas and native database operations live in `spiral-robotics-storage`.
//! Relation columns are discovered from Spiral instead of duplicated here.

include!(concat!(env!("OUT_DIR"), "/storage_schema_version.rs"));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedTable {
    Datasets,
    Episodes,
    EpisodeStreams,
    JointSamples,
    VideoAssets,
    TrainingCheckpoints,
    Robots,
    StudioSettings,
    TrainingRuns,
    TrainingRunMetrics,
    TrainingRunCheckpoints,
    RolloutRuns,
    WorkflowRuns,
    FeatureRegistry,
}

impl ManagedTable {
    /// Every managed relation, in the Data schema browser's display order —
    /// kept identical to storage's own relation order.
    pub(crate) const ALL: [Self; 14] = [
        Self::Datasets,
        Self::Episodes,
        Self::EpisodeStreams,
        Self::JointSamples,
        Self::VideoAssets,
        Self::TrainingCheckpoints,
        Self::Robots,
        Self::StudioSettings,
        Self::TrainingRuns,
        Self::TrainingRunMetrics,
        Self::TrainingRunCheckpoints,
        Self::RolloutRuns,
        Self::WorkflowRuns,
        Self::FeatureRegistry,
    ];

    pub(crate) const fn name(self) -> &'static str {
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
            Self::TrainingRunMetrics => "training_run_metrics",
            Self::TrainingRunCheckpoints => "training_run_checkpoints",
            Self::RolloutRuns => "rollout_runs",
            Self::WorkflowRuns => "workflow_runs",
            Self::FeatureRegistry => "feature_registry",
        }
    }

    pub(crate) fn qualified_name(self, namespace: &str) -> String {
        format!("{namespace}.{}", self.name())
    }

    /// Build the default native-training query for one logical dataset.
    ///
    pub(crate) fn training_sql_for_dataset(self, namespace: &str, dataset_id: &str) -> String {
        let datasets = self.qualified_name(namespace);
        let samples = Self::JointSamples.qualified_name(namespace);
        let episodes = Self::Episodes.qualified_name(namespace);
        let streams = Self::EpisodeStreams.qualified_name(namespace);
        let escaped_dataset_id = dataset_id.replace('\'', "''");
        format!(
            "SELECT j.episode_id, j.stream_id, j.sample_index, j.episode_time_ns,\n       e.sample_hz, e.frames AS episode_frames, s.robot_type, s.joint_names,\n       j.state_position AS state,\n       j.action_position AS action\nFROM {samples} AS j\nJOIN {episodes} AS e ON e.episode_id = j.episode_id\nJOIN {datasets} AS d ON d.dataset_id = e.dataset_id\nJOIN {streams} AS s ON s.episode_id = j.episode_id AND s.stream_id = j.stream_id\nWHERE e.schema_version = {SCHEMA_VERSION}\n  AND d.dataset_id = '{escaped_dataset_id}'\n  AND NOT e.discarded\n  AND s.modality = 'joint'\n  AND (\n      LOWER(s.robot_type) LIKE '%follower%'\n      OR (s.robot_type IS NULL AND s.stream_id = 'follower')\n  )\n  AND j.state_valid\n  AND j.action_valid"
        )
    }

    /// The Data workbench's default query for this relation.
    pub(crate) fn sql(self, namespace: &str) -> String {
        let relation = self.qualified_name(namespace);
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
            // Ordinal zero is the newest retained run.
            Self::TrainingRuns => format!("SELECT * FROM {relation} ORDER BY ordinal LIMIT 100"),
            Self::TrainingRunMetrics | Self::TrainingRunCheckpoints => {
                format!("SELECT * FROM {relation} ORDER BY run_id, step LIMIT 100")
            }
            Self::RolloutRuns | Self::WorkflowRuns => {
                format!("SELECT * FROM {relation} ORDER BY updated_ms DESC LIMIT 100")
            }
            Self::FeatureRegistry => {
                format!("SELECT * FROM {relation} ORDER BY source_relation, feature_name LIMIT 100")
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod column {
    pub(crate) const STATE_VALID: &str = "state_valid";
    pub(crate) const ACTION_VALID: &str = "action_valid";
}
