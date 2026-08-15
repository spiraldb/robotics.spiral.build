//! Shared Spiral Robotics relational contract for SpiralDB adapters.
//!
//! This module keeps the native backend and WebAssembly frontend on the same
//! relation and column names. Arrow types, ADBC execution, and SQL construction
//! remain backend concerns; read-only authorization belongs to core policy.

use std::fmt;

/// Schema version written into every completed episode row.
pub const SCHEMA_VERSION: u32 = 7;

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
}

impl Table {
    /// Relations shown in the Data schema browser.
    pub const ALL: [Self; 10] = [
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
        }
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
            Self::TrainingRuns | Self::RolloutRuns => {
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

/// Built-in read-only queries shown by the Data workbench.
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
}

impl QueryPreset {
    pub const ALL: [Self; 10] = [
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
        }
    }

    pub const fn table(self) -> Table {
        match self {
            Self::Datasets => Table::Datasets,
            Self::Episodes => Table::Episodes,
            Self::EpisodeStreams => Table::EpisodeStreams,
            Self::JointSamples => Table::JointSamples,
            Self::VideoAssets => Table::VideoAssets,
            Self::TrainingCheckpoints => Table::TrainingCheckpoints,
            Self::Robots => Table::Robots,
            Self::StudioSettings => Table::StudioSettings,
            Self::TrainingRuns => Table::TrainingRuns,
            Self::RolloutRuns => Table::RolloutRuns,
        }
    }

    pub fn sql(self, dataset: &str) -> String {
        let relation = relation(dataset, self.table());
        match self {
            Self::Datasets => format!(
                "SELECT * FROM {relation} ORDER BY {} DESC LIMIT 100",
                column::CREATED_MS
            ),
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
            Self::TrainingCheckpoints => format!(
                "SELECT {}, {}, {}, {}, {}, {} FROM {relation} ORDER BY {} DESC LIMIT 100",
                column::RUN_ID,
                column::RUN_NAME,
                column::POLICY,
                column::STEP,
                column::CREATED_MS,
                column::VALIDATION_LOSS,
                column::CREATED_MS,
            ),
            Self::Robots => format!(
                "SELECT * FROM {relation} ORDER BY {} LIMIT 100",
                column::ROBOT_ID,
            ),
            Self::StudioSettings => format!(
                "SELECT * FROM {relation} ORDER BY {} LIMIT 100",
                column::KEY,
            ),
            Self::TrainingRuns | Self::RolloutRuns => format!(
                "SELECT * FROM {relation} ORDER BY {} DESC LIMIT 100",
                column::UPDATED_MS,
            ),
        }
    }
}

/// Qualify one owned table inside a configured dataset.
pub fn relation(dataset: &str, table: Table) -> String {
    format!("{dataset}.{}", table.name())
}

/// Default typed sample projection used by ACT training.
pub fn default_training_sql(namespace: &str) -> String {
    default_training_sql_for_dataset(namespace, "default")
}

/// Default typed sample projection for one relational dataset row.
pub fn default_training_sql_for_dataset(namespace: &str, dataset_id: &str) -> String {
    let datasets = relation(namespace, Table::Datasets);
    let samples = relation(namespace, Table::JointSamples);
    let episodes = relation(namespace, Table::Episodes);
    let streams = relation(namespace, Table::EpisodeStreams);
    let dataset_id = dataset_id.replace('\'', "''");
    format!(
        "SELECT j.{episode_id}, j.{stream_id}, j.{sample_index}, j.{episode_time_ns},\n       e.{sample_hz}, e.{frames} AS episode_frames, s.{robot_type}, s.{joint_names},\n       j.{state_position} AS {state},\n       j.{action_position} AS {action}\nFROM {samples} AS j\nJOIN {episodes} AS e ON e.{episode_id} = j.{episode_id}\nJOIN {datasets} AS d ON d.{dataset_id} = e.{dataset_id}\nJOIN {streams} AS s ON s.{episode_id} = j.{episode_id} AND s.{stream_id} = j.{stream_id}\nWHERE e.{schema_version} = {current_schema}\n  AND d.{dataset_id} = '{dataset_value}'\n  AND NOT e.{discarded}\n  AND s.{modality} = 'joint'\n  AND (\n      LOWER(s.{robot_type}) LIKE '%follower%'\n      OR (s.{robot_type} IS NULL AND s.{stream_id} = 'follower')\n  )\n  AND j.{state_valid}\n  AND j.{action_valid}",
        episode_id = column::EPISODE_ID,
        stream_id = column::STREAM_ID,
        sample_index = column::SAMPLE_INDEX,
        episode_time_ns = column::EPISODE_TIME_NS,
        sample_hz = column::SAMPLE_HZ,
        frames = column::FRAMES,
        robot_type = column::ROBOT_TYPE,
        joint_names = column::JOINT_NAMES,
        state_position = column::STATE_POSITION,
        state = column::STATE,
        action_position = column::ACTION_POSITION,
        action = column::ACTION,
        samples = samples,
        episodes = episodes,
        streams = streams,
        datasets = datasets,
        dataset_id = column::DATASET_ID,
        dataset_value = dataset_id,
        schema_version = column::SCHEMA_VERSION,
        current_schema = SCHEMA_VERSION,
        discarded = column::DISCARDED,
        modality = column::MODALITY,
        state_valid = column::STATE_VALID,
        action_valid = column::ACTION_VALID,
    )
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
    let rendered = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if rendered == value && !rendered.is_empty() {
        return rendered;
    }
    // Replacing punctuation alone would alias IDs such as `episode-1` and
    // `episode_1`. A deterministic suffix keeps generated resource names both
    // SQL-safe and one-to-one without leaking this convention to readers.
    let hash = value
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("{rendered}_{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn training_query_comes_from_the_shared_relation_contract() {
        let sql = default_training_sql_for_dataset("robotics", "pick-place");
        assert!(sql.contains(&format!(
            "FROM {}",
            relation("robotics", Table::JointSamples)
        )));
        assert!(sql.contains(&format!(
            "JOIN {} AS d",
            relation("robotics", Table::Datasets)
        )));
        assert!(sql.contains("d.dataset_id = 'pick-place'"));
        assert!(sql.contains(&format!("{} AS {}", column::STATE_POSITION, column::STATE)));
        assert!(sql.contains(&format!(
            "{} AS {}",
            column::ACTION_POSITION,
            column::ACTION
        )));
        assert!(sql.contains("robot_type IS NULL AND s.stream_id = 'follower'"));
    }

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
        assert_eq!(QueryPreset::ALL.len(), Table::ALL.len());
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
