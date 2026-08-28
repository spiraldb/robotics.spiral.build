//! User-visible catalog vocabulary shared by the Data and Training activities.
//!
//! Physical Arrow schemas and native database operations live in `spiral-robotics-storage`;
//! this module only publishes the managed relation names and query templates
//! that cross the frontend/backend boundary. A storage-side test pins every
//! name and column list here to the persisted Arrow schemas, so this listing
//! cannot silently drift from what the database actually holds.

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
    TrainingRunMetrics,
    TrainingRunCheckpoints,
    RolloutRuns,
    WorkflowRuns,
}

impl ManagedTable {
    /// Every managed relation, in the Data schema browser's display order —
    /// kept identical to storage's own relation order.
    pub const ALL: [Self; 13] = [
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
            Self::TrainingRunMetrics => "training_run_metrics",
            Self::TrainingRunCheckpoints => "training_run_checkpoints",
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
                "recording_gap_after_ns",
                "recording_gap_duration_ns",
            ],
            Self::EpisodeStreams => &[
                "episode_id",
                "stream_id",
                "ordinal",
                "modality",
                "robot_type",
                "joint_names",
                "camera_id",
                "width",
                "height",
                "fps",
                "start_temperature_celsius",
                "start_voltage_volts",
                "limit_max_temperature_celsius",
                "limit_min_voltage_volts",
                "limit_max_voltage_volts",
                "limit_max_load",
                "peak_temperature_celsius",
                "min_voltage_volts",
                "peak_load",
                "peak_current_amps",
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
            Self::TrainingRuns => &[
                "run_id",
                "ordinal",
                "run_name",
                "config_dataset_id",
                "source_dataset_id",
                "policy",
                "query_sql",
                "state_column",
                "action_column",
                "device",
                "steps",
                "batch_size",
                "validation_split",
                "learning_rate",
                "checkpoint_every",
                "chunk_size",
                "seed",
                "camera_streams",
                "image_width",
                "image_height",
                "video_decoder",
                "pretrained_backbone",
                "hidden_dim",
                "latent_dim",
                "encoder_layers",
                "vae_encoder_layers",
                "decoder_layers",
                "attention_heads",
                "feedforward_dim",
                "kl_weight",
                "phase",
                "current_step",
                "started_ms",
                "finished_ms",
                "message",
                "logs",
            ],
            Self::TrainingRunMetrics => &[
                "run_id",
                "step",
                "train_loss",
                "validation_loss",
                "learning_rate",
                "elapsed_seconds",
                "event",
                "transfer_bytes_per_second",
                "examples_per_second",
                "cpu_percent",
                "gpu_percent",
                "memory_bytes",
                "loader_ms",
                "arrow_encode_ms",
                "upload_ms",
                "device_copy_ms",
                "model_ms",
            ],
            Self::TrainingRunCheckpoints => &[
                "run_id",
                "step",
                "created_ms",
                "validation_loss",
                "byte_size",
                "inference_ready",
                "recommended_action_steps",
                "sample_hz",
            ],
            Self::RolloutRuns => &[
                "session_id",
                "run_id",
                "checkpoint_step",
                "robot",
                "duration_seconds",
                "phase",
                "started_ms",
                "finished_ms",
                "message",
                "warning",
                "inference_count",
                "command_count",
                "missed_deadlines",
                "last_latency_ms",
                "last_model_ms",
                "mean_latency_ms",
                "max_latency_ms",
                "updated_ms",
            ],
            Self::WorkflowRuns => &["run_id", "document_version", "snapshot_json", "updated_ms"],
        }
    }

    /// Build the default native-training query for one logical dataset.
    ///
    pub fn training_sql_for_dataset(self, namespace: &str, dataset_id: &str) -> String {
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
    pub fn sql(self, namespace: &str) -> String {
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
        }
    }
}

pub mod column {
    pub const STATE_VALID: &str = "state_valid";
    pub const ACTION_VALID: &str = "action_valid";
}
