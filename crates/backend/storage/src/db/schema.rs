//! Arrow schemas for Spiral data-plane tables.

use std::fmt;

use arrow_schema::{DataType, Field, Schema};

include!(concat!(env!("OUT_DIR"), "/storage_schema_version.rs"));

/// Data-plane tables included in project bundles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Table {
    DatasetSources,
    TrainingCheckpoints,
}

impl Table {
    pub(crate) const ALL: [Self; 2] = [Self::DatasetSources, Self::TrainingCheckpoints];

    pub(crate) fn schema(self) -> Schema {
        match self {
            Self::DatasetSources => crate::datasets::McapSources::bundle_schema(),
            Self::TrainingCheckpoints => crate::training::CheckpointArtifact::schema(),
        }
    }

    pub(crate) fn validate(self, schema: &Schema, label: &str) -> Result<(), String> {
        ManagedRelationDefinition::new(self, self.schema()).validate(schema, label)
    }

    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|table| table.name() == name)
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::DatasetSources => "dataset_source",
            Self::TrainingCheckpoints => "training_checkpoints",
        }
    }

    pub(crate) fn qualified_name(self, table_prefix: &str) -> String {
        match self {
            Self::DatasetSources => self.name().into(),
            Self::TrainingCheckpoints => format!("{table_prefix}_{}", self.name()),
        }
    }
}

impl fmt::Display for Table {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
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

/// Persisted and query-result column names.
pub(crate) mod column {
    pub(crate) const ACTION_VALID: &str = "action_valid";
    pub(crate) const ACTION_POSITION: &str = "action_position";
    pub(crate) const CAMERA_ID: &str = "camera_id";
    pub(crate) const CHECKPOINT: &str = "checkpoint";
    pub(crate) const CONFIG_JSON: &str = "config_json";
    pub(crate) const CREATED_MS: &str = "created_ms";
    pub(crate) const DATASET_ID: &str = "dataset_id";
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
    /// Torque ceiling one motor reports for itself, captured with the take.
    /// The `LIMIT_` prefix separates a motor's own configured threshold from
    /// the value actually measured during the recording.
    pub(crate) const LIMIT_MAX_LOAD: &str = "limit_max_load";
    pub(crate) const LIMIT_MAX_TEMPERATURE_CELSIUS: &str = "limit_max_temperature_celsius";
    pub(crate) const LIMIT_MAX_VOLTAGE_VOLTS: &str = "limit_max_voltage_volts";
    pub(crate) const LIMIT_MIN_VOLTAGE_VOLTS: &str = "limit_min_voltage_volts";
    pub(crate) const MAX_FOLLOW_ERROR_DEGREES: &str = "max_follow_error_degrees";
    /// Lowest supply voltage measured on a motor across the whole take. See
    /// [`LIMIT_MIN_VOLTAGE_VOLTS`] for the motor's own configured floor.
    pub(crate) const MIN_VOLTAGE_VOLTS: &str = "min_voltage_volts";
    pub(crate) const MODALITY: &str = "modality";
    pub(crate) const NOTE: &str = "note";
    pub(crate) const ORDINAL: &str = "ordinal";
    pub(crate) const PEAK_CURRENT_AMPS: &str = "peak_current_amps";
    pub(crate) const PEAK_LOAD: &str = "peak_load";
    pub(crate) const PEAK_TEMPERATURE_CELSIUS: &str = "peak_temperature_celsius";
    pub(crate) const POLICY: &str = "policy";
    pub(crate) const RAW_POSITION: &str = "raw_position";
    /// Start of one omitted wall-clock interval, on the gap-free episode clock.
    /// Parallel to [`RECORDING_GAP_DURATION_NS`]; both lists are equal length.
    pub(crate) const RECORDING_GAP_AFTER_NS: &str = "recording_gap_after_ns";
    pub(crate) const RECORDING_GAP_DURATION_NS: &str = "recording_gap_duration_ns";
    pub(crate) const ROBOT_TYPE: &str = "robot_type";
    pub(crate) const RUN_ID: &str = "run_id";
    pub(crate) const RUN_NAME: &str = "run_name";
    pub(crate) const SAMPLE_HZ: &str = "sample_hz";
    pub(crate) const SAMPLE_INDEX: &str = "sample_index";
    pub(crate) const SCHEMA_VERSION: &str = "schema_version";
    pub(crate) const SOURCE: &str = "source";
    pub(crate) const STARTED_MS: &str = "started_ms";
    /// Motor temperature measured as the take opened. See
    /// [`PEAK_TEMPERATURE_CELSIUS`] for the extreme reached during it.
    pub(crate) const START_TEMPERATURE_CELSIUS: &str = "start_temperature_celsius";
    pub(crate) const START_VOLTAGE_VOLTS: &str = "start_voltage_volts";
    pub(crate) const STATE_VALID: &str = "state_valid";
    pub(crate) const STATE_POSITION: &str = "state_position";
    pub(crate) const STEP: &str = "step";
    pub(crate) const STREAM_ID: &str = "stream_id";
    pub(crate) const VALIDATION_LOSS: &str = "validation_loss";
    pub(crate) const VELOCITY: &str = "velocity";
    pub(crate) const WIDTH: &str = "width";
}

/// Public entry point for physical storage-schema naming and validation.
pub struct StorageSchema;

impl StorageSchema {
    /// Whether a value is safe to interpolate as an unquoted SQL identifier.
    pub fn is_identifier(value: &str) -> bool {
        spiral_robotics_common::sql::UnquotedSqlIdentifier::try_from(value).is_ok()
    }

    /// Whether a value is safe as a logical dataset row identifier.
    pub fn is_dataset_id(value: &str) -> bool {
        spiral_robotics_common::identity::DatasetId::is_canonical(value)
    }
}
