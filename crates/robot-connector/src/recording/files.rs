//! Finished takes, each one immutable MCAP file, kept until the browser has
//! carried it to Studio.
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use spiral_robotics_common::identity::DatasetId;
use spiral_robotics_common::recording::SavedRecording;
use spiral_robotics_recording::EpisodeId;

use crate::OperationError;

/// `<directory>/<dataset>/<episode>.mcap`
pub(crate) struct RecordingFiles {
    directory: PathBuf,
}

impl RecordingFiles {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Save `bytes` as `episode`'s recording in `dataset`. Saving the same
    /// bytes again is a no-op; a recording is never replaced.
    pub(crate) fn save(&self, dataset: &str, episode: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self
            .path(dataset, episode)
            .map_err(|error| error.to_string())?;
        let parent = path
            .parent()
            .ok_or("Recording file has no parent directory")?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Creating recording directory: {error}"))?;
        if path.exists() {
            return if fs::read(&path).map_err(|error| error.to_string())? == bytes {
                Ok(())
            } else {
                Err(format!(
                    "Refusing to overwrite immutable recording {}",
                    path.display()
                ))
            };
        }
        let temporary = parent.join(format!(".{episode}.partial"));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            // Linking publishes the complete file without replacing an existing recording.
            fs::hard_link(&temporary, &path)?;
            fs::File::open(parent)?.sync_all()
        })();
        let _ = fs::remove_file(temporary);
        result.map_err(|error: std::io::Error| {
            format!("Saving recording {}: {error}", path.display())
        })
    }

    /// Every saved recording, by dataset and then episode.
    pub(crate) async fn list(&self) -> Result<Vec<SavedRecording>, OperationError> {
        let mut recordings = Vec::new();
        for dataset in Self::entries(&self.directory)? {
            if !dataset.is_dir() {
                continue;
            }
            for file in Self::entries(&dataset)? {
                let (Some(dataset), Some(episode)) = (
                    dataset.file_name().and_then(|name| name.to_str()),
                    file.file_stem().and_then(|name| name.to_str()),
                ) else {
                    continue;
                };
                if file.extension().is_none_or(|extension| extension != "mcap")
                    || self.path(dataset, episode).is_err()
                {
                    continue;
                }
                let bytes = fs::metadata(&file).map_err(Self::failed)?.len();
                recordings.push(SavedRecording::new(dataset, episode, bytes));
            }
        }
        Ok(recordings)
    }

    pub(crate) async fn read(
        &self,
        dataset: &str,
        episode: &str,
    ) -> Result<Vec<u8>, OperationError> {
        let path = self.path(dataset, episode)?;
        tokio::fs::read(&path)
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => {
                    OperationError::NotFound(format!("no saved recording {dataset}/{episode}"))
                }
                _ => Self::failed(error),
            })
    }

    /// Forget a recording Studio has stored.
    pub(crate) async fn delete(&self, dataset: &str, episode: &str) -> Result<(), OperationError> {
        let path = self.path(dataset, episode)?;
        tokio::fs::remove_file(&path)
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => {
                    OperationError::NotFound(format!("no saved recording {dataset}/{episode}"))
                }
                _ => Self::failed(error),
            })
    }

    /// Validated identities can never name a path outside the directory.
    fn path(&self, dataset: &str, episode: &str) -> Result<PathBuf, OperationError> {
        if !DatasetId::is_canonical(dataset) || dataset.contains('.') {
            return Err(OperationError::invalid_input(
                "Invalid recording dataset name",
            ));
        }
        EpisodeId::try_from(episode).map_err(OperationError::invalid_input)?;
        Ok(self.directory.join(dataset).join(format!("{episode}.mcap")))
    }

    fn entries(directory: &Path) -> Result<Vec<PathBuf>, OperationError> {
        match fs::read_dir(directory) {
            Ok(entries) => entries
                .map(|entry| entry.map(|entry| entry.path()).map_err(Self::failed))
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(Self::failed(error)),
        }
    }

    fn failed(error: std::io::Error) -> OperationError {
        OperationError::Internal(format!("reading saved recordings: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_saved_recording_is_listed_read_and_deleted_but_never_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let files = RecordingFiles::new(directory.path().to_owned());
        files.save("pick_place", "take-1", b"original").unwrap();
        files.save("pick_place", "take-1", b"original").unwrap();
        assert!(files.save("pick_place", "take-1", b"changed").is_err());
        assert_eq!(
            files.list().await.unwrap(),
            vec![SavedRecording::new("pick_place", "take-1", 8)]
        );
        assert_eq!(
            files.read("pick_place", "take-1").await.unwrap(),
            b"original"
        );
        files.delete("pick_place", "take-1").await.unwrap();
        assert!(files.list().await.unwrap().is_empty());
        assert!(matches!(
            files.read("pick_place", "take-1").await,
            Err(OperationError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn identities_cannot_leave_the_directory() {
        let files = RecordingFiles::new(tempfile::tempdir().unwrap().path().to_owned());
        for (dataset, episode) in [("..", "take"), ("data", "../take"), ("a.b", "take")] {
            assert!(files.read(dataset, episode).await.is_err());
        }
    }
}
