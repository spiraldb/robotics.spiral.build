//! One owner for the backend recording transaction.
//!
//! Hardware remains authoritative for physical sampling. This manager owns the
//! surrounding catalog reservation, episode identity, finalization phase, and
//! optional camera capture as one bounded session.

use std::sync::{Arc, Mutex};

use crate::OperationError;
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogLeaseToken, CatalogMutationLease};
use crate::robotics::video::{CameraRecording, VideoCapture};
use crate::sync::MutexRecovery;

#[derive(Clone, Debug)]
enum RecordingSessionPhase {
    Opening(CatalogLeaseToken),
    Recording {
        token: CatalogLeaseToken,
        episode: String,
    },
    Finalizing {
        token: CatalogLeaseToken,
        episode: String,
    },
}

impl RecordingSessionPhase {
    fn token(&self) -> CatalogLeaseToken {
        match self {
            Self::Opening(token)
            | Self::Recording { token, .. }
            | Self::Finalizing { token, .. } => *token,
        }
    }

    fn episode(&self) -> Option<&str> {
        match self {
            Self::Opening(_) => None,
            Self::Recording { episode, .. } | Self::Finalizing { episode, .. } => Some(episode),
        }
    }
}

pub(crate) struct RecordingSessionManager {
    catalog_lease: Arc<CatalogMutationLease>,
    phase: Mutex<Option<RecordingSessionPhase>>,
    media: tokio::sync::Mutex<Option<VideoCapture>>,
    preview: tokio::sync::Mutex<Option<(String, Vec<CameraRecording>)>>,
}

impl RecordingSessionManager {
    pub(crate) fn new(catalog_lease: Arc<CatalogMutationLease>) -> Self {
        Self {
            catalog_lease,
            phase: Mutex::new(None),
            media: tokio::sync::Mutex::new(None),
            preview: tokio::sync::Mutex::new(None),
        }
    }

    pub(crate) fn reserve_start(
        &self,
        activity: impl FnOnce() -> (bool, bool),
    ) -> Result<Option<CatalogLeaseToken>, OperationError> {
        if self.phase.recover_lock().is_some() {
            return Ok(None);
        }
        let Some(token) = self.catalog_lease.try_acquire(CatalogLeaseOwner::Recording) else {
            return Ok(None);
        };
        let (workflow_active, assistant_active) = activity();
        let conflict = if workflow_active {
            Some("Wait for the active Workflow run to finish before recording a new take.")
        } else if assistant_active {
            Some(
                "Wait for the active Workflow Assistant turn to finish before recording a new take.",
            )
        } else {
            None
        };
        if let Some(message) = conflict {
            let released = self.catalog_lease.release(token);
            debug_assert!(released, "rejected recording admission releases its lease");
            return Err(OperationError::Conflict(message.to_owned()));
        }
        *self.phase.recover_lock() = Some(RecordingSessionPhase::Opening(token));
        Ok(Some(token))
    }

    pub(crate) fn bind_episode(&self, token: CatalogLeaseToken, episode: String) -> bool {
        let mut phase = self.phase.recover_lock();
        if !matches!(phase.as_ref(), Some(RecordingSessionPhase::Opening(current)) if *current == token)
        {
            return false;
        }
        *phase = Some(RecordingSessionPhase::Recording { token, episode });
        true
    }

    pub(crate) fn claim_finalization(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
        let mut phase = self.phase.recover_lock();
        match phase.as_ref() {
            Some(RecordingSessionPhase::Opening(_)) => Err(OperationError::Conflict(
                "the recording is still opening; wait before stopping it".to_owned(),
            )),
            Some(RecordingSessionPhase::Recording { token, episode }) => {
                let token = *token;
                let episode = episode.clone();
                *phase = Some(RecordingSessionPhase::Finalizing { token, episode });
                Ok(Some(token))
            }
            Some(RecordingSessionPhase::Finalizing { .. }) | None => Ok(None),
        }
    }

