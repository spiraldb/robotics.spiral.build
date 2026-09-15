//! Application-owned boundary for immutable training artifacts.

use std::future::Future;
use std::pin::Pin;

use arrow_array::{
    BinaryArray, BinaryViewArray, LargeBinaryArray, LargeStringArray, RecordBatch, StringArray,
    StringViewArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::StreamExt as _;

use crate::db::DatabaseSession as _;
use crate::db::ProjectStore;
use crate::db::SpiralSql;
use crate::db::{Table, column as db};
use crate::manager::StorageManager;

/// Keep one model cell below Spiral's 256 MiB operation limit, with headroom
/// for the row's metadata and Arrow buffers.
const MAX_CHECKPOINT_ARTIFACT_BYTES: usize = 240 * 1024 * 1024;
/// Configuration is small JSON policy metadata, never a second weight blob.
const MAX_CHECKPOINT_CONFIG_BYTES: usize = 1024 * 1024;

/// Identity selected by the rollout use case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointKey {
    run_id: String,
    step: u64,
}

impl CheckpointKey {
    pub fn new(run_id: String, step: u64) -> Self {
        Self { run_id, step }
    }
    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }
    pub(crate) const fn step(&self) -> u64 {
        self.step
    }
}

/// Opaque model material loaded for native rollout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointArtifact {
    config_json: Vec<u8>,
    payload: Vec<u8>,
}

impl CheckpointArtifact {
    pub(crate) fn schema() -> Schema {
        Schema::new(vec![
            Field::new(db::RUN_ID, DataType::Utf8, false),
            Field::new(db::RUN_NAME, DataType::Utf8, false),
            Field::new(db::POLICY, DataType::Utf8, false),
            Field::new(db::STEP, DataType::UInt64, false),
            Field::new(db::CREATED_MS, DataType::Int64, false),
            Field::new(db::VALIDATION_LOSS, DataType::Float64, true),
            Field::new(db::CONFIG_JSON, DataType::Utf8, false),
            Field::new(db::CHECKPOINT, DataType::LargeBinary, false),
        ])
    }

