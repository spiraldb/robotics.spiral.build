//! Physical relational schema owned by the SpiralDB storage adapter.
//!
//! User-visible catalog vocabulary lives in `spiral-robotics-common`. This module
//! owns persisted relation names, table prefix qualification, columns, and
//! schema-drift checks. Feature modules contribute their concrete Arrow
//! layouts through [`ManagedRelationSchema`].

use std::fmt;
use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};

include!(concat!(env!("OUT_DIR"), "/storage_schema_version.rs"));

/// Stable persisted relations owned by Spiral Robotics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Table {
    RecordingSources,
    TrainingCheckpoints,
    Robots,
    StudioSettings,
    TrainingRuns,
    TrainingRunMetrics,
    TrainingRunCheckpoints,
    RolloutRuns,
    Workflows,
    FeatureRegistry,
}

impl Table {
    /// Relations shown in the Data schema browser.
    ///
    /// Ordered so a parent relation always precedes the child relations that
    /// reference it; bundle import walks this order and deletion walks its
    /// reverse.
    pub(crate) const ALL: [Self; 10] = [
        Self::RecordingSources,
        Self::TrainingCheckpoints,
        Self::Robots,
        Self::StudioSettings,
        Self::TrainingRuns,
        Self::TrainingRunMetrics,
        Self::TrainingRunCheckpoints,
        Self::RolloutRuns,
        Self::Workflows,
        Self::FeatureRegistry,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::RecordingSources => "dataset_source",
            Self::TrainingCheckpoints => "training_checkpoints",
            Self::Robots => "robots",
            Self::StudioSettings => "studio_settings",
            Self::TrainingRuns => "training_runs",
            Self::TrainingRunMetrics => "training_run_metrics",
            Self::TrainingRunCheckpoints => "training_run_checkpoints",
            Self::RolloutRuns => "rollout_runs",
            Self::Workflows => "workflows",
            Self::FeatureRegistry => "feature_registry",
        }
    }

    /// Qualify this owned table using the configured Studio table prefix.
    pub(crate) fn qualified_name(self, table_prefix: &str) -> String {
        if self == Self::RecordingSources {
            return "dataset_source".into();
        }
        let suffix = if self == Self::StudioSettings {
            "settings"
        } else {
            self.name()
        };
        format!("{table_prefix}_{suffix}")
    }
}

impl fmt::Display for Table {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// One feature's contribution to the managed relational schema.
///
/// The database layer owns validation semantics, while each feature owns the
/// concrete Arrow layouts for its relations. Transfer and provisioning compose
/// these sections without teaching this layer about product features.
pub(crate) trait ManagedRelationSchema {
    fn relations(&self) -> &'static [Table];

    fn managed_relation_schema(&self, table: Table) -> Option<Schema>;

    fn validate_managed_relation_schema(
        &self,
        table: Table,
        schema: &Schema,
        label: &str,
    ) -> Option<Result<(), String>> {
        let expected = self.managed_relation_schema(table)?;
        Some(ManagedRelationDefinition::new(table, expected).validate(schema, label))
    }
}

struct ManagedRelationDefinition {
    table: Table,
    schema: Schema,
}

impl ManagedRelationDefinition {
    const fn new(table: Table, schema: Schema) -> Self {
        Self { table, schema }
    }

    fn validate(&self, schema: &Schema, label: &str) -> Result<(), String> {
        if !schema.metadata().is_empty() {
            return Err(format!(
                "{label} relation {} has unsupported schema metadata",
                self.table
            ));
        }
        if schema.fields().len() != self.schema.fields().len() {
            return Err(format!(
                "{label} relation {} has {} columns; expected {}",
                self.table,
                schema.fields().len(),
                self.schema.fields().len(),
            ));
        }
        for (actual, expected) in schema.fields().iter().zip(self.schema.fields()) {
            if !self.field_matches(actual, expected) {
                return Err(format!(
                    "{label} relation {} field {:?} has type {} (nullable {}); expected semantic type {} (nullable {})",
                    self.table,
                    actual.name(),
                    actual.data_type(),
                    actual.is_nullable(),
                    expected.data_type(),
                    expected.is_nullable(),
                ));
            }
        }
        Ok(())
    }

