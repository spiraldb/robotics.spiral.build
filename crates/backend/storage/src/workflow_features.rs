//! Storage-owned schema for durable Workflow feature definitions.
//!
//! Dynamic feature caches keep schemas derived from their registered feature
//! columns. The registry itself is a stable managed relation and therefore
//! participates in provisioning, schema-drift checks, and project bundles.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};

use crate::db::{ManagedRelationSchema, Table};

/// Workflow feature relations contributed to project bundles.
pub(crate) struct WorkflowFeatureBundleSection;

impl WorkflowFeatureBundleSection {
    const RELATIONS: [Table; 1] = [Table::FeatureRegistry];
}

impl ManagedRelationSchema for WorkflowFeatureBundleSection {
    fn relations(&self) -> &'static [Table] {
        &Self::RELATIONS
    }

    fn managed_relation_schema(&self, table: Table) -> Option<Schema> {
        (table == Table::FeatureRegistry)
            .then(|| WorkflowFeatureSchema::registry().as_ref().clone())
    }
}

pub const MAX_FEATURE_REGISTRY_ROWS: usize = 4_096;
pub const MAX_FEATURE_CACHE_ROWS: usize = 1_000_000;
pub const MAX_FEATURE_CACHE_COLUMNS: usize = 256;
pub const SIGLIP_BASE_MODEL_ID: &str = "siglip-base";
pub const SIGLIP_BASE_REPOSITORY: &str = "google/siglip-base-patch16-224";
pub const SIGLIP_BASE_REVISION: &str = "7fd15f0689c79d79e38b1c2e2e2370a7bf2761ed";
pub const FEATURE_VALUE_SEMANTICS: &str = "sigmoid_probability_0_1";
pub const CACHE_KEY_FIELDS: [(&str, DataType); 5] = [
    ("episode_id", DataType::Utf8),
    ("stream_id", DataType::Utf8),
    ("segment_index", DataType::UInt32),
    ("frame_index", DataType::UInt64),
    ("episode_time_ns", DataType::UInt64),
];

/// Canonical registry and cache schema rules used by execution and project transfer.
pub struct WorkflowFeatureSchema;

impl WorkflowFeatureSchema {
    pub fn registry() -> SchemaRef {
        let text = |name| Field::new(name, DataType::Utf8, false);
        Arc::new(Schema::new(vec![
            text("source_relation"),
            text("cache_relation"),
            text("feature_name"),
            text("description"),
            text("concept"),
            text("media_field"),
            Field::new(
                "stream_ids",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
                false,
            ),
            text("producer_model_id"),
            text("producer_repository"),
            text("artifact_revision"),
            text("value_semantics"),
            Field::new("definition_version", DataType::UInt32, false),
            Field::new("created_at_ms", DataType::UInt64, false),
            Field::new("updated_at_ms", DataType::UInt64, false),
        ]))
    }

    pub fn validate_registry(schema: &Schema) -> Result<(), String> {
        let expected = Self::registry();
        if schema.fields().len() != expected.fields().len()
            || schema
                .fields()
                .iter()
                .zip(expected.fields())
                .any(|(actual, expected)| {
                    actual.name() != expected.name()
                        || actual.is_nullable() != expected.is_nullable()
                        || !Self::storage_type_matches(actual.data_type(), expected.data_type())
                })
        {
            return Err(format!(
                "Workflow feature registry has an incompatible schema. Studio will not replace it. Expected {expected:?}; actual {schema:?}."
            ));
        }
        Ok(())
    }

    pub fn cache_key_may_be_nullable(name: &str) -> bool {
        matches!(name, "frame_index" | "episode_time_ns")
    }

    /// Arrow text encodings can change after storage; nested field identity remains strict.
    pub fn storage_type_matches(actual: &DataType, expected: &DataType) -> bool {
        actual == expected
            || matches!(
                (actual, expected),
                (
                    DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View,
                    DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
                )
            )
            || match (actual, expected) {
                (DataType::List(actual), DataType::List(expected)) => {
                    actual.name() == expected.name()
                        && actual.is_nullable() == expected.is_nullable()
                        && actual.metadata() == expected.metadata()
                        && Self::storage_type_matches(actual.data_type(), expected.data_type())
                }
                _ => false,
            }
    }
}
