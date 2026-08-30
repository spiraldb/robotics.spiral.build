//! Training-owned history and checkpoint persistence.

mod checkpoints;
mod training_runs;

pub use checkpoints::{CheckpointArtifact, CheckpointFuture, CheckpointKey, CheckpointStore};
pub(crate) use training_runs::TrainingHistoryPersistence;

use arrow_schema::{DataType, Field, Schema};

use crate::db::{ManagedRelationSchema, Table, column};
use training_runs::TrainingRunSchema;

/// Training history and artifact relations contributed to project bundles.
pub(crate) struct TrainingBundleSection;

impl TrainingBundleSection {
    const RELATIONS: [Table; 4] = [
        Table::TrainingCheckpoints,
        Table::TrainingRuns,
        Table::TrainingRunMetrics,
        Table::TrainingRunCheckpoints,
    ];
}

impl ManagedRelationSchema for TrainingBundleSection {
    fn relations(&self) -> &'static [Table] {
        &Self::RELATIONS
    }

    fn managed_relation_schema(&self, table: Table) -> Option<Schema> {
        let text = |name, nullable| Field::new(name, DataType::Utf8, nullable);
        let timestamp = |name| Field::new(name, DataType::Int64, false);
        match table {
            Table::TrainingRuns => Some(TrainingRunSchema::runs().as_ref().clone()),
            Table::TrainingRunMetrics => Some(TrainingRunSchema::metrics().as_ref().clone()),
            Table::TrainingRunCheckpoints => {
                Some(TrainingRunSchema::checkpoints().as_ref().clone())
            }
            Table::TrainingCheckpoints => Some(Schema::new(vec![
                text(column::RUN_ID, false),
                text(column::RUN_NAME, false),
                text(column::POLICY, false),
                Field::new(column::STEP, DataType::UInt64, false),
                timestamp(column::CREATED_MS),
                Field::new(column::VALIDATION_LOSS, DataType::Float64, true),
                text(column::CONFIG_JSON, false),
                Field::new(column::CHECKPOINT, DataType::LargeBinary, false),
            ])),
            _ => None,
        }
    }
}