    fn field_matches(&self, actual: &Field, expected: &Field) -> bool {
        (actual.is_nullable() == expected.is_nullable())
            && actual.name() == expected.name()
            && actual.metadata() == expected.metadata()
            && actual
                .data_type()
                .matches_managed_data_type(expected.data_type())
    }
}

trait ManagedFieldCompatibility {
    fn matches_managed_field(&self, expected: &Field) -> bool;
}

impl ManagedFieldCompatibility for Field {
    fn matches_managed_field(&self, expected: &Field) -> bool {
        self.name() == expected.name()
            && self.is_nullable() == expected.is_nullable()
            && self.metadata() == expected.metadata()
            && self
                .data_type()
                .matches_managed_data_type(expected.data_type())
    }
}

trait ManagedDataTypeCompatibility {
    fn matches_managed_data_type(&self, expected: &DataType) -> bool;
}

impl ManagedDataTypeCompatibility for DataType {
    fn matches_managed_data_type(&self, expected: &DataType) -> bool {
        match expected {
            DataType::Utf8 => matches!(
                self,
                DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
            ),
            DataType::Binary | DataType::LargeBinary => matches!(
                self,
                DataType::Binary | DataType::LargeBinary | DataType::BinaryView
            ),
            DataType::List(expected_child) => {
                let DataType::List(actual_child) = self else {
                    return false;
                };
                actual_child.matches_managed_field(expected_child)
            }
            _ => self == expected,
        }
    }
}

/// Relations owned by the database mechanism rather than a product feature.
pub(crate) struct DatabaseBundleSection;

impl DatabaseBundleSection {
    const RELATIONS: [Table; 1] = [Table::Robots];
}

impl ManagedRelationSchema for DatabaseBundleSection {
    fn relations(&self) -> &'static [Table] {
        &Self::RELATIONS
    }

    fn managed_relation_schema(&self, table: Table) -> Option<Schema> {
        let text = |name, nullable| Field::new(name, DataType::Utf8, nullable);
        let timestamp = |name| Field::new(name, DataType::Int64, false);
        match table {
            Table::Robots => Some(Schema::new(vec![
                text(column::ROBOT_ID, false),
                text(column::CONFIG_JSON, false),
                timestamp(column::UPDATED_MS),
            ])),
            _ => None,
        }
    }
}

