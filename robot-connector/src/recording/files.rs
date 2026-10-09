//! Each episode's MCAP file through its life: written while the episode records,
//! saved once it stops, uploaded to Studio, then kept for review until the
//! next episode opens.
use std::fs;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
use tokio::sync::Notify;

use spiral_robotics_common::identity::DatasetId;
use spiral_robotics_common::recording::SavedRecording;
use spiral_robotics_recording::EpisodeId;

use crate::OperationError;

/// `<directory>/<dataset>/<episode>.mcap`, named for where the episode is in its
/// life.
pub(crate) struct RecordingFiles {
    directory: PathBuf,
    /// Wakes the upload of episodes when one is saved.
    saved: Notify,
}

/// Where an episode's file is in its life, as its name says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EpisodeFileState {
    /// Being written while the episode records, or paused.
    Recording,
    /// Saved, and waiting for Studio to hold it whole.
    Saved,
    /// Studio holds it; kept for review until the next episode opens.
    Uploaded,
}

impl EpisodeFileState {
    const ALL: [Self; 3] = [Self::Recording, Self::Saved, Self::Uploaded];

    fn file_name(self, episode: &str) -> String {
        match self {
            Self::Recording => format!(".{episode}.partial"),
            Self::Saved => format!("{episode}.mcap"),
            Self::Uploaded => format!("{episode}.uploaded"),
        }
    }

    /// The episode a file name holds, and where it is in its life.
    fn of(file_name: &str) -> Option<(&str, Self)> {
        if let Some(episode) = file_name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".partial"))
        {
            return Some((episode, Self::Recording));
        }
        file_name
            .strip_suffix(".mcap")
            .map(|episode| (episode, Self::Saved))
            .or_else(|| {
                file_name
                    .strip_suffix(".uploaded")
                    .map(|episode| (episode, Self::Uploaded))
            })
    }
}

impl RecordingFiles {
    /// The episodes kept under `directory`. An episode the last run was writing can
    /// never be finished, and the last run's review is over, so their files
    /// go.
    pub(crate) fn new(directory: PathBuf) -> Self {
        let files = Self {
            directory,
            saved: Notify::new(),
        };
        files.remove(|state| state != EpisodeFileState::Saved);
        files
    }

    /// Wait until an episode is saved; one saved since the last wait counts.
    pub(crate) async fn saved(&self) {
        self.saved.notified().await;
    }

    /// Where `episode` of `dataset` is written while it records.
    pub(crate) fn partial_path(
        &self,
        dataset: &str,
        episode: &str,
    ) -> Result<PathBuf, OperationError> {
        let path = self.path(dataset, episode, EpisodeFileState::Recording)?;
        let parent = path
            .parent()
            .expect("an episode's file is in its dataset's folder");
        fs::create_dir_all(parent).map_err(Self::failed)?;
        Ok(path)
    }

    /// Save the finished file of `episode` of `dataset` for the upload. A
    /// saved episode is never replaced.
    pub(crate) fn publish(&self, dataset: &str, episode: &str) -> Result<(), String> {
        let state = |state| {
            self.path(dataset, episode, state)
                .map_err(|error| error.message().to_owned())
        };
        let (partial, saved) = (
            state(EpisodeFileState::Recording)?,
            state(EpisodeFileState::Saved)?,
        );
        // Linking publishes the complete file without replacing a saved one.
        fs::hard_link(&partial, &saved)
            .and_then(|()| fs::remove_file(&partial))
            .and_then(|()| fs::File::open(saved.parent().expect("a dataset folder"))?.sync_all())
            .map_err(|error| format!("Saving recording {}: {error}", saved.display()))?;
        self.saved.notify_one();
        Ok(())
    }

    /// Every saved episode waiting for the upload, by dataset and then episode.
    pub(crate) async fn list(&self) -> Result<Vec<SavedRecording>, OperationError> {
        let mut recordings = Vec::new();
        for (dataset, episode, state, file) in self.episode_files()? {
            if state == EpisodeFileState::Saved {
                let bytes = fs::metadata(&file).map_err(Self::failed)?.len();
                recordings.push(SavedRecording::new(dataset, episode, bytes));
            }
        }
        Ok(recordings)
    }

