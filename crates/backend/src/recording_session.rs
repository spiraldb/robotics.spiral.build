//! Recording lifecycle and ownership. Encoder work runs outside the state lock.

use std::sync::{Arc, Mutex};

use crate::OperationError;
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease};
use crate::robotics::video::{CameraRecording, VideoCapture};
use crate::sync::MutexRecovery;

enum RecordingSessionState {
    Idle,
    Opening(CatalogLeaseToken),
    Recording(ActiveRecording),
    UpdatingCapture(RecordingIdentity),
    Finalizing(RecordingIdentity),
    Review(CompletedRecording),
}

#[derive(Clone)]
struct RecordingIdentity {
    token: CatalogLeaseToken,
    episode: String,
}

struct ActiveRecording {
    identity: RecordingIdentity,
    capture: Option<VideoCapture>,
}

struct CompletedRecording {
    episode: String,
    cameras: Vec<CameraRecording>,
}

pub(crate) struct RecordingSessionManager {
    catalog_lease: Arc<CatalogMutationLease>,
    state: Mutex<RecordingSessionState>,
}

/// Owns the capture while an asynchronous operation is in flight. Dropping an
/// unfinished operation restores the capture for retry, retaining catalog exclusion.
pub(crate) struct RecordingOperation {
    session: Arc<RecordingSessionManager>,
    recording: Option<ActiveRecording>,
}

impl RecordingOperation {
    pub(crate) fn episode(&self) -> &str {
        &self
            .recording
            .as_ref()
            .expect("live recording operation")
            .identity
            .episode
    }

    pub(crate) async fn finish_media(&mut self) -> (Vec<CameraRecording>, Vec<String>) {
        let recording = self.recording.as_mut().expect("live recording operation");
        match recording.capture.as_mut() {
            Some(capture) => {
                let failures = capture.finish_pause().await;
                (capture.completed_assets(), failures)
            }
            None => (Vec::new(), Vec::new()),
        }
    }

    pub(crate) fn request_pause(&self) {
        if let Some(capture) = &self
            .recording
            .as_ref()
            .expect("live recording operation")
            .capture
        {
            capture.request_pause();
        }
    }

    pub(crate) fn resume(&mut self, active_seconds: f64) -> Result<(), String> {
        match self
            .recording
            .as_mut()
            .expect("live recording operation")
            .capture
            .as_mut()
        {
            Some(capture) => capture.resume(active_seconds),
            None => Ok(()),
        }
    }

    /// Publication has finished (successfully or with an error). Keep the take
    /// available for review and release its catalog lease in the same transition.
    pub(crate) fn complete(mut self, cameras: Vec<CameraRecording>) {
        let recording = self.recording.take().expect("live recording operation");
        let mut state = self.session.state.recover_lock();
        assert!(
            matches!(&*state, RecordingSessionState::Finalizing(identity)
            if identity.token == recording.identity.token)
        );
        let released = self.session.catalog_lease.release(recording.identity.token);
        debug_assert!(released, "finalization releases its exact catalog lease");
        *state = RecordingSessionState::Review(CompletedRecording {
            episode: recording.identity.episode,
            cameras,
        });
    }
}

impl Drop for RecordingOperation {
    fn drop(&mut self) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        let mut state = self.session.state.recover_lock();
        if matches!(&*state,
            RecordingSessionState::UpdatingCapture(identity) | RecordingSessionState::Finalizing(identity)
            if identity.token == recording.identity.token)
        {
            *state = RecordingSessionState::Recording(recording);
        }
    }
}

impl RecordingSessionManager {
    pub(crate) fn new(catalog_lease: Arc<CatalogMutationLease>) -> Self {
        Self {
            catalog_lease,
            state: Mutex::new(RecordingSessionState::Idle),
        }
    }

