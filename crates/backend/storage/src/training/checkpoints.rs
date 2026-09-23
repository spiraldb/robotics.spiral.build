//! Training checkpoints the worker commits to Spiral: the only place they are
//! listed, loaded, or deleted.

use std::future::Future;
use std::pin::Pin;

use arrow_array::{BinaryArray, BinaryViewArray, LargeBinaryArray, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use futures::StreamExt as _;
use spiral_robotics_common::training::{CheckpointPolicy, TrainingCheckpoint};

use crate::db::DatabaseSession as _;
use crate::db::ProjectStore;
use crate::db::SpiralSql;
use crate::db::{ArrowColumns, Table, column as db};
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
    fn run_id(&self) -> &str {
        &self.run_id
    }
    const fn step(&self) -> u64 {
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

    fn from_bytes(config_json: Vec<u8>, payload: Vec<u8>) -> Result<Self, String> {
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
    fn validate_size(config_bytes: usize, payload_bytes: usize) -> Result<(), String> {
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

/// Checkpoint loading used by rollout policy. The concrete Studio
/// implementation always addresses its one process-owned Spiral project.
pub trait CheckpointStore: Send + Sync {
    fn load_checkpoint(&self, key: CheckpointKey) -> CheckpointFuture<'_, CheckpointArtifact>;
}

/// One project's checkpoints in Spiral.
pub struct TrainingCheckpointPersistence<'a> {
    project_store: &'a ProjectStore,
}

/// The policy facts a checkpoint's stored model configuration carries.
#[derive(serde::Deserialize)]
struct StoredCheckpointPolicy {
    spiral_camera_order: Vec<String>,
    n_action_steps: usize,
    spiral_sample_hz: Option<f64>,
}

impl<'a> TrainingCheckpointPersistence<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    /// Every checkpoint of every run, without its weights.
    pub async fn list(&self) -> Result<Vec<TrainingCheckpoint>, String> {
        let query = SpiralSql::training_checkpoint_catalog(self.project_store.table_prefix());
        let mut records = self.record_batches(query).await?;
        let mut checkpoints = Vec::new();
        while let Some(batch) = records.next().await {
            let batch = batch.map_err(|error| format!("reading Spiral checkpoints: {error}"))?;
            let columns = ArrowColumns::new(&batch, "training checkpoint");
            for row in 0..batch.num_rows() {
                checkpoints.push(Self::checkpoint_at(&columns, row)?);
            }
        }
        Ok(checkpoints)
    }

    /// Delete every checkpoint of one run.
    pub async fn delete_run(&self, run_id: &str) -> Result<(), String> {
        let target = Table::TrainingCheckpoints.qualified_name(self.project_store.table_prefix());
        let mut connection = self.project_store.connect().await?;
        connection
            .execute_update(SpiralSql::delete_training_run(&target, run_id).into())
            .await
            .map(|_| ())
            .map_err(|error| {
                format!("deleting training run {run_id:?} checkpoints from {target}: {error}")
            })
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

    fn checkpoint_at(columns: &ArrowColumns<'_>, row: usize) -> Result<TrainingCheckpoint, String> {
        let run_id = columns.text(db::RUN_ID, row)?;
        let step = columns.unsigned(db::STEP, row)?;
        let policy: StoredCheckpointPolicy =
            serde_json::from_str(columns.text(db::CONFIG_JSON, row)?).map_err(|error| {
                format!("checkpoint {run_id:?} step {step} has an invalid model config: {error}")
            })?;
        Ok(
            TrainingCheckpoint::new(run_id, step, columns.signed(db::CREATED_MS, row)?)
                .with_validation_loss(columns.optional_float(db::VALIDATION_LOSS, row)?)
                .with_policy(CheckpointPolicy::new(
                    policy.spiral_camera_order,
                    policy.n_action_steps,
                    policy.spiral_sample_hz,
                )),
        )
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
        Box::pin(async move { self.training_checkpoints().load(key).await })
    }
}

impl CheckpointArtifact {
    fn from_batch_row(batch: &RecordBatch, row: usize) -> Result<Self, String> {
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
        let config_json = ArrowColumns::new(batch, "training checkpoint")
            .text(db::CONFIG_JSON, row)?
            .as_bytes()
            .to_vec();
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