    pub(crate) fn from_bytes(config_json: Vec<u8>, payload: Vec<u8>) -> Result<Self, String> {
        Self::validate_size(config_json.len(), payload.len())?;
        Ok(Self {
            config_json,
            payload,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn new(config_json: Vec<u8>, payload: Vec<u8>) -> Result<Self, String> {
        Self::from_bytes(config_json, payload)
    }

    pub fn into_parts(self) -> (Vec<u8>, Vec<u8>) {
        (self.config_json, self.payload)
    }
    pub fn config_json(&self) -> &[u8] {
        &self.config_json
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Validate opaque artifact sizes before adapters copy or materialize them
    /// a second time. Overflow and oversized external rows fail closed.
    pub(crate) fn validate_size(config_bytes: usize, payload_bytes: usize) -> Result<(), String> {
        if config_bytes > MAX_CHECKPOINT_CONFIG_BYTES {
            return Err(format!(
                "checkpoint configuration exceeds the {MAX_CHECKPOINT_CONFIG_BYTES}-byte limit"
            ));
        }
        if payload_bytes > MAX_CHECKPOINT_ARTIFACT_BYTES {
            return Err(format!(
                "checkpoint exceeds the {MAX_CHECKPOINT_ARTIFACT_BYTES}-byte single-artifact limit"
            ));
        }
        config_bytes
            .checked_add(payload_bytes)
            .ok_or_else(|| "checkpoint artifact size overflows usize".to_owned())?;
        Ok(())
    }
}

pub type CheckpointFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// Checkpoint operations used by rollout policy. The concrete
/// Studio implementation always addresses its one process-owned Spiral project.
pub trait CheckpointStore: Send + Sync {
    fn load_checkpoint(&self, key: CheckpointKey) -> CheckpointFuture<'_, CheckpointArtifact>;

    fn delete_run_checkpoints(&self, run_id: String) -> CheckpointFuture<'_, ()>;
}

/// Immutable checkpoint operations for one project.
struct TrainingCheckpointPersistence<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> TrainingCheckpointPersistence<'a> {
    const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    async fn load(&self, key: CheckpointKey) -> Result<CheckpointArtifact, String> {
        let query = SpiralSql::training_checkpoint(
            self.project_store.table_prefix(),
            key.run_id(),
            key.step(),
        );
        let mut records = self.record_batches(query).await?;
        let mut artifact = None;
        while let Some(batch) = records.next().await {
            let batch = batch.map_err(|error| format!("reading Spiral checkpoint: {error}"))?;
            for row in 0..batch.num_rows() {
                if artifact.is_some() {
                    return Err("Spiral returned duplicate rows for that checkpoint".to_owned());
                }
                artifact = Some(CheckpointArtifact::from_batch_row(&batch, row)?);
            }
        }
        artifact.ok_or_else(|| "No checkpoint matched that training run and step".to_owned())
    }

    async fn delete(&self, run_id: String) -> Result<(), String> {
        let target = Table::TrainingCheckpoints.qualified_name(self.project_store.table_prefix());
        let mut connection = self.project_store.connect().await?;
        connection
            .execute_update(SpiralSql::delete_training_run(&target, &run_id).into())
            .await
            .map(|_| ())
            .map_err(|error| {
                format!("deleting training run {run_id:?} checkpoints from {target}: {error}")
            })
    }

    async fn record_batches(&self, sql: String) -> Result<crate::RecordBatchStream, String> {
        let mut connection = self.project_store.connect().await?;
        connection
            .query(sql.into())
            .await
            .map_err(|error| error.to_string())
    }
}

impl CheckpointStore for StorageManager {
    fn load_checkpoint(&self, key: CheckpointKey) -> CheckpointFuture<'_, CheckpointArtifact> {
        Box::pin(async move {
            TrainingCheckpointPersistence::new(self.project_store())
                .load(key)
                .await
        })
    }

    fn delete_run_checkpoints(&self, run_id: String) -> CheckpointFuture<'_, ()> {
        Box::pin(async move {
            TrainingCheckpointPersistence::new(self.project_store())
                .delete(run_id)
                .await
        })
    }
}

impl CheckpointArtifact {
    fn from_batch_row(batch: &RecordBatch, row: usize) -> Result<Self, String> {
        let config = batch.column_by_name(db::CONFIG_JSON).ok_or_else(|| {
            format!(
                "Spiral checkpoint query returned no UTF-8 {} column",
                db::CONFIG_JSON
            )
        })?;
        let checkpoint = batch.column_by_name(db::CHECKPOINT).ok_or_else(|| {
            format!(
                "Spiral checkpoint query returned no {} column",
                db::CHECKPOINT
            )
        })?;
        let payload = if let Some(values) = checkpoint.as_any().downcast_ref::<BinaryArray>() {
            values.value(row).to_vec()
        } else if let Some(values) = checkpoint.as_any().downcast_ref::<LargeBinaryArray>() {
            values.value(row).to_vec()
        } else if let Some(values) = checkpoint.as_any().downcast_ref::<BinaryViewArray>() {
            values.value(row).to_vec()
        } else {
            return Err(format!(
                "Spiral checkpoint payload has unexpected type {}",
                checkpoint.data_type()
            ));
        };
        Self::validate_size(0, payload.len())?;
        let config_json = if let Some(values) = config.as_any().downcast_ref::<StringArray>() {
            values.value(row).as_bytes().to_vec()
        } else if let Some(values) = config.as_any().downcast_ref::<LargeStringArray>() {
            values.value(row).as_bytes().to_vec()
        } else if let Some(values) = config.as_any().downcast_ref::<StringViewArray>() {
            values.value(row).as_bytes().to_vec()
        } else {
            return Err(format!(
                "Spiral checkpoint config has unexpected type {}",
                config.data_type()
            ));
        };
        Self::validate_size(config_json.len(), payload.len())?;
        Self::from_bytes(config_json, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::{CheckpointArtifact, MAX_CHECKPOINT_ARTIFACT_BYTES, MAX_CHECKPOINT_CONFIG_BYTES};

    #[test]
    fn checkpoint_size_limits_are_inclusive_and_fail_closed() {
        assert!(
            CheckpointArtifact::validate_size(
                MAX_CHECKPOINT_CONFIG_BYTES,
                MAX_CHECKPOINT_ARTIFACT_BYTES
            )
            .is_ok()
        );
        assert!(
            CheckpointArtifact::validate_size(
                MAX_CHECKPOINT_CONFIG_BYTES + 1,
                MAX_CHECKPOINT_ARTIFACT_BYTES
            )
            .is_err()
        );
        assert!(
            CheckpointArtifact::validate_size(
                MAX_CHECKPOINT_CONFIG_BYTES,
                MAX_CHECKPOINT_ARTIFACT_BYTES + 1
            )
            .is_err()
        );
        assert!(CheckpointArtifact::validate_size(usize::MAX, usize::MAX).is_err());
    }
}
