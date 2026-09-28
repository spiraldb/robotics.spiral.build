//! The one take a project may have open, and the last one kept for review.
//!
//! Whichever operation changes the take — start, pause, stop, or closing a
//! take the control loop ended — holds it for the whole operation. Operations
//! never interleave, and one that is cancelled leaves the take as it was.
use tokio::sync::{Mutex, MutexGuard};

use crate::OperationError;
use crate::robotics::video::{CameraRecording, VideoCapture};
use crate::sync::MutexRecovery;

#[derive(Default)]
pub(crate) struct RecordingSession {
    open: Mutex<Option<OpenTake>>,
    review: std::sync::Mutex<Option<ReviewedTake>>,
}

/// A take the control loop is recording, until its episode and media are
/// committed.
pub(crate) struct OpenTake {
    episode: String,
    capture: Option<VideoCapture>,
}

struct ReviewedTake {
    episode: String,
    cameras: Vec<CameraRecording>,
}

/// One operation's exclusive hold on the session's take.
pub(crate) struct TakeChange<'a> {
    open: MutexGuard<'a, Option<OpenTake>>,
    review: &'a std::sync::Mutex<Option<ReviewedTake>>,
}

impl OpenTake {
    pub(crate) fn new(episode: String, capture: Option<VideoCapture>) -> Self {
        debug_assert!(
            capture
                .as_ref()
                .is_none_or(|capture| capture.episode_id() == episode)
        );
        Self { episode, capture }
    }
}

impl RecordingSession {
    /// Hold the take for a command, refusing while another operation holds it.
    pub(crate) fn try_change(&self) -> Result<TakeChange<'_>, OperationError> {
        let open = self.open.try_lock().map_err(|_| {
            OperationError::conflict(
                "another recording operation is in progress; wait for it to finish",
            )
        })?;
        Ok(TakeChange {
            open,
            review: &self.review,
        })
    }

    /// Hold the take once the operation holding it now has finished.
    pub(crate) async fn change(&self) -> TakeChange<'_> {
        TakeChange {
            open: self.open.lock().await,
            review: &self.review,
        }
    }

    /// A take is open, or an operation is changing it.
    pub(crate) fn active(&self) -> bool {
        self.open.try_lock().map_or(true, |open| open.is_some())
    }

    pub(crate) fn camera_preview(
        &self,
        episode: &str,
        stream: &str,
    ) -> Option<Result<spiral_robotics_common::datasets::EpisodeVideo, String>> {
        let live = self.open.try_lock().ok().and_then(|open| {
            let take = open.as_ref().filter(|take| take.episode == episode)?;
            Some(take.capture.as_ref()?.completed_assets())
        });
        let cameras = match live {
            Some(cameras) => cameras,
            None => self
                .review
                .recover_lock()
                .as_ref()
                .filter(|review| review.episode == episode)?
                .cameras
                .clone(),
        };
        Some(CameraRecording::preview(&cameras, stream))
    }
}

impl TakeChange<'_> {
    pub(crate) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub(crate) fn episode(&self) -> Option<&str> {
        self.open.as_ref().map(|take| take.episode.as_str())
    }

    /// The open take's camera capture; `None` without a take or cameras.
    pub(crate) fn capture(&mut self) -> Option<&mut VideoCapture> {
        self.open.as_mut()?.capture.as_mut()
    }

    /// Hold `take` as the open one; the previous take's review is gone.
    pub(crate) fn open(&mut self, take: OpenTake) {
        debug_assert!(self.open.is_none(), "one take is open at a time");
        *self.review.recover_lock() = None;
        *self.open = Some(take);
    }

    /// Close the open take for review: its media stays previewable.
    pub(crate) fn close(mut self, cameras: Vec<CameraRecording>) {
        if let Some(take) = self.open.take() {
            *self.review.recover_lock() = Some(ReviewedTake {
                episode: take.episode,
                cameras,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn take(episode: &str) -> OpenTake {
        OpenTake::new(episode.to_owned(), None)
    }

    #[test]
    fn an_open_take_stays_active_until_it_closes_for_review() {
        let session = RecordingSession::default();
        let mut change = session.try_change().unwrap();
        change.open(take("take-one"));
        assert!(
            session.try_change().is_err(),
            "one operation holds the take"
        );
        drop(change);
        assert!(session.active());
        let change = session.try_change().unwrap();
        assert_eq!(change.episode(), Some("take-one"));
        change.close(Vec::new());
        assert!(!session.active());
    }

    #[tokio::test]
    async fn a_cancelled_operation_leaves_the_take_open() {
        let session = Arc::new(RecordingSession::default());
        session.try_change().unwrap().open(take("take-one"));
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
        assert_eq!(session.change().await.episode(), Some("take-one"));
    }

    #[test]
    fn completed_preview_belongs_to_the_take_and_is_replaced_on_next_open() {
        let session = RecordingSession::default();
        session.try_change().unwrap().open(take("take-one"));
        let camera = CameraRecording::from_annex_b(
            "wrist".into(),
            (32, 32, 30),
            (
                include_bytes!("../../../frontend/src/activities/episode/fixtures/seek.h264")
                    .to_vec(),
                (0..8).map(|index| index * 33_333_333).collect(),
            ),
        )
        .unwrap();
        session.try_change().unwrap().close(vec![camera]);
        assert!(session.camera_preview("another", "wrist").is_none());
        assert_eq!(
            session
                .camera_preview("take-one", "wrist")
                .unwrap()
                .unwrap()
                .packets()
                .len(),
            8
        );
        session.try_change().unwrap().open(take("take-two"));
        assert!(session.camera_preview("take-one", "wrist").is_none());
    }
}
