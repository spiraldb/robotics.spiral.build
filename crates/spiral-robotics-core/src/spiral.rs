//! Shared Spiral Robotics relational contract for SpiralDB.
//!
//! This module is deliberately dependency-free so the native backend and the
//! WebAssembly frontend use the same relation and column names. Arrow types,
//! ADBC execution, and SQL authorization remain backend concerns.

use std::fmt;

/// Schema version written into every completed episode row.
pub const SCHEMA_VERSION: u32 = 4;

/// Marker used to distinguish Spiral Robotics' managed dataset namespace resource.
pub const DATASET_NAMESPACE_MARKER: &str = "_spiral_robotics_dataset_key_v1";

/// Stable persisted relations owned by Spiral Robotics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Table {
    Episodes,
    EpisodeStreams,
    JointSamples,
    VideoAssets,
    TrainingCheckpoints,
}

impl Table {
    /// Relations published for each episode, in safe deletion order.
    pub const EPISODE_RELATIONS: [Self; 4] = [
        Self::Episodes,
        Self::JointSamples,
        Self::EpisodeStreams,
        Self::VideoAssets,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Episodes => "episodes",
            Self::EpisodeStreams => "episode_streams",
            Self::JointSamples => "joint_samples",
            Self::VideoAssets => "video_assets",
            Self::TrainingCheckpoints => "training_checkpoints",
        }
    }

    /// Ordered persisted columns for schema-drift checks.
    pub const fn columns(self) -> &'static [&'static str] {
        match self {
            Self::Episodes => &[
                column::EPISODE_ID,
                column::SCHEMA_VERSION,
                column::STARTED_MS,
                column::SAMPLE_HZ,
                column::SOURCE,
                column::NOTE,
                column::FRAMES,
                column::DURATION_NS,
                column::MAX_FOLLOW_ERROR_DEGREES,
                column::DISCARDED,
            ],
            Self::EpisodeStreams => &[
                column::EPISODE_ID,
                column::STREAM_ID,
                column::MODALITY,
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
    pub const ACTION_POSITION: &str = "action_position";
    pub const BYTE_SIZE: &str = "byte_size";
    pub const CAMERA_ID: &str = "camera_id";
    pub const CHECKPOINT: &str = "checkpoint";
    pub const CONFIG_JSON: &str = "config_json";
    pub const CREATED_MS: &str = "created_ms";
    pub const DISCARDED: &str = "discarded";
    pub const DURATION_NS: &str = "duration_ns";
    pub const EFFORT: &str = "effort";
    pub const EPISODE_ID: &str = "episode_id";
    pub const EPISODE_TIME_NS: &str = "episode_time_ns";
    pub const FOLLOW_ERROR_DEGREES: &str = "follow_error_degrees";
    pub const FPS: &str = "fps";
    pub const FRAMES: &str = "frames";
    pub const HEIGHT: &str = "height";
    pub const JOINT_NAMES: &str = "joint_names";
    pub const MAX_FOLLOW_ERROR_DEGREES: &str = "max_follow_error_degrees";
    pub const MODALITY: &str = "modality";
    pub const NOTE: &str = "note";
    pub const PAYLOAD: &str = "payload";
    pub const POLICY: &str = "policy";
    pub const RAW_POSITION: &str = "raw_position";
    pub const REPLAY_POSITION: &str = "replay_position";
    pub const RUN_ID: &str = "run_id";
    pub const RUN_NAME: &str = "run_name";
    pub const SAMPLE_HZ: &str = "sample_hz";
    pub const SAMPLE_INDEX: &str = "sample_index";
    pub const SCHEMA_VERSION: &str = "schema_version";
    pub const SEGMENT_INDEX: &str = "segment_index";
    pub const SOURCE: &str = "source";
    pub const STARTED_MS: &str = "started_ms";
    pub const STATE: &str = "state";
    pub const STATE_POSITION: &str = "state_position";
    pub const STEP: &str = "step";
    pub const STREAM_ID: &str = "stream_id";
    pub const TIMELINE_OFFSET_NS: &str = "timeline_offset_ns";
    pub const VALIDATION_LOSS: &str = "validation_loss";
    pub const VELOCITY: &str = "velocity";
    pub const WIDTH: &str = "width";
}

/// Built-in read-only queries shown by the Data workbench.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryPreset {
    Episodes,
    EpisodeStreams,
    JointSamples,
    VideoAssets,
}

impl QueryPreset {
    pub const ALL: [Self; 4] = [
        Self::Episodes,
        Self::EpisodeStreams,
        Self::JointSamples,
        Self::VideoAssets,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Episodes => "Episodes",
            Self::EpisodeStreams => "Streams",
            Self::JointSamples => "Joint samples",
            Self::VideoAssets => "Video assets",
        }
    }

    pub const fn table(self) -> Table {
        match self {
            Self::Episodes => Table::Episodes,
            Self::EpisodeStreams => Table::EpisodeStreams,
            Self::JointSamples => Table::JointSamples,
            Self::VideoAssets => Table::VideoAssets,
        }
    }

    pub fn sql(self, dataset: &str) -> String {
        let relation = relation(dataset, self.table());
        match self {
            Self::Episodes => format!(
                "SELECT * FROM {relation} ORDER BY {} DESC LIMIT 100",
                column::STARTED_MS
            ),
            Self::EpisodeStreams => format!(
                "SELECT * FROM {relation} ORDER BY {} DESC, {} LIMIT 100",
                column::EPISODE_ID,
                column::STREAM_ID
            ),
            Self::JointSamples => format!(
                "SELECT * FROM {relation} ORDER BY {} DESC, {}, {} LIMIT 100",
                column::EPISODE_ID,
                column::STREAM_ID,
                column::SAMPLE_INDEX
            ),
            Self::VideoAssets => format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {} FROM {relation} ORDER BY {} DESC, {} LIMIT 100",
                column::EPISODE_ID,
                column::STREAM_ID,
                column::CAMERA_ID,
                column::BYTE_SIZE,
                column::WIDTH,
                column::HEIGHT,
                column::FPS,
                column::FRAMES,
                column::EPISODE_ID,
                column::STREAM_ID,
            ),
        }
    }
}

