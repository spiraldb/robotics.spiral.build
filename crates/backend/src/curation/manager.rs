//! Saved curations and the progress scores one of their nodes asks for. The
//! manager owns identity, size, and revision policy; the payload passes
//! through untouched.
//!
//! There is no in-process save lock: the conditional write in storage is the
//! single authority on which revision a document currently holds, so two tabs
//! racing to save produce exactly one winner and one conflict.
use super::episode_progress::EpisodeProgress;
use crate::OperationError;
use spiral_robotics_common::curation::progress::{ProgressScoreRequest, ProgressScoreResult};
use spiral_robotics_common::curation::{CurationDocument, CurationSummary};
use spiral_robotics_storage::StorageManager;
use std::sync::Arc;

/// The one message a losing save shows; reopening reloads the stored revision.
const SAVED_ELSEWHERE: &str = "This curation was saved elsewhere. Reopen it before saving.";

/// Shared per project, because a score in flight is shared: two views asking
/// about one recording wait on one inference.
pub(crate) struct CurationManager {
    storage: Arc<StorageManager>,
    episode_progress: EpisodeProgress,
}

impl CurationManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            episode_progress: EpisodeProgress::new(storage.clone()),
            storage,
        }
    }

    pub(super) async fn list(&self) -> Result<Vec<CurationSummary>, OperationError> {
        self.storage
            .curations()
            .list()
            .await
            .map_err(OperationError::External)
    }

    pub(super) async fn load(&self, id: &str) -> Result<CurationDocument, OperationError> {
        self.storage
            .curations()
            .load(id)
            .await
            .map_err(OperationError::External)?
            .ok_or_else(|| OperationError::NotFound("That curation no longer exists.".into()))
    }

    /// Save one revision. The submitted revision must equal the stored one, or
    /// be 0 for a document that has never been saved; the stored copy carries
    /// the next revision and is what the caller must edit from now on.
    pub(super) async fn save(
        &self,
        document: CurationDocument,
    ) -> Result<CurationDocument, OperationError> {
        document.validate().map_err(OperationError::InvalidInput)?;
        let stored = document
            .revision()
            .checked_add(1)
            .ok_or_else(|| OperationError::Internal("Curation revision overflowed.".into()))?;
        if !self
            .storage
            .curations()
            .save(&document)
            .await
            .map_err(OperationError::External)?
        {
            return Err(OperationError::Conflict(SAVED_ELSEWHERE.into()));
        }
        Ok(document.with_revision(stored))
    }

    pub(super) async fn score_episode(
        &self,
        request: ProgressScoreRequest,
    ) -> Result<ProgressScoreResult, OperationError> {
        self.episode_progress
            .score(request)
            .await
            .map_err(OperationError::External)
    }
}

#[cfg(test)]
mod tests;
