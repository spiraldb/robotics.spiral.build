//! Physical relational schema owned by the SpiralDB storage adapter.
//!
//! User-visible catalog vocabulary lives in `spiral-robotics-api`. This module
//! owns persisted relation names, columns, and schema-drift checks.

use std::fmt;

include!(concat!(env!("OUT_DIR"), "/storage_schema_version.rs"));

/// Marker used to distinguish Spiral Robotics' managed dataset namespace resource.
pub(crate) const DATASET_NAMESPACE_MARKER: &str = "_spiral_robotics_dataset_key_v1";

/// Stable persisted relations owned by Spiral Robotics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Table {
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

impl Table {
    /// Relations shown in the Data schema browser.
    pub(crate) const ALL: [Self; 11] = [
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

    /// Relations published for each episode, in safe deletion order.
    pub(crate) const EPISODE_RELATIONS: [Self; 4] = [
        Self::Episodes,
        Self::JointSamples,
        Self::EpisodeStreams,
        Self::VideoAssets,
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
            Self::RolloutRuns => "rollout_runs",
            Self::WorkflowRuns => "workflow_runs",
        }
    }

    /// Qualify this owned table inside a configured dataset.
    pub(crate) fn qualified_name(self, dataset: &str) -> String {
        format!("{dataset}.{}", self.name())
    }

    #[cfg(test)]
    pub(crate) const fn columns(self) -> &'static [&'static str] {
        match self {
            Self::Datasets => &[
                column::DATASET_ID,
                column::NAME,
                column::CREATED_MS,
                column::DESCRIPTION,
            ],
            Self::Episodes => &[
                column::EPISODE_ID,
                column::DATASET_ID,
                column::SCHEMA_VERSION,
                column::STARTED_MS,
                column::SAMPLE_HZ,
                column::SOURCE,
                column::NOTE,
                column::FRAMES,
                column::DURATION_NS,
                column::MAX_FOLLOW_ERROR_DEGREES,
                column::DISCARDED,
                column::METADATA_JSON,
            ],
            Self::EpisodeStreams => &[
                column::EPISODE_ID,
                column::STREAM_ID,
                column::MODALITY,
                column::ROBOT_TYPE,
                column::JOINT_NAMES,
                column::CAMERA_ID,
                column::WIDTH,
                column::HEIGHT,
                column::FPS,
            ],
            Self::JointSamples => &[
                column::EPISODE_ID,
                column::STREAM_ID,
                column::SAMPLE_INDEX,
                column::EPISODE_TIME_NS,
                column::STATE_VALID,
                column::ACTION_VALID,
                column::STATE_POSITION,
                column::ACTION_POSITION,
                column::RAW_POSITION,
                column::VELOCITY,
                column::EFFORT,
                column::FOLLOW_ERROR_DEGREES,
            ],
            Self::VideoAssets => &[
                column::EPISODE_ID,
                column::STREAM_ID,
                column::SEGMENT_INDEX,
                column::TIMELINE_OFFSET_NS,
                column::CAMERA_ID,
                column::PAYLOAD,
                column::BYTE_SIZE,
                column::WIDTH,
                column::HEIGHT,
                column::FPS,
                column::FRAMES,
                column::MEDIA_RELATION,
            ],
            Self::TrainingCheckpoints => &[
                column::RUN_ID,
                column::RUN_NAME,
                column::POLICY,
                column::STEP,
                column::CREATED_MS,
                column::VALIDATION_LOSS,
                column::CONFIG_JSON,
                column::CHECKPOINT,
            ],
            Self::Robots => &[column::ROBOT_ID, column::CONFIG_JSON, column::UPDATED_MS],
            Self::StudioSettings => &[column::KEY, column::VALUE_JSON, column::UPDATED_MS],
            Self::TrainingRuns | Self::RolloutRuns | Self::WorkflowRuns => {
                &[column::RUN_ID, column::SNAPSHOT_JSON, column::UPDATED_MS]
            }
        }
    }
}