/// Qualify one owned table inside a configured dataset.
pub fn relation(dataset: &str, table: Table) -> String {
    format!("{dataset}.{}", table.name())
}

/// Default typed sample projection used by ACT training.
pub fn default_training_sql(dataset: &str) -> String {
    let relation = relation(dataset, Table::JointSamples);
    format!(
        "SELECT {episode_id}, {stream_id}, {sample_index},\n       {state_position} AS {state},\n       {action_position} AS {action}\nFROM {relation}\nWHERE {action_position} IS NOT NULL\nORDER BY {episode_id}, {stream_id}, {sample_index}",
        episode_id = column::EPISODE_ID,
        stream_id = column::STREAM_ID,
        sample_index = column::SAMPLE_INDEX,
        state_position = column::STATE_POSITION,
        state = column::STATE,
        action_position = column::ACTION_POSITION,
        action = column::ACTION,
    )
}

/// Whether a value is safe to interpolate as an unquoted SQL identifier.
pub fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Convert an external immutable ID into one component of a generated name.
pub fn identifier_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn training_query_comes_from_the_shared_relation_contract() {
        let sql = default_training_sql("robotics");
        assert!(sql.contains(&format!(
            "FROM {}",
            relation("robotics", Table::JointSamples)
        )));
        assert!(sql.contains(&format!("{} AS {}", column::STATE_POSITION, column::STATE)));
        assert!(sql.contains(&format!(
            "{} AS {}",
            column::ACTION_POSITION,
            column::ACTION
        )));
    }

    #[test]
    fn every_persisted_table_declares_unique_columns() {
        for table in [
            Table::Episodes,
            Table::EpisodeStreams,
            Table::JointSamples,
            Table::VideoAssets,
            Table::TrainingCheckpoints,
        ] {
            let mut columns = table.columns().to_vec();
            columns.sort_unstable();
            columns.dedup();
            assert_eq!(columns.len(), table.columns().len(), "{table}");
        }
    }

    #[test]
    fn identifiers_follow_the_sql_contract() {
        assert!(is_identifier("spiral_robotics_v3"));
        assert!(!is_identifier("spiral-robotics-v3"));
        assert!(!is_identifier("3spiral_robotics"));
    }
}
