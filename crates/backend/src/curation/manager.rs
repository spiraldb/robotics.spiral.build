//! Saved curations and the views their outputs are published as. The manager
//! owns identity, size, and revision policy; the payload passes through
//! untouched. The browser compiles an output; publishing admits its statement
//! as read-only and hands it to a fill run, which writes the answers it reads
//! and leaves the DDL to storage.
//!
//! There is no in-process save lock: the conditional write in storage is the
//! single authority on which revision a document currently holds, so two tabs
//! racing to save produce exactly one winner and one conflict.
use super::fill::FillRuns;
use super::preview::PreviewRuns;
use crate::OperationError;
use spiral_robotics_common::curation::{
    CurationDeletion, CurationDocument, CurationSummary, FillRequest, FillStatus, PublishedView,
    UnpublishRequest, ViewName,
};
use spiral_robotics_storage::{ProviderKeys, StorageManager};
use std::sync::Arc;

/// The one message a losing save shows; reopening reloads the stored revision.
const SAVED_ELSEWHERE: &str = "This curation was saved elsewhere. Reopen it before saving.";
/// The same for a delete: the version on screen is no longer the stored one.
const DELETE_SAVED_ELSEWHERE: &str =
    "This curation was saved elsewhere. Reopen it before deleting.";

pub(crate) struct CurationManager {
    storage: Arc<StorageManager>,
    previews: PreviewRuns,
    fills: FillRuns,
}

impl CurationManager {
    /// `keys` reach the Modal service that scores progress.
    pub(crate) fn new(storage: Arc<StorageManager>, keys: Arc<ProviderKeys>) -> Self {
        Self {
            previews: PreviewRuns::new(storage.clone()),
            fills: FillRuns::new(storage.clone(), keys),
            storage,
        }
    }

    /// The preview runs this project's curations have started.
    pub(super) fn previews(&self) -> &PreviewRuns {
        &self.previews
    }

    /// The fill runs this project's curations have started.
    pub(super) fn fills(&self) -> &FillRuns {
        &self.fills
    }

    /// Abandon every preview and fill run; the project is closing.
    pub(crate) fn shutdown(&self) {
        self.previews.shutdown();
        self.fills.shutdown();
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

    /// Delete one saved curation at the revision it was loaded at. One saved
    /// elsewhere since is refused, so a delete never removes a version the
    /// operator has not seen, and one already gone is not found. The views
    /// its outputs publish are the browser's to drop first: only the payload
    /// names them.
    pub(super) async fn delete(&self, request: CurationDeletion) -> Result<(), OperationError> {
        let curations = self.storage.curations();
        if curations
            .delete(request.id(), request.revision())
            .await
            .map_err(OperationError::External)?
        {
            return Ok(());
        }
        match curations
            .load(request.id())
            .await
            .map_err(OperationError::External)?
        {
            Some(_) => Err(OperationError::Conflict(DELETE_SAVED_ELSEWHERE.into())),
            None => Err(OperationError::NotFound(
                "That curation no longer exists.".into(),
            )),
        }
    }

    /// Start filling the answers `request` asks for, then publishing its
    /// views, each replacing the view it names.
    pub(super) fn fill(&self, request: FillRequest) -> Result<FillStatus, OperationError> {
        let admitted = self.fills.admit(request)?;
        Ok(self.fills.start(admitted))
    }

    /// Every view the project's curations published.
    pub(super) async fn published(&self) -> Result<Vec<PublishedView>, OperationError> {
        self.storage
            .curated_views()
            .list()
            .await
            .map_err(OperationError::External)
    }

    /// Drop the view `name`, if one exists.
    pub(super) async fn unpublish(&self, request: UnpublishRequest) -> Result<(), OperationError> {
        let name = ViewName::try_from(request.name()).map_err(OperationError::InvalidInput)?;
        self.storage
            .curated_views()
            .drop(&name)
            .await
            .map_err(OperationError::External)
    }
}

#[cfg(test)]
mod tests;