/// Persisted and query-result column names.
pub(crate) mod column {
    pub(crate) const ACTION_COLUMN: &str = "action_column";
    pub(crate) const ACTION_VALID: &str = "action_valid";
    pub(crate) const ACTION_POSITION: &str = "action_position";
    pub(crate) const ARROW_ENCODE_MS: &str = "arrow_encode_ms";
    pub(crate) const ATTENTION_HEADS: &str = "attention_heads";
    pub(crate) const BATCH_SIZE: &str = "batch_size";
    pub(crate) const BYTE_SIZE: &str = "byte_size";
    pub(crate) const CAMERA_ID: &str = "camera_id";
    pub(crate) const CAMERA_STREAMS: &str = "camera_streams";
    pub(crate) const CHECKPOINT: &str = "checkpoint";
    pub(crate) const CHECKPOINT_EVERY: &str = "checkpoint_every";
    pub(crate) const CHECKPOINT_STEP: &str = "checkpoint_step";
    pub(crate) const CHUNK_SIZE: &str = "chunk_size";
    pub(crate) const COMMAND_COUNT: &str = "command_count";
    /// Dataset selected by the operator in a training run's start request.
    /// Deliberately distinct from [`DATASET_ID`], which only ever links an
    /// episode to its owning dataset row.
    pub(crate) const CONFIG_DATASET_ID: &str = "config_dataset_id";
    pub(crate) const CONFIG_JSON: &str = "config_json";
    pub(crate) const CPU_PERCENT: &str = "cpu_percent";
    pub(crate) const CREATED_MS: &str = "created_ms";
    pub(crate) const CURRENT_STEP: &str = "current_step";
    pub(crate) const DATASET_ID: &str = "dataset_id";
    pub(crate) const DECODER_LAYERS: &str = "decoder_layers";
    pub(crate) const DEVICE: &str = "device";
    pub(crate) const DEVICE_COPY_MS: &str = "device_copy_ms";
    pub(crate) const DISCARDED: &str = "discarded";
    pub(crate) const DURATION_NS: &str = "duration_ns";
    pub(crate) const DURATION_SECONDS: &str = "duration_seconds";
    pub(crate) const EFFORT: &str = "effort";
    pub(crate) const ELAPSED_SECONDS: &str = "elapsed_seconds";
    pub(crate) const ENCODER_LAYERS: &str = "encoder_layers";
    pub(crate) const EPISODE_ID: &str = "episode_id";
    pub(crate) const EPISODE_TIME_NS: &str = "episode_time_ns";
    pub(crate) const EVENT: &str = "event";
    pub(crate) const EXAMPLES_PER_SECOND: &str = "examples_per_second";
    pub(crate) const FEEDFORWARD_DIM: &str = "feedforward_dim";
    pub(crate) const FINISHED_MS: &str = "finished_ms";
    pub(crate) const FOLLOW_ERROR_DEGREES: &str = "follow_error_degrees";
    pub(crate) const FPS: &str = "fps";
    pub(crate) const FRAMES: &str = "frames";
    pub(crate) const GPU_PERCENT: &str = "gpu_percent";
    pub(crate) const HEIGHT: &str = "height";
    pub(crate) const HIDDEN_DIM: &str = "hidden_dim";
    pub(crate) const IMAGE_HEIGHT: &str = "image_height";
    pub(crate) const IMAGE_WIDTH: &str = "image_width";
    pub(crate) const INFERENCE_COUNT: &str = "inference_count";
    pub(crate) const INFERENCE_READY: &str = "inference_ready";
    pub(crate) const JOINT_NAMES: &str = "joint_names";
    pub(crate) const KL_WEIGHT: &str = "kl_weight";
    pub(crate) const LAST_LATENCY_MS: &str = "last_latency_ms";
    pub(crate) const LAST_MODEL_MS: &str = "last_model_ms";
    pub(crate) const LATENT_DIM: &str = "latent_dim";
    pub(crate) const LEARNING_RATE: &str = "learning_rate";
    /// Torque ceiling one motor reports for itself, captured with the take.
    /// The `LIMIT_` prefix separates a motor's own configured threshold from
    /// the value actually measured during the recording.
    pub(crate) const LIMIT_MAX_LOAD: &str = "limit_max_load";
    pub(crate) const LIMIT_MAX_TEMPERATURE_CELSIUS: &str = "limit_max_temperature_celsius";
    pub(crate) const LIMIT_MAX_VOLTAGE_VOLTS: &str = "limit_max_voltage_volts";
    pub(crate) const LIMIT_MIN_VOLTAGE_VOLTS: &str = "limit_min_voltage_volts";
    pub(crate) const LOADER_MS: &str = "loader_ms";
    pub(crate) const LOGS: &str = "logs";
    pub(crate) const MAX_FOLLOW_ERROR_DEGREES: &str = "max_follow_error_degrees";
    pub(crate) const MAX_LATENCY_MS: &str = "max_latency_ms";
    pub(crate) const MEAN_LATENCY_MS: &str = "mean_latency_ms";
    pub(crate) const MEMORY_BYTES: &str = "memory_bytes";
    pub(crate) const MESSAGE: &str = "message";
    /// Lowest supply voltage measured on a motor across the whole take. See
    /// [`LIMIT_MIN_VOLTAGE_VOLTS`] for the motor's own configured floor.
    pub(crate) const MIN_VOLTAGE_VOLTS: &str = "min_voltage_volts";
    pub(crate) const MISSED_DEADLINES: &str = "missed_deadlines";
    pub(crate) const MODALITY: &str = "modality";
    pub(crate) const MODEL_MS: &str = "model_ms";
    pub(crate) const NOTE: &str = "note";
    pub(crate) const ORDINAL: &str = "ordinal";
    pub(crate) const PEAK_CURRENT_AMPS: &str = "peak_current_amps";
    pub(crate) const PEAK_LOAD: &str = "peak_load";
    pub(crate) const PEAK_TEMPERATURE_CELSIUS: &str = "peak_temperature_celsius";
    pub(crate) const PHASE: &str = "phase";
    pub(crate) const POLICY: &str = "policy";
    pub(crate) const PRETRAINED_BACKBONE: &str = "pretrained_backbone";
    pub(crate) const QUERY_SQL: &str = "query_sql";
    pub(crate) const RAW_POSITION: &str = "raw_position";
    pub(crate) const RECOMMENDED_ACTION_STEPS: &str = "recommended_action_steps";
    /// Start of one omitted wall-clock interval, on the gap-free episode clock.
    /// Parallel to [`RECORDING_GAP_DURATION_NS`]; both lists are equal length.
    pub(crate) const RECORDING_GAP_AFTER_NS: &str = "recording_gap_after_ns";
    pub(crate) const RECORDING_GAP_DURATION_NS: &str = "recording_gap_duration_ns";
    pub(crate) const ROBOT: &str = "robot";
    pub(crate) const ROBOT_TYPE: &str = "robot_type";
    pub(crate) const ROBOT_ID: &str = "robot_id";
    pub(crate) const RUN_ID: &str = "run_id";
    pub(crate) const RUN_NAME: &str = "run_name";
    pub(crate) const SAMPLE_HZ: &str = "sample_hz";
    pub(crate) const SAMPLE_INDEX: &str = "sample_index";
    pub(crate) const SCHEMA_VERSION: &str = "schema_version";
    pub(crate) const SEED: &str = "seed";
    pub(crate) const SESSION_ID: &str = "session_id";
    pub(crate) const SOURCE: &str = "source";
    /// Dataset a training run actually drew its episodes from, captured when
    /// the run started. See [`CONFIG_DATASET_ID`].
    pub(crate) const SOURCE_DATASET_ID: &str = "source_dataset_id";
    pub(crate) const STARTED_MS: &str = "started_ms";
    /// Motor temperature measured as the take opened. See
    /// [`PEAK_TEMPERATURE_CELSIUS`] for the extreme reached during it.
    pub(crate) const START_TEMPERATURE_CELSIUS: &str = "start_temperature_celsius";
    pub(crate) const START_VOLTAGE_VOLTS: &str = "start_voltage_volts";
    pub(crate) const SNAPSHOT_JSON: &str = "snapshot_json";
    pub(crate) const STATE_COLUMN: &str = "state_column";
    pub(crate) const STATE_VALID: &str = "state_valid";
    pub(crate) const STATE_POSITION: &str = "state_position";
    pub(crate) const STEP: &str = "step";
    pub(crate) const STEPS: &str = "steps";
    pub(crate) const STREAM_ID: &str = "stream_id";
    pub(crate) const TRAIN_LOSS: &str = "train_loss";
    pub(crate) const TRANSFER_BYTES_PER_SECOND: &str = "transfer_bytes_per_second";
    pub(crate) const UPLOAD_MS: &str = "upload_ms";
    pub(crate) const VAE_ENCODER_LAYERS: &str = "vae_encoder_layers";
    pub(crate) const VALIDATION_LOSS: &str = "validation_loss";
    pub(crate) const VALIDATION_SPLIT: &str = "validation_split";
    pub(crate) const VALUE_JSON: &str = "value_json";
    pub(crate) const VELOCITY: &str = "velocity";
    pub(crate) const VIDEO_DECODER: &str = "video_decoder";
    pub(crate) const WARNING: &str = "warning";
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
    /// Fully qualify the robot-configuration relation using the active table prefix.
    pub fn robot_configuration_relation(table_prefix: &str) -> String {
        Table::Robots.qualified_name(table_prefix)
    }