    pub(crate) fn reserve_start(
        &self,
        activity: impl FnOnce() -> bool,
    ) -> Result<Option<CatalogLeaseToken>, OperationError> {
        let mut state = self.state.recover_lock();
        if !matches!(
            *state,
            RecordingSessionState::Idle | RecordingSessionState::Review(_)
        ) {
            return Ok(None);
        }
        let Some(token) = self.catalog_lease.try_acquire(CatalogLeaseOwner::Recording) else {
            return Ok(None);
        };
        let workflow_active = activity();
        let conflict = if workflow_active {
            Some("Wait for the active Workflow run to finish before recording a new take.")
        } else {
            None
        };
        if let Some(message) = conflict {
            let released = self.catalog_lease.release(token);
            debug_assert!(released, "rejected recording admission releases its lease");
            return Err(OperationError::Conflict(message.to_owned()));
        }
        *state = RecordingSessionState::Opening(token);
        Ok(Some(token))
    }

    pub(crate) fn begin(&self, token: CatalogLeaseToken, take: (String, Option<VideoCapture>)) {
        let mut state = self.state.recover_lock();
        assert!(matches!(*state, RecordingSessionState::Opening(current) if current == token));
        let (episode, capture) = take;
        assert!(
            capture
                .as_ref()
                .is_none_or(|capture| capture.episode_id() == episode)
        );
        *state = RecordingSessionState::Recording(ActiveRecording {
            identity: RecordingIdentity { token, episode },
            capture,
        });
    }

    pub(crate) fn cancel_start(&self, token: CatalogLeaseToken) {
        let mut state = self.state.recover_lock();
        if matches!(*state, RecordingSessionState::Opening(current) if current == token) {
            let released = self.catalog_lease.release(token);
            debug_assert!(released, "opening releases its exact catalog lease");
            *state = RecordingSessionState::Idle;
        }
    }

    pub(crate) fn claim_finalization(
        self: &Arc<Self>,
    ) -> Result<Option<RecordingOperation>, OperationError> {
        let mut state = self.state.recover_lock();
        match &*state {
            RecordingSessionState::Opening(_) => {
                return Err(OperationError::conflict(
                    "the recording is still opening; wait before stopping it",
                ));
            }
            RecordingSessionState::UpdatingCapture(_) => {
                return Err(OperationError::conflict(
                    "the recording is changing pause state; wait before stopping it",
                ));
            }
            RecordingSessionState::Recording(_) => {}
            _ => return Ok(None),
        }
        let RecordingSessionState::Recording(recording) =
            std::mem::replace(&mut *state, RecordingSessionState::Idle)
        else {
            unreachable!()
        };
        *state = RecordingSessionState::Finalizing(recording.identity.clone());
        Ok(Some(RecordingOperation {
            session: Arc::clone(self),
            recording: Some(recording),
        }))
    }

    pub(crate) fn episode(&self) -> Option<String> {
        match &*self.state.recover_lock() {
            RecordingSessionState::Recording(recording) => Some(recording.identity.episode.clone()),
            RecordingSessionState::UpdatingCapture(identity)
            | RecordingSessionState::Finalizing(identity) => Some(identity.episode.clone()),
            _ => None,
        }
    }

    pub(crate) fn camera_preview(
        &self,
        episode: &str,
        stream: &str,
    ) -> Option<Result<spiral_robotics_common::datasets::EpisodeVideo, String>> {
        let cameras = match &*self.state.recover_lock() {
            RecordingSessionState::Recording(recording)
                if recording.identity.episode == episode =>
            {
                recording.capture.as_ref()?.completed_assets()
            }
            RecordingSessionState::Review(review) if review.episode == episode => {
                review.cameras.clone()
            }
            _ => return None,
        };
        Some(CameraRecording::preview(&cameras, stream))
    }

    pub(crate) fn forget_preview(&self, episode: &str) {
        let mut state = self.state.recover_lock();
        if matches!(&*state, RecordingSessionState::Review(review) if review.episode == episode) {
            *state = RecordingSessionState::Idle;
        }
    }

