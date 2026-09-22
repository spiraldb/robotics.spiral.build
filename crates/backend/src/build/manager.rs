//! Saved Build documents. The manager owns identity, size, and revision
//! policy; the payload passes through untouched.
//!
//! There is no in-process save lock: the conditional write in storage is the
//! single authority on which revision a document currently holds, so two tabs
//! racing to save produce exactly one winner and one conflict.
use crate::OperationError;
use spiral_robotics_common::build::{BuildDocument, BuildDocumentSummary};
use spiral_robotics_storage::StorageManager;
use std::sync::Arc;

/// The one message a losing save shows; reopening reloads the stored revision.
const SAVED_ELSEWHERE: &str = "This Build document was saved elsewhere. Reopen it before saving.";

pub(crate) struct BuildDocumentManager {
    storage: Arc<StorageManager>,
}

impl BuildDocumentManager {
    pub(crate) const fn new(storage: Arc<StorageManager>) -> Self {
        Self { storage }
    }

    pub(super) async fn list(&self) -> Result<Vec<BuildDocumentSummary>, OperationError> {
        self.storage
            .build_documents()
            .await
            .map_err(OperationError::External)
    }

    pub(super) async fn load(&self, id: &str) -> Result<BuildDocument, OperationError> {
        self.storage
            .load_build_document(id)
            .await
            .map_err(OperationError::External)?
            .ok_or_else(|| OperationError::NotFound("That Build document no longer exists.".into()))
    }

    /// Save one revision. The submitted revision must equal the stored one, or
    /// be 0 for a document that has never been saved; the stored copy carries
    /// the next revision and is what the caller must edit from now on.
    pub(super) async fn save(
        &self,
        document: BuildDocument,
    ) -> Result<BuildDocument, OperationError> {
        document.validate().map_err(OperationError::InvalidInput)?;
        let stored = document.revision().checked_add(1).ok_or_else(|| {
            OperationError::Internal("Build document revision overflowed.".into())
        })?;
        if !self
            .storage
            .save_build_document(&document)
            .await
            .map_err(OperationError::External)?
        {
            return Err(OperationError::Conflict(SAVED_ELSEWHERE.into()));
        }
        Ok(document.with_revision(stored))
    }
}

#[cfg(test)]
mod tests;
