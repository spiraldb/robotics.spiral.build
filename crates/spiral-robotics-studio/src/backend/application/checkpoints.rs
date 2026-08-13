//! Application-owned boundary for immutable training artifacts.

use std::future::Future;
use std::pin::Pin;

/// Keep one model cell below Spiral's 256 MiB operation limit, with headroom
/// for the row's metadata and Arrow buffers.
pub(crate) const MAX_CHECKPOINT_ARTIFACT_BYTES: usize = 240 * 1024 * 1024;
/// Configuration is small JSON policy metadata, never a second weight blob.
pub(crate) const MAX_CHECKPOINT_CONFIG_BYTES: usize = 1024 * 1024;

/// Credential-free destination frozen when a training run starts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CheckpointSource {
    pub project: String,
    pub namespace: String,
    pub dataset_id: String,
}

/// Identity selected by the rollout use case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CheckpointKey {
    pub run_id: String,
    pub step: u64,
}

/// Trainer-produced artifact and metadata ready for durable publication.
pub(crate) struct CheckpointPublication {
    pub source: CheckpointSource,
    pub run_id: String,
    pub run_name: String,
    pub policy: String,
    pub step: u64,
    pub created_ms: i64,
    pub validation_loss: Option<f64>,
    pub config_json: String,
    pub payload: Vec<u8>,
}

/// Opaque model material loaded for native rollout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CheckpointArtifact {
    pub config_json: Vec<u8>,
    pub payload: Vec<u8>,
}

/// Validate opaque artifact sizes before adapters copy or materialize them a
/// second time. Overflow and oversized external rows fail closed.
pub(crate) fn validate_checkpoint_artifact_size(
    config_bytes: usize,
    payload_bytes: usize,
) -> Result<(), String> {
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

pub(crate) type CheckpointFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// Persistence boundary owned by training and rollout application policy.
///
/// Implementations may use a database, files, or a remote service. Callers see
/// only immutable source identity, publication metadata, and opaque bytes.
pub(crate) trait CheckpointRepository: Send + Sync {
    fn register_run_source(&self, run_id: &str, source: CheckpointSource) -> Result<(), String>;

    fn publish_checkpoint(&self, publication: CheckpointPublication) -> CheckpointFuture<'_, ()>;

    fn load_checkpoint(&self, key: CheckpointKey) -> CheckpointFuture<'_, CheckpointArtifact>;
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_CHECKPOINT_ARTIFACT_BYTES, MAX_CHECKPOINT_CONFIG_BYTES,
        validate_checkpoint_artifact_size,
    };

    #[test]
    fn checkpoint_size_limits_are_inclusive_and_fail_closed() {
        assert!(
            validate_checkpoint_artifact_size(
                MAX_CHECKPOINT_CONFIG_BYTES,
                MAX_CHECKPOINT_ARTIFACT_BYTES
            )
            .is_ok()
        );
        assert!(
            validate_checkpoint_artifact_size(
                MAX_CHECKPOINT_CONFIG_BYTES + 1,
                MAX_CHECKPOINT_ARTIFACT_BYTES
            )
            .is_err()
        );
        assert!(
            validate_checkpoint_artifact_size(
                MAX_CHECKPOINT_CONFIG_BYTES,
                MAX_CHECKPOINT_ARTIFACT_BYTES + 1
            )
            .is_err()
        );
        assert!(validate_checkpoint_artifact_size(usize::MAX, usize::MAX).is_err());
    }
}
