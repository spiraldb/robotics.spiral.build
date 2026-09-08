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
}

impl ManagedTable {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Datasets => "datasets",
            Self::Episodes => "episodes",
            Self::EpisodeStreams => "episode_streams",
            Self::JointSamples => "joint_samples",
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
}

#[cfg(test)]
pub(crate) mod column {
    pub(crate) const STATE_VALID: &str = "state_valid";
    pub(crate) const ACTION_VALID: &str = "action_valid";
}