    /// Fully qualify the Workflow feature registry using the active table prefix.
    pub fn workflow_feature_registry_relation(table_prefix: &str) -> String {
        Table::FeatureRegistry.qualified_name(table_prefix)
    }

    /// Derive and qualify a Workflow feature cache using the active table prefix.
    pub fn workflow_feature_cache_relation(table_prefix: &str, source_relation: &str) -> String {
        format!(
            "{table_prefix}_{}",
            spiral_robotics_common::workflow::WorkflowFeatureApplicability::derived_cache_name(
                source_relation
            )
        )
    }

    /// Whether a qualified cache name has the storage-owned physical shape
    /// derived for this source relation.
    pub(crate) fn is_workflow_feature_cache_relation(
        relation: &str,
        source_relation: &str,
    ) -> bool {
        let suffix = format!(
            "_{}",
            spiral_robotics_common::workflow::WorkflowFeatureApplicability::derived_cache_name(
                source_relation
            )
        );
        relation
            .strip_suffix(&suffix)
            .is_some_and(Self::is_identifier)
    }

    /// Build one robot-configuration row against the managed relation schema.
    pub fn robot_configuration_batch(
        robot_identifier: String,
        configuration_json: String,
        updated_millis: i64,
    ) -> Result<RecordBatch, String> {
        RecordBatch::try_new(
            Arc::new(
                DatabaseBundleSection
                    .managed_relation_schema(Table::Robots)
                    .expect("the database section owns the robot relation"),
            ),
            vec![
                Arc::new(StringArray::from(vec![robot_identifier])) as ArrayRef,
                Arc::new(StringArray::from(vec![configuration_json])) as ArrayRef,
                Arc::new(Int64Array::from(vec![updated_millis])) as ArrayRef,
            ],
        )
        .map_err(|error| error.to_string())
    }

    /// Whether a value is safe to interpolate as an unquoted SQL identifier.
    pub fn is_identifier(value: &str) -> bool {
        spiral_robotics_common::sql::UnquotedSqlIdentifier::try_from(value).is_ok()
    }

