//! Arrow schemas for Spiral data-plane tables.

use std::fmt;

use arrow_schema::{DataType, Field, Schema};

include!(concat!(env!("OUT_DIR"), "/storage_schema_version.rs"));

/// Prefix of the relations Studio owns in Spiral, such as
/// `studio_training_checkpoints`.
pub const STUDIO_TABLE_PREFIX: &str = "studio";

/// Data-plane relations included in project bundles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Table {
    /// Every MCAP dataset's source files, in one entry naming each file's
    /// dataset; each dataset keeps them in its own source table.
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

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::DatasetSources => "dataset_source",
            Self::TrainingCheckpoints => "training_checkpoints",
        }
    }

    pub(crate) fn qualified_name(self) -> String {
        match self {
            Self::DatasetSources => self.name().into(),
            Self::TrainingCheckpoints => format!("{STUDIO_TABLE_PREFIX}_{}", self.name()),
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
            _ => self == expected,
        }
    }
}

/// Persisted and query-result column names.
pub(crate) mod column {
    pub(crate) const CHECKPOINT: &str = "checkpoint";
    pub(crate) const CONFIG_JSON: &str = "config_json";
    pub(crate) const CREATED_MS: &str = "created_ms";
    pub(crate) const POLICY: &str = "policy";
    pub(crate) const RUN_ID: &str = "run_id";
    pub(crate) const RUN_NAME: &str = "run_name";
    pub(crate) const STEP: &str = "step";
    pub(crate) const VALIDATION_LOSS: &str = "validation_loss";
}

/// Storage-schema naming validation.
pub(crate) struct StorageSchema;

impl StorageSchema {
    /// Whether a value is safe as a logical dataset row identifier.
    pub(crate) fn is_dataset_id(value: &str) -> bool {
        spiral_robotics_common::identity::DatasetId::is_canonical(value)
    }
}
