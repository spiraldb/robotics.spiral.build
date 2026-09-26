//! Saved curations and the views their outputs are published as. The manager
//! owns identity, size, and revision policy; the payload passes through
//! untouched. The browser compiles an output; publishing admits its statement
//! as read-only and leaves the DDL to storage.
//!
//! There is no in-process save lock: the conditional write in storage is the
//! single authority on which revision a document currently holds, so two tabs
//! racing to save produce exactly one winner and one conflict.
use super::preview::PreviewRuns;
use crate::OperationError;
use spiral_robotics_common::curation::{
    CurationDocument, CurationSummary, PublishRequest, UnpublishRequest, ViewName,
};
use spiral_robotics_common::sql::{ReadOnlyQuery, ReadOnlyQueryMode};
use spiral_robotics_storage::StorageManager;
use std::sync::Arc;

/// The one message a losing save shows; reopening reloads the stored revision.
const SAVED_ELSEWHERE: &str = "This curation was saved elsewhere. Reopen it before saving.";

pub(crate) struct CurationManager {
    storage: Arc<StorageManager>,
    previews: PreviewRuns,
}

impl CurationManager {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            previews: PreviewRuns::new(storage.clone()),
            storage,
        }
    }

    /// The preview runs this project's curations have started.
    pub(super) fn previews(&self) -> &PreviewRuns {
        &self.previews
    }

    /// Abandon every preview run; the project is closing.
    pub(crate) fn shutdown(&self) {
        self.previews.shutdown();
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

    /// Publish one output under `name`, replacing the view it names. The
    /// statement must be one read-only query, and the name must not belong to
    /// a table or a Dataset.
    pub(super) async fn publish(&self, request: PublishRequest) -> Result<(), OperationError> {
        let name = ViewName::try_from(request.name()).map_err(OperationError::InvalidInput)?;
        ReadOnlyQuery::new(request.statement(), ReadOnlyQueryMode::Curation)
            .authorize()
            .map_err(|error| OperationError::InvalidInput(error.to_string()))?;
        self.refuse_held(&name).await?;
        self.storage
            .curated_views()
            .replace(&name, request.statement())
            .await
            .map_err(OperationError::External)
    }

    /// Drop the view `name`, if one exists.
    pub(super) async fn unpublish(&self, request: UnpublishRequest) -> Result<(), OperationError> {
        let name = ViewName::try_from(request.name()).map_err(OperationError::InvalidInput)?;
        self.refuse_held(&name).await?;
        self.storage
            .curated_views()
            .drop(&name)
            .await
            .map_err(OperationError::External)
    }

    /// Refuse a name a table or a Dataset holds: publishing replaces only views.
    async fn refuse_held(&self, name: &ViewName) -> Result<(), OperationError> {
        let holder = self
            .storage
            .curated_views()
            .holder(name)
            .await
            .map_err(OperationError::External)?;
        match holder {
            Some(kind) => Err(OperationError::Conflict(format!(
                "“{name}” is a {kind} in this project; choose another name."
            ))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
