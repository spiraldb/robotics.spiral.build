//! Resource capabilities for saved recipes and judged runs.
use crate::BackendCommandError;
use crate::data::catalog_lease::CatalogLeaseOwner;
use crate::project_session::context::ProjectContext;
use spiral_robotics_common::curation::{CurationSpec, CurationSummary, run::CurationRun};

#[derive(Clone)]
pub(crate) struct CurationCommands {
    state: ProjectContext,
}
impl CurationCommands {
    pub(super) async fn score_episode(
        &self,
        request: spiral_robotics_common::curation::progress::ProgressScoreRequest,
    ) -> Result<spiral_robotics_common::curation::progress::ProgressScoreResult, BackendCommandError>
    {
        let _activity = self.state.admit()?;
        self.state
            .curation()
            .score_episode(request)
            .await
            .map_err(Into::into)
    }
    pub(super) async fn preview(
        &self,
        curation: CurationSpec,
    ) -> Result<spiral_robotics_common::curation::run::CurationOutcome, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.state
            .curation()
            .preview(&curation)
            .await
            .map_err(Into::into)
    }
    pub(super) async fn list(&self) -> Result<Vec<CurationSummary>, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.state.curation().list().await.map_err(Into::into)
    }
    pub(super) async fn load(&self, id: &str) -> Result<CurationSpec, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.state.curation().load(id).await.map_err(Into::into)
    }
    pub(super) async fn save(
        &self,
        curation: CurationSpec,
    ) -> Result<CurationSpec, BackendCommandError> {
        self.state
            .execute(move |state, _activity| async move {
                state.curation().save(curation).await.map_err(Into::into)
            })
            .await
    }
    pub(super) async fn run(
        &self,
        curation: CurationSpec,
    ) -> Result<CurationRun, BackendCommandError> {
        let admission = self
            .state
            .catalog_lease()
            .try_reserve(CatalogLeaseOwner::CurationStart)
            .ok_or_else(|| {
                BackendCommandError::conflict(
                    "Wait for recording or catalog changes before running a curation.",
                )
            })?;
        self.state
            .execute(move |state, _activity| async move {
                state
                    .curation()
                    .run(curation, admission)
                    .await
                    .map_err(Into::into)
            })
            .await
    }
}
impl From<ProjectContext> for CurationCommands {
    fn from(state: ProjectContext) -> Self {
        Self { state }
    }
}
