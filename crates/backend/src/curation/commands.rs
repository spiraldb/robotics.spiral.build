//! Resource capabilities for saved recipes and judged runs.
use crate::data::catalog_lease::CatalogLeaseOwner;
use crate::project_session::context::ProjectContext;
use crate::{BackendCommandError, OperationError};
use spiral_robotics_common::curation::{CurationSpec, CurationSummary, run::CurationRun};

#[derive(Clone)]
pub(crate) struct CurationCommands {
    state: ProjectContext,
}
impl CurationCommands {
    fn admit(&self) -> Result<crate::project_admission::ProjectActivity, BackendCommandError> {
        let activity = self.state.admit()?;
        Ok(activity)
    }
    pub(crate) async fn list(&self) -> Result<Vec<CurationSummary>, BackendCommandError> {
        let _activity = self.admit()?;
        self.state.curation().list().await.map_err(Into::into)
    }
    pub(crate) async fn load(&self, id: &str) -> Result<CurationSpec, BackendCommandError> {
        let _activity = self.admit()?;
        self.state.curation().load(id).await.map_err(Into::into)
    }
    pub(crate) async fn save(
        &self,
        curation: CurationSpec,
    ) -> Result<CurationSpec, BackendCommandError> {
        self.state
            .execute(move |state, _activity| async move {
                state.curation().save(curation).await.map_err(Into::into)
            })
            .await
    }
    pub(crate) fn runs(&self) -> Result<Vec<CurationRun>, BackendCommandError> {
        let _activity = self.admit()?;
        Ok(self.state.curation().runs())
    }
    pub(crate) fn run(&self, id: &str) -> Result<CurationRun, BackendCommandError> {
        let _activity = self.admit()?;
        self.state.curation().run(id).map_err(Into::into)
    }
    pub(crate) fn start(&self, curation: CurationSpec) -> Result<CurationRun, BackendCommandError> {
        let _activity = self.admit()?;
        let lease = self.state.catalog_lease();
        let token = lease
            .try_acquire(CatalogLeaseOwner::CurationStart)
            .ok_or_else(|| {
                BackendCommandError::conflict(
                    "Wait for recording or catalog changes before running a curation.",
                )
            })?;
        let result = self.state.curation().start(curation);
        let released = lease.release(token);
        debug_assert!(released);
        result.map_err(|error: OperationError| error.into())
    }
}
impl From<ProjectContext> for CurationCommands {
    fn from(state: ProjectContext) -> Self {
        Self { state }
    }
}
