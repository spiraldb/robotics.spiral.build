//! Storage-owned schema for durable Workflow feature definitions.
//!
//! Dynamic feature caches keep schemas derived from their registered feature
//! columns. The registry itself is a stable managed relation and therefore
//! participates in provisioning, schema-drift checks, and project bundles.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema};

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
        (table == Table::FeatureRegistry).then(|| {
            let text = |name| Field::new(name, DataType::Utf8, false);
            Schema::new(vec![
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
            ])
        })
    }
}