    pub(crate) fn release(&self, token: CatalogLeaseToken) {
        let mut phase = self.phase.recover_lock();
        if phase
            .as_ref()
            .is_some_and(|current| current.token() == token)
        {
            *phase = None;
            let released = self.catalog_lease.release(token);
            debug_assert!(released, "recording releases its exact catalog lease once");
        }
    }

    pub(crate) fn episode(&self) -> Option<String> {
        self.phase
            .recover_lock()
            .as_ref()
            .and_then(RecordingSessionPhase::episode)
            .map(str::to_owned)
    }

    pub(crate) fn media_finalizing(&self) -> bool {
        self.media
            .try_lock()
            .map_or(true, |capture| capture.is_some())
    }

    pub(crate) async fn retain_media(&self, capture: Option<VideoCapture>) {
        *self.preview.lock().await = None;
        *self.media.lock().await = capture;
    }

    pub(crate) async fn take_media(&self) -> Option<VideoCapture> {
        self.media.lock().await.take()
    }

    pub(crate) async fn camera_preview(
        &self,
        episode: &str,
        stream: &str,
    ) -> Option<Result<spiral_robotics_common::datasets::EpisodeVideo, String>> {
        let media = self.media.lock().await;
        if let Some(capture) = media
            .as_ref()
            .filter(|capture| capture.episode_id() == episode)
        {
            return Some(CameraRecording::preview(
                &capture.completed_assets(),
                stream,
            ));
        }
        drop(media);
        let preview = self.preview.lock().await;
        preview
            .as_ref()
            .filter(|(id, _)| id == episode)
            .map(|(_, cameras)| CameraRecording::preview(cameras, stream))
    }

    pub(crate) async fn retain_preview(&self, episode: String, cameras: Vec<CameraRecording>) {
        *self.preview.lock().await = Some((episode, cameras));
    }

    pub(crate) async fn forget_preview(&self, episode: &str) {
        let mut preview = self.preview.lock().await;
        if preview.as_ref().is_some_and(|(id, _)| id == episode) {
            *preview = None;
        }
    }

    pub(crate) async fn request_media_pause(&self) {
        if let Some(capture) = self.media.lock().await.as_ref() {
            capture.request_pause();
        }
    }

    pub(crate) async fn finish_media_pause(
        &self,
    ) -> Option<(String, Vec<CameraRecording>, Vec<String>)> {
        let mut media = self.media.lock().await;
        let capture = media.as_mut()?;
        let failures = capture.finish_pause().await;
        Some((
            capture.episode_id().to_owned(),
            capture.completed_assets(),
            failures,
        ))
    }

    pub(crate) async fn resume_media(&self, active_seconds: f64) -> Result<(), String> {
        let mut media = self.media.lock().await;
        match media.as_mut() {
            Some(capture) => capture.resume(active_seconds),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completed_camera_preview_is_session_owned_and_cleared_for_the_next_take() {
        let session = RecordingSessionManager::new(Arc::new(CatalogMutationLease::default()));
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
            .retain_preview("take-one".into(), vec![camera])
            .await;
        assert!(session.camera_preview("another", "wrist").await.is_none());
        assert_eq!(
            session
                .camera_preview("take-one", "wrist")
                .await
                .unwrap()
                .unwrap()
                .packets()
                .len(),
            8
        );
        session.forget_preview("another").await;
        assert!(session.camera_preview("take-one", "wrist").await.is_some());
        session.retain_media(None).await;
        assert!(session.camera_preview("take-one", "wrist").await.is_none());
    }

    #[test]
    fn recording_identity_and_catalog_exclusion_have_one_lifecycle() {
        let lease = Arc::new(CatalogMutationLease::default());
        let session = RecordingSessionManager::new(lease.clone());
        let token = session.reserve_start(|| (false, false)).unwrap().unwrap();
        assert!(session.bind_episode(token, "episode_000001".to_owned()));
        assert_eq!(session.episode().as_deref(), Some("episode_000001"));
        assert_eq!(session.claim_finalization().unwrap(), Some(token));
        assert!(session.claim_finalization().unwrap().is_none());
        session.release(token);
        assert!(session.episode().is_none());
        assert!(!lease.is_held());
    }
}