    pub(crate) fn claim_capture_change(
        self: &Arc<Self>,
    ) -> Result<RecordingOperation, OperationError> {
        let mut state = self.state.recover_lock();
        if !matches!(*state, RecordingSessionState::Recording(_)) {
            return Err(OperationError::conflict(
                "recording is not available for pause",
            ));
        }
        let RecordingSessionState::Recording(recording) =
            std::mem::replace(&mut *state, RecordingSessionState::Idle)
        else {
            unreachable!()
        };
        *state = RecordingSessionState::UpdatingCapture(recording.identity.clone());
        Ok(RecordingOperation {
            session: Arc::clone(self),
            recording: Some(recording),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalization_excludes_other_operations_and_drop_restores_the_take() {
        let lease = Arc::new(CatalogMutationLease::default());
        let session = Arc::new(RecordingSessionManager::new(lease.clone()));
        let token = session.reserve_start(|| false).unwrap().unwrap();
        assert!(session.claim_finalization().is_err());
        session.begin(token, ("take-one".into(), None));
        let operation = session.claim_finalization().unwrap().unwrap();
        assert_eq!(operation.episode(), "take-one");
        assert!(
            session.state.try_lock().is_ok(),
            "operation owns data, not the mutex guard"
        );
        assert!(session.claim_finalization().unwrap().is_none());
        assert!(session.claim_capture_change().is_err());
        assert!(session.reserve_start(|| false).unwrap().is_none());
        // A stale opening cleanup cannot release a recording's lease.
        session.cancel_start(token);
        assert!(lease.is_held());
        drop(operation);
        assert_eq!(session.episode().as_deref(), Some("take-one"));
        assert!(lease.is_held());
        session
            .claim_finalization()
            .unwrap()
            .unwrap()
            .complete(Vec::new());
        assert!(session.episode().is_none());
        assert!(!lease.is_held());
    }

    #[tokio::test]
    async fn cancelling_a_capture_change_restores_the_session_for_stop() {
        let lease = Arc::new(CatalogMutationLease::default());
        let session = Arc::new(RecordingSessionManager::new(lease.clone()));
        let token = session.reserve_start(|| false).unwrap().unwrap();
        session.begin(token, ("take-one".into(), None));
        let (claimed, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn({
            let session = session.clone();
            async move {
                let _operation = session.claim_capture_change().unwrap();
                claimed.send(()).unwrap();
                std::future::pending::<()>().await;
            }
        });
        ready.await.unwrap();
        assert!(session.state.try_lock().is_ok());
        assert!(session.claim_finalization().is_err());
        assert!(session.claim_capture_change().is_err());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(lease.is_held());
        let mut operation = session.claim_finalization().unwrap().unwrap();
        let (cameras, failures) = operation.finish_media().await;
        assert!(cameras.is_empty() && failures.is_empty());
        operation.complete(cameras);
        assert!(!lease.is_held());
    }

    #[test]
    fn completed_preview_belongs_to_the_take_and_is_replaced_on_next_start() {
        let lease = Arc::new(CatalogMutationLease::default());
        let session = Arc::new(RecordingSessionManager::new(lease.clone()));
        let token = session.reserve_start(|| false).unwrap().unwrap();
        session.begin(token, ("take-one".into(), None));
        let camera = CameraRecording::from_annex_b(
            "wrist".into(),
            (32, 32, 30),
            (
                include_bytes!("../../frontend/src/activities/data/fixtures/seek.h264").to_vec(),
                (0..8).map(|index| index * 33_333_333).collect(),
            ),
        )
        .unwrap();
        session
            .claim_finalization()
            .unwrap()
            .unwrap()
            .complete(vec![camera]);
        assert!(!lease.is_held());
        assert!(session.camera_preview("another", "wrist").is_none());
        session.forget_preview("another");
        assert_eq!(
            session
                .camera_preview("take-one", "wrist")
                .unwrap()
                .unwrap()
                .packets()
                .len(),
            8
        );
        // Rejected admission preserves the review.
        assert!(session.reserve_start(|| true).is_err());
        assert!(session.camera_preview("take-one", "wrist").is_some());
        let next = session.reserve_start(|| false).unwrap().unwrap();
        assert!(session.camera_preview("take-one", "wrist").is_none());
        session.cancel_start(token);
        assert!(lease.is_held(), "old token cannot cancel a new session");
        session.cancel_start(next);
        assert!(!lease.is_held());
    }
}
