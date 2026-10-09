//! Policies Studio wrote to this connector: the checkpoints rollouts load.

pub(crate) mod http;

use std::path::PathBuf;

use spiral_robotics_common::rollout::StoredPolicy;
use spiral_robotics_inference::ModelArtifact;

use crate::OperationError;

/// `<directory>/<run>/<step>.policy`, each one envelope as Studio sent it.
pub(crate) struct PolicyStore {
    directory: PathBuf,
}

impl PolicyStore {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Keep `envelope` as the policy for `run`'s checkpoint `step`, replacing
    /// whatever was written for it before.
    pub(crate) async fn save(
        &self,
        run: &str,
        step: u64,
        envelope: &[u8],
    ) -> Result<StoredPolicy, OperationError> {
        ModelArtifact::from_envelope(envelope)
            .map_err(|error| OperationError::InvalidInput(error.to_string()))?;
        let path = self.path(run, step)?;
        let parent = path.parent().expect("a policy path has a run directory");
        let partial = path.with_extension("partial");
        let written = async {
            tokio::fs::create_dir_all(parent).await?;
            tokio::fs::write(&partial, envelope).await?;
            tokio::fs::rename(&partial, &path).await
        };
        written.await.map_err(Self::failed)?;
        Ok(StoredPolicy::new(run, step, envelope.len() as u64))
    }

    /// Every policy written here, by run and then step.
    pub(crate) async fn list(&self) -> Result<Vec<StoredPolicy>, OperationError> {
        let mut policies = Vec::new();
        for run in Self::entries(self.directory.clone()).await? {
            let Some(run_id) = run.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !run.is_dir() || Self::valid_run(run_id).is_err() {
                continue;
            }
            for file in Self::entries(run.clone()).await? {
                let step = file
                    .extension()
                    .filter(|extension| *extension == "policy")
                    .and(file.file_stem())
                    .and_then(|stem| stem.to_str()?.parse::<u64>().ok());
                let Some(step) = step else {
                    continue;
                };
                let bytes = tokio::fs::metadata(&file)
                    .await
                    .map_err(Self::failed)?
                    .len();
                policies.push(StoredPolicy::new(run_id, step, bytes));
            }
        }
        policies.sort_by(|left, right| {
            (left.run_id(), left.checkpoint_step()).cmp(&(right.run_id(), right.checkpoint_step()))
        });
        Ok(policies)
    }

    pub(crate) async fn load(&self, run: &str, step: u64) -> Result<ModelArtifact, String> {
        let path = self.path(run, step).map_err(|error| error.to_string())?;
        let envelope = tokio::fs::read(&path)
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => format!(
                    "checkpoint {step} of run {run:?} has not been written to this robot connector"
                ),
                _ => format!("reading the policy: {error}"),
            })?;
        ModelArtifact::from_envelope(&envelope).map_err(|error| error.to_string())
    }

    pub(crate) async fn delete(&self, run: &str, step: u64) -> Result<(), OperationError> {
        tokio::fs::remove_file(self.path(run, step)?)
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => OperationError::NotFound(format!(
                    "checkpoint {step} of run {run:?} is not on this robot connector"
                )),
                _ => Self::failed(error),
            })
    }

    fn path(&self, run: &str, step: u64) -> Result<PathBuf, OperationError> {
        Self::valid_run(run)?;
        Ok(self.directory.join(run).join(format!("{step}.policy")))
    }

    /// Training run identities; none can name a path outside the directory.
    fn valid_run(run: &str) -> Result<(), OperationError> {
        let valid = !run.is_empty()
            && run.len() <= 128
            && !run.starts_with('.')
            && run
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
        valid
            .then_some(())
            .ok_or_else(|| OperationError::invalid_input("Training run ID is invalid."))
    }

    async fn entries(directory: PathBuf) -> Result<Vec<PathBuf>, OperationError> {
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(Self::failed(error)),
        };
        let mut paths = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(Self::failed)? {
            paths.push(entry.path());
        }
        Ok(paths)
    }

    fn failed(error: std::io::Error) -> OperationError {
        OperationError::Internal(format!("policy storage: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn written_policies_are_listed_loaded_and_deleted() {
        let directory = tempfile::tempdir().unwrap();
        let store = PolicyStore::new(directory.path().to_owned());
        let envelope = ModelArtifact::from_bytes(b"{}".to_vec(), b"weights".to_vec())
            .unwrap()
            .envelope();
        store.save("run-1", 200, &envelope).await.unwrap();
        store.save("run-1", 100, &envelope).await.unwrap();
        assert_eq!(
            store
                .list()
                .await
                .unwrap()
                .iter()
                .map(StoredPolicy::checkpoint_step)
                .collect::<Vec<_>>(),
            [100, 200]
        );
        assert_eq!(
            store.load("run-1", 100).await.unwrap().weights(),
            b"weights"
        );
        store.delete("run-1", 100).await.unwrap();
        assert!(store.load("run-1", 100).await.is_err());
        assert!(store.save("run-1", 1, b"short").await.is_err());
        assert!(store.save("../escape", 1, &envelope).await.is_err());
    }
}