impl fmt::Display for Table {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Persisted and query-result column names.
pub(crate) mod column {
    pub(crate) const ACTION_VALID: &str = "action_valid";
    pub(crate) const ACTION_POSITION: &str = "action_position";
    pub(crate) const BYTE_SIZE: &str = "byte_size";
    pub(crate) const CAMERA_ID: &str = "camera_id";
    pub(crate) const CHECKPOINT: &str = "checkpoint";
    pub(crate) const CONFIG_JSON: &str = "config_json";
    pub(crate) const CREATED_MS: &str = "created_ms";
    pub(crate) const DATASET_ID: &str = "dataset_id";
    pub(crate) const DESCRIPTION: &str = "description";
    pub(crate) const DISCARDED: &str = "discarded";
    pub(crate) const DURATION_NS: &str = "duration_ns";
    pub(crate) const EFFORT: &str = "effort";
    pub(crate) const EPISODE_ID: &str = "episode_id";
    pub(crate) const EPISODE_TIME_NS: &str = "episode_time_ns";
    pub(crate) const FOLLOW_ERROR_DEGREES: &str = "follow_error_degrees";
    pub(crate) const FPS: &str = "fps";
    pub(crate) const FRAMES: &str = "frames";
    pub(crate) const HEIGHT: &str = "height";
    pub(crate) const JOINT_NAMES: &str = "joint_names";
    pub(crate) const MAX_FOLLOW_ERROR_DEGREES: &str = "max_follow_error_degrees";
    pub(crate) const METADATA_JSON: &str = "metadata_json";
    pub(crate) const MEDIA_RELATION: &str = "media_relation";
    pub(crate) const MODALITY: &str = "modality";
    pub(crate) const NAME: &str = "name";
    pub(crate) const NOTE: &str = "note";
    pub(crate) const PAYLOAD: &str = "payload";
    pub(crate) const POLICY: &str = "policy";
    pub(crate) const RAW_POSITION: &str = "raw_position";
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) const REPLAY_POSITION: &str = "replay_position";
    pub(crate) const ROBOT_TYPE: &str = "robot_type";
    pub(crate) const ROBOT_ID: &str = "robot_id";
    pub(crate) const RUN_ID: &str = "run_id";
    pub(crate) const RUN_NAME: &str = "run_name";
    pub(crate) const SAMPLE_HZ: &str = "sample_hz";
    pub(crate) const SAMPLE_INDEX: &str = "sample_index";
    pub(crate) const SCHEMA_VERSION: &str = "schema_version";
    pub(crate) const SEGMENT_INDEX: &str = "segment_index";
    pub(crate) const SOURCE: &str = "source";
    pub(crate) const STARTED_MS: &str = "started_ms";
    pub(crate) const SNAPSHOT_JSON: &str = "snapshot_json";
    pub(crate) const STATE_VALID: &str = "state_valid";
    pub(crate) const STATE_POSITION: &str = "state_position";
    pub(crate) const STEP: &str = "step";
    pub(crate) const STREAM_ID: &str = "stream_id";
    pub(crate) const TIMELINE_OFFSET_NS: &str = "timeline_offset_ns";
    pub(crate) const VALIDATION_LOSS: &str = "validation_loss";
    pub(crate) const VALUE_JSON: &str = "value_json";
    pub(crate) const VELOCITY: &str = "velocity";
    pub(crate) const WIDTH: &str = "width";
    pub(crate) const KEY: &str = "key";
    pub(crate) const UPDATED_MS: &str = "updated_ms";
}

/// Column carrying a persisted robot's stable identifier.
pub const ROBOT_IDENTIFIER_COLUMN: &str = column::ROBOT_ID;
/// Column carrying a persisted robot's validated configuration document.
pub const ROBOT_CONFIGURATION_COLUMN: &str = column::CONFIG_JSON;
/// Column carrying the robot configuration's last-update timestamp.
pub const ROBOT_UPDATED_MILLIS_COLUMN: &str = column::UPDATED_MS;

/// Public entry point for physical storage-schema naming and validation.
pub struct StorageSchema;

impl StorageSchema {
    /// Fully qualify the robot-configuration relation in the active namespace.
    pub fn robot_configuration_relation(namespace: &str) -> String {
        Table::Robots.qualified_name(namespace)
    }

    /// Whether a value is safe to interpolate as an unquoted SQL identifier.
    pub fn is_identifier(value: &str) -> bool {
        spiral_robotics_api::sql::UnquotedSqlIdentifier::try_from(value).is_ok()
    }

    /// Whether a value is safe as a logical dataset row identifier.
    pub fn is_dataset_id(value: &str) -> bool {
        spiral_robotics_api::identity::DatasetId::is_canonical(value)
    }

    /// Convert an external immutable ID into one component of a generated name.
    #[cfg(test)]
    fn identifier_component(value: &str) -> String {
        spiral_robotics_api::identity::SqlNameComponent::from(value).into_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_persisted_table_declares_unique_columns() {
        for table in Table::ALL {
            let mut columns = table.columns().to_vec();
            columns.sort_unstable();
            columns.dedup();
            assert_eq!(columns.len(), table.columns().len(), "{table}");
        }
    }

    #[test]
    fn episodes_reference_the_dataset_table() {
        assert_eq!(Table::Datasets.columns().first(), Some(&column::DATASET_ID));
        assert_eq!(Table::Episodes.columns().get(1), Some(&column::DATASET_ID));
        for table in Table::ALL {
            if table != Table::Datasets && table != Table::Episodes {
                assert!(
                    !table.columns().contains(&column::DATASET_ID),
                    "{table} must reach datasets through episodes"
                );
            }
        }
    }

    #[test]
    fn identifiers_follow_the_sql_contract() {
        assert!(StorageSchema::is_identifier("spiral_robotics_v3"));
        assert!(!StorageSchema::is_identifier("spiral-robotics-v3"));
        assert!(!StorageSchema::is_identifier("3spiral_robotics"));
        assert!(StorageSchema::is_dataset_id("2026.pick-place_v1"));
        assert!(!StorageSchema::is_dataset_id("pick place"));
        assert!(!StorageSchema::is_dataset_id(""));
        assert_eq!(
            StorageSchema::identifier_component("episode_37"),
            "episode_37"
        );
        assert_ne!(
            StorageSchema::identifier_component("episode-37"),
            StorageSchema::identifier_component("episode_37")
        );
    }
}
