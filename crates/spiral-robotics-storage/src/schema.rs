//! Physical relational schema owned by the SpiralDB storage adapter.
//!
//! User-visible catalog vocabulary lives in `spiral-robotics-api`. This module
//! owns persisted relation names, columns, and schema-drift checks.

use std::fmt;

include!(concat!(env!("OUT_DIR"), "/spiral_boundary.rs"));

/// Marker used to distinguish Spiral Robotics' managed dataset namespace resource.
pub const DATASET_NAMESPACE_MARKER: &str = "_spiral_robotics_dataset_key_v1";

/// Stable persisted relations owned by Spiral Robotics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Table {
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

    /// Relations published for each episode, in safe deletion order.
    pub const EPISODE_RELATIONS: [Self; 4] = [
        Self::Episodes,
        Self::JointSamples,
        Self::EpisodeStreams,
        Self::VideoAssets,
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

    /// Qualify this owned table inside a configured dataset.
    pub fn qualified_name(self, dataset: &str) -> String {
        format!("{dataset}.{}", self.name())
    }

    /// Ordered persisted columns for schema-drift checks.
    pub const fn columns(self) -> &'static [&'static str] {
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
pub mod column {
    pub const ACTION: &str = "action";
    pub const ACTION_VALID: &str = "action_valid";
    pub const ACTION_POSITION: &str = "action_position";
    pub const BYTE_SIZE: &str = "byte_size";
    pub const CAMERA_ID: &str = "camera_id";
    pub const CHECKPOINT: &str = "checkpoint";
    pub const CONFIG_JSON: &str = "config_json";
    pub const CREATED_MS: &str = "created_ms";
    pub const DATASET_ID: &str = "dataset_id";
    pub const DESCRIPTION: &str = "description";
    pub const DISCARDED: &str = "discarded";
    pub const DURATION_NS: &str = "duration_ns";
    pub const EFFORT: &str = "effort";
    pub const EPISODE_ID: &str = "episode_id";
    pub const EPISODE_FRAMES: &str = "episode_frames";
    pub const EPISODE_TIME_NS: &str = "episode_time_ns";
    pub const FOLLOW_ERROR_DEGREES: &str = "follow_error_degrees";
    pub const FPS: &str = "fps";
    pub const FRAMES: &str = "frames";
    pub const HEIGHT: &str = "height";
    pub const JOINT_NAMES: &str = "joint_names";
    pub const MAX_FOLLOW_ERROR_DEGREES: &str = "max_follow_error_degrees";
    pub const METADATA_JSON: &str = "metadata_json";
    pub const MEDIA_RELATION: &str = "media_relation";
    pub const MODALITY: &str = "modality";
    pub const NAME: &str = "name";
    pub const NOTE: &str = "note";
    pub const PAYLOAD: &str = "payload";
    pub const POLICY: &str = "policy";
    pub const RAW_POSITION: &str = "raw_position";
    pub const ROBOT_TYPE: &str = "robot_type";
    pub const ROBOT_ID: &str = "robot_id";
    pub const REPLAY_POSITION: &str = "replay_position";
    pub const RUN_ID: &str = "run_id";
    pub const RUN_NAME: &str = "run_name";
    pub const SAMPLE_HZ: &str = "sample_hz";
    pub const SAMPLE_INDEX: &str = "sample_index";
    pub const SCHEMA_VERSION: &str = "schema_version";
    pub const SEGMENT_INDEX: &str = "segment_index";
    pub const SOURCE: &str = "source";
    pub const STARTED_MS: &str = "started_ms";
    pub const SNAPSHOT_JSON: &str = "snapshot_json";
    pub const STATE: &str = "state";
    pub const STATE_VALID: &str = "state_valid";
    pub const STATE_POSITION: &str = "state_position";
    pub const STEP: &str = "step";
    pub const STREAM_ID: &str = "stream_id";
    pub const TIMELINE_OFFSET_NS: &str = "timeline_offset_ns";
    pub const VALIDATION_LOSS: &str = "validation_loss";
    pub const VALUE_JSON: &str = "value_json";
    pub const VELOCITY: &str = "velocity";
    pub const WIDTH: &str = "width";
    pub const KEY: &str = "key";
    pub const UPDATED_MS: &str = "updated_ms";
}

/// Whether a value is safe to interpolate as an unquoted SQL identifier.
pub fn is_identifier(value: &str) -> bool {
    spiral_robotics_core::query::is_identifier(value)
}

/// Whether a value is safe as a logical dataset row identifier.
pub fn is_dataset_id(value: &str) -> bool {
    spiral_robotics_core::identity::DatasetId::parse(value)
        .is_ok_and(|identity| identity.as_str() == value)
}

/// Convert an external immutable ID into one component of a generated name.
pub fn identifier_component(value: &str) -> String {
    spiral_robotics_core::identity::identifier_component(value)
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
        assert!(is_identifier("spiral_robotics_v3"));
        assert!(!is_identifier("spiral-robotics-v3"));
        assert!(!is_identifier("3spiral_robotics"));
        assert!(is_dataset_id("2026.pick-place_v1"));
        assert!(!is_dataset_id("pick place"));
        assert!(!is_dataset_id(""));
        assert_eq!(identifier_component("episode_37"), "episode_37");
        assert_ne!(
            identifier_component("episode-37"),
            identifier_component("episode_37")
        );
    }
}