    /// Whether a value is safe as a logical dataset row identifier.
    pub fn is_dataset_id(value: &str) -> bool {
        spiral_robotics_common::identity::DatasetId::is_canonical(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PublishedManagedRelation {
        name: &'static str,
        columns: &'static [&'static str],
    }

    const PUBLISHED_MANAGED_RELATIONS: [PublishedManagedRelation; 10] = [
        PublishedManagedRelation {
            name: "dataset_source",
            columns: &["dataset_name", "recording", "payload"],
        },
        PublishedManagedRelation {
            name: "training_checkpoints",
            columns: &[
                "run_id",
                "run_name",
                "policy",
                "step",
                "created_ms",
                "validation_loss",
                "config_json",
                "checkpoint",
            ],
        },
        PublishedManagedRelation {
            name: "robots",
            columns: &["robot_id", "config_json", "updated_ms"],
        },
        PublishedManagedRelation {
            name: "studio_settings",
            columns: &["key", "value_json", "updated_ms"],
        },
        PublishedManagedRelation {
            name: "training_runs",
            columns: &[
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
        },
        PublishedManagedRelation {
            name: "training_run_metrics",
            columns: &[
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
        },
        PublishedManagedRelation {
            name: "training_run_checkpoints",
            columns: &[
                "run_id",
                "step",
                "created_ms",
                "validation_loss",
                "byte_size",
                "inference_ready",
                "recommended_action_steps",
                "sample_hz",
            ],
        },
        PublishedManagedRelation {
            name: "rollout_runs",
            columns: &[
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
        },
        PublishedManagedRelation {
            name: "workflows",
            columns: &[
                "workflow_id",
                "name",
                "graph_version",
                "graph_json",
                "conversation_version",
                "conversation_json",
                "updated_ms",
            ],
        },
        PublishedManagedRelation {
            name: "feature_registry",
            columns: &[
                "source_relation",
                "cache_relation",
                "feature_name",
                "description",
                "concept",
                "media_field",
                "stream_ids",
                "producer_model_id",
                "producer_repository",
                "artifact_revision",
                "value_semantics",
                "definition_version",
                "created_at_ms",
                "updated_at_ms",
            ],
        },
    ];

    /// What a relation actually holds, read from the one production Arrow
    /// schema that both the database writes and a bundle validates against.
    fn persisted_columns(table: Table) -> Vec<String> {
        crate::transfer::ManagedRelationCatalog::schema(table)
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect()
    }

    #[test]
    fn every_persisted_table_declares_unique_columns() {
        for table in Table::ALL {
            let declared = persisted_columns(table);
            let mut columns = declared.clone();
            columns.sort_unstable();
            columns.dedup();
            assert_eq!(columns.len(), declared.len(), "{table}");
        }
    }

    /// The frontend's managed catalog is the browser-facing listing of these
    /// relations; a mismatch means the Data schema browser lies about what the
    /// database holds. This local copy pins that presentation contract against
    /// the production Arrow schemas.
    #[test]
    fn the_managed_catalog_mirrors_the_persisted_relations() {
        let persisted: Vec<&str> = Table::ALL.iter().map(|table| table.name()).collect();
        let published: Vec<&str> = PUBLISHED_MANAGED_RELATIONS
            .iter()
            .map(|relation| relation.name)
            .collect();
        assert_eq!(published, persisted, "relation names and browser order");

        for (table, published) in Table::ALL.into_iter().zip(PUBLISHED_MANAGED_RELATIONS) {
            assert_eq!(published.columns, persisted_columns(table), "{table}");
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
            spiral_robotics_common::workflow::WorkflowFeatureApplicability::derived_cache_name(
                "episode_37"
            ),
            "features_episode_37"
        );
        assert_ne!(
            spiral_robotics_common::workflow::WorkflowFeatureApplicability::derived_cache_name(
                "episode-37"
            ),
            spiral_robotics_common::workflow::WorkflowFeatureApplicability::derived_cache_name(
                "episode_37"
            )
        );
        assert_eq!(
            StorageSchema::workflow_feature_registry_relation("studio"),
            "studio_feature_registry"
        );
        let cache = StorageSchema::workflow_feature_cache_relation("studio", "media_frames");
        assert_eq!(cache, "studio_features_media_frames");
        assert!(StorageSchema::is_workflow_feature_cache_relation(
            &cache,
            "media_frames"
        ));
        assert!(!StorageSchema::is_workflow_feature_cache_relation(
            "other.nested.features_media_frames",
            "media_frames"
        ));
    }
}
