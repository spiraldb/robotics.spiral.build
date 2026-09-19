//! Immutable capture files survive failed or interrupted publication to Spiral.
use sha2::{Digest as _, Sha256};
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

pub(crate) struct RecordingFile {
    path: PathBuf,
}

impl RecordingFile {
    pub(super) fn new(
        store: &crate::ProjectStore,
        episode: &super::FullEpisode,
    ) -> Result<Self, String> {
        let directory = dirs::data_local_dir().ok_or("Cannot locate the recording directory")?;
        let project = format!(
            "{:x}",
            Sha256::digest(format!("{}\n{}", store.endpoint(), store.project_id()).as_bytes())
        );
        let summary = episode.summary();
        super::EpisodeId::try_from(summary.id()).map_err(|error| error.to_string())?;
        if !crate::db::StorageSchema::is_dataset_id(summary.dataset_id())
            || summary.dataset_id().contains('.')
        {
            return Err("Invalid recording dataset name".into());
        }
        Ok(Self {
            path: directory
                .join("spiral-robotics")
                .join("recordings")
                .join(project)
                .join(summary.dataset_id())
                .join(format!("{}.mcap", summary.id())),
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn save(&self, bytes: &[u8]) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or("Recording file has no parent directory")?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Creating recording directory: {error}"))?;
        if self.path.exists() {
            return if fs::read(&self.path).map_err(|error| error.to_string())? == bytes {
                Ok(())
            } else {
                Err(format!(
                    "Refusing to overwrite immutable recording {}",
                    self.path.display()
                ))
            };
        }
        let temporary = parent.join(format!(".{}.partial", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            // Linking publishes the complete file without replacing an existing recording.
            fs::hard_link(&temporary, &self.path)?;
            fs::File::open(parent)?.sync_all()
        })();
        let _ = fs::remove_file(temporary);
        result.map_err(|error: std::io::Error| {
            format!("Saving recording {}: {error}", self.path.display())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_never_changes_an_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let recording = RecordingFile {
            path: directory.path().join("take.mcap"),
        };
        recording.save(b"original recording").unwrap();
        recording.save(b"original recording").unwrap();
        assert!(recording.save(b"recalibrated recording").is_err());
        assert_eq!(fs::read(recording.path()).unwrap(), b"original recording");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