    /// `length` bytes of a saved episode, from `offset` on.
    pub(crate) async fn read_part(
        &self,
        dataset: &str,
        episode: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, OperationError> {
        let path = self.path(dataset, episode, EpisodeFileState::Saved)?;
        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => {
                    OperationError::NotFound(format!("no saved recording {dataset}/{episode}"))
                }
                _ => Self::failed(error),
            })?;
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(Self::failed)?;
        let mut part = vec![0; length];
        file.read_exact(&mut part).await.map_err(Self::failed)?;
        Ok(part)
    }

    /// Studio holds `episode` of `dataset` whole: keep it for review only.
    pub(crate) async fn mark_uploaded(
        &self,
        dataset: &str,
        episode: &str,
    ) -> Result<(), OperationError> {
        tokio::fs::rename(
            self.path(dataset, episode, EpisodeFileState::Saved)?,
            self.path(dataset, episode, EpisodeFileState::Uploaded)?,
        )
        .await
        .map_err(Self::failed)
    }

    /// Forget the uploaded episodes kept for review.
    pub(crate) fn forget_uploaded(&self) {
        self.remove(|state| state == EpisodeFileState::Uploaded);
    }

    /// The file a review of `episode` of `dataset` reads, wherever the episode is
    /// in its life.
    pub(crate) fn review_path(&self, dataset: &str, episode: &str) -> Option<PathBuf> {
        EpisodeFileState::ALL
            .into_iter()
            .filter_map(|state| self.path(dataset, episode, state).ok())
            .find(|path| path.is_file())
    }

    /// Delete every episode's file whose state `doomed` names; best effort.
    fn remove(&self, doomed: impl Fn(EpisodeFileState) -> bool) {
        let Ok(files) = self.episode_files() else {
            return;
        };
        for (_, _, state, file) in files {
            if doomed(state) {
                let _ = fs::remove_file(file);
            }
        }
    }

    /// Every episode's file: its dataset, episode, state, and path.
    fn episode_files(
        &self,
    ) -> Result<Vec<(String, String, EpisodeFileState, PathBuf)>, OperationError> {
        let mut files = Vec::new();
        for dataset in Self::entries(&self.directory)? {
            if !dataset.is_dir() {
                continue;
            }
            for file in Self::entries(&dataset)? {
                let (Some(dataset), Some(name)) = (
                    dataset.file_name().and_then(|name| name.to_str()),
                    file.file_name().and_then(|name| name.to_str()),
                ) else {
                    continue;
                };
                let Some((episode, state)) = EpisodeFileState::of(name) else {
                    continue;
                };
                if self.path(dataset, episode, state).is_ok() {
                    files.push((dataset.to_owned(), episode.to_owned(), state, file.clone()));
                }
            }
        }
        Ok(files)
    }

    /// Validated identities can never name a path outside the directory.
    fn path(
        &self,
        dataset: &str,
        episode: &str,
        state: EpisodeFileState,
    ) -> Result<PathBuf, OperationError> {
        if !DatasetId::is_canonical(dataset) || dataset.contains('.') {
            return Err(OperationError::invalid_input(
                "Invalid recording dataset name",
            ));
        }
        EpisodeId::try_from(episode).map_err(OperationError::invalid_input)?;
        Ok(self.directory.join(dataset).join(state.file_name(episode)))
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

    /// Write `bytes` as `episode`'s finished file and save it.
    fn save(files: &RecordingFiles, episode: &str, bytes: &[u8]) -> Result<(), String> {
        fs::write(files.partial_path("pick_place", episode).unwrap(), bytes).unwrap();
        files.publish("pick_place", episode)
    }

    #[tokio::test]
    async fn an_episode_is_saved_uploaded_kept_for_review_and_then_forgotten() {
        let directory = tempfile::tempdir().unwrap();
        let files = RecordingFiles::new(directory.path().to_owned());
        let partial = files.partial_path("pick_place", "episode-1").unwrap();
        fs::write(&partial, b"original").unwrap();
        // A paused episode is reviewed from the file being written.
        assert_eq!(files.review_path("pick_place", "episode-1"), Some(partial));
        assert!(files.list().await.unwrap().is_empty());
        files.publish("pick_place", "episode-1").unwrap();
        // Saving wakes the upload.
        files.saved().await;
        // A saved episode is never replaced.
        assert!(save(&files, "episode-1", b"changed").is_err());
        fs::remove_file(files.partial_path("pick_place", "episode-1").unwrap()).unwrap();
        assert_eq!(
            files.list().await.unwrap(),
            vec![SavedRecording::new("pick_place", "episode-1", 8)]
        );
        assert_eq!(
            files
                .read_part("pick_place", "episode-1", 2, 4)
                .await
                .unwrap(),
            b"igin"
        );

        files
            .mark_uploaded("pick_place", "episode-1")
            .await
            .unwrap();
        assert!(files.list().await.unwrap().is_empty());
        let review = files.review_path("pick_place", "episode-1").unwrap();
        assert_eq!(fs::read(&review).unwrap(), b"original");
        files.forget_uploaded();
        assert_eq!(files.review_path("pick_place", "episode-1"), None);
    }

    #[tokio::test]
    async fn a_new_run_keeps_only_the_episodes_waiting_for_the_upload() {
        let directory = tempfile::tempdir().unwrap();
        let files = RecordingFiles::new(directory.path().to_owned());
        save(&files, "saved", b"saved").unwrap();
        save(&files, "uploaded", b"uploaded").unwrap();
        files.mark_uploaded("pick_place", "uploaded").await.unwrap();
        fs::write(
            files.partial_path("pick_place", "cut-off").unwrap(),
            b"half",
        )
        .unwrap();

        let files = RecordingFiles::new(directory.path().to_owned());
        assert_eq!(
            files.list().await.unwrap(),
            vec![SavedRecording::new("pick_place", "saved", 5)]
        );
        assert_eq!(files.review_path("pick_place", "uploaded"), None);
        assert_eq!(files.review_path("pick_place", "cut-off"), None);
    }

    #[tokio::test]
    async fn identities_cannot_leave_the_directory() {
        let files = RecordingFiles::new(tempfile::tempdir().unwrap().path().to_owned());
        for (dataset, episode) in [
            ("..", "episode"),
            ("data", "../episode"),
            ("a.b", "episode"),
        ] {
            assert!(files.read_part(dataset, episode, 0, 1).await.is_err());
            assert!(files.partial_path(dataset, episode).is_err());
        }
    }
}
