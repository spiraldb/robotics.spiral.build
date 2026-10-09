//! The one episode a robot connector may have open.
//!
//! Whichever operation changes the episode — start, pause, stop, or closing a
//! episode the control loop ended — holds it for the whole operation. Operations
//! never interleave, and one that is cancelled leaves the episode as it was.
use spiral_robotics_recording::EpisodeFile;
use tokio::sync::{Mutex, MutexGuard};

use crate::OperationError;
use crate::robotics::video::VideoCapture;

#[derive(Default)]
pub(crate) struct RecordingSession {
    open: Mutex<Option<RecordingEpisode>>,
}

/// An episode the control loop is recording, until its file is saved.
pub(crate) struct RecordingEpisode {
    episode: String,
    capture: Option<VideoCapture>,
    file: EpisodeFile,
}

/// One operation's exclusive hold on the session's episode.
pub(crate) struct EpisodeChange<'a> {
    open: MutexGuard<'a, Option<RecordingEpisode>>,
}

impl RecordingEpisode {
    pub(crate) fn new(episode: String, capture: Option<VideoCapture>, file: EpisodeFile) -> Self {
        debug_assert!(
            capture
                .as_ref()
                .is_none_or(|capture| capture.episode_id() == episode)
        );
        Self {
            episode,
            capture,
            file,
        }
    }
}

impl RecordingSession {
    /// Hold the episode for a command, refusing while another operation holds it.
    pub(crate) fn try_change(&self) -> Result<EpisodeChange<'_>, OperationError> {
        let open = self.open.try_lock().map_err(|_| {
            OperationError::conflict(
                "another recording operation is in progress; wait for it to finish",
            )
        })?;
        Ok(EpisodeChange { open })
    }

    /// Hold the episode once the operation holding it now has finished.
    pub(crate) async fn change(&self) -> EpisodeChange<'_> {
        EpisodeChange {
            open: self.open.lock().await,
        }
    }
}

impl EpisodeChange<'_> {
    pub(crate) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub(crate) fn episode(&self) -> Option<&str> {
        self.open.as_ref().map(|open| open.episode.as_str())
    }

    /// The open episode's file.
    pub(crate) fn file(&self) -> Option<&EpisodeFile> {
        self.open.as_ref().map(|open| &open.file)
    }

    /// The open episode's camera capture; `None` without an episode or cameras.
    pub(crate) fn capture(&mut self) -> Option<&mut VideoCapture> {
        self.open.as_mut()?.capture.as_mut()
    }

    /// Hold `episode` as the open one.
    pub(crate) fn open(&mut self, episode: RecordingEpisode) {
        debug_assert!(self.open.is_none(), "one episode is open at a time");
        *self.open = Some(episode);
    }

    /// Close the open episode, handing over its file to finish.
    pub(crate) fn close(mut self) -> Option<EpisodeFile> {
        self.open.take().map(|take| take.file)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use spiral_robotics_common::recording::EpisodeArm;
    use spiral_robotics_recording::{EpisodeRecordingRequest, RecordingBuffer};

    use super::*;

    fn open_episode(directory: &tempfile::TempDir, episode: &str) -> RecordingEpisode {
        let request = EpisodeRecordingRequest::new("data".to_owned(), "test".to_owned(), 60.0)
            .with_arms(
                vec![EpisodeArm::new("arm", None, vec!["joint".into()])],
                vec![],
            );
        let start = RecordingBuffer::new().begin(request).unwrap();
        let file = EpisodeFile::create(directory.path().join(episode), &start, Vec::new()).unwrap();
        RecordingEpisode::new(episode.to_owned(), None, file)
    }

    #[test]
    fn an_open_episode_stays_open_until_it_closes_with_its_file() {
        let directory = tempfile::tempdir().unwrap();
        let session = RecordingSession::default();
        let mut change = session.try_change().unwrap();
        change.open(open_episode(&directory, "episode-one"));
        assert!(
            session.try_change().is_err(),
            "one operation holds the episode"
        );
        drop(change);
        let change = session.try_change().unwrap();
        assert_eq!(change.episode(), Some("episode-one"));
        assert!(change.close().is_some());
        assert!(!session.try_change().unwrap().is_open());
    }

    #[tokio::test]
    async fn a_cancelled_operation_leaves_the_episode_open() {
        let directory = tempfile::tempdir().unwrap();
        let session = Arc::new(RecordingSession::default());
        session
            .try_change()
            .unwrap()
            .open(open_episode(&directory, "episode-one"));
        let (held, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn({
            let session = session.clone();
            async move {
                let _change = session.change().await;
                held.send(()).unwrap();
                std::future::pending::<()>().await;
            }
        });
        ready.await.unwrap();
        assert!(session.try_change().is_err());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(session.change().await.episode(), Some("episode-one"));
    }
}
