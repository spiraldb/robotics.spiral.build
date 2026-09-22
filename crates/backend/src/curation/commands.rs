//! Resource capabilities for saved recipes and judged runs.
use crate::BackendCommandError;
use crate::projects::context::ProjectContext;
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
        self.state
            .curation()
            .preview(&curation)
            .await
            .map_err(Into::into)
    }
    pub(super) async fn list(&self) -> Result<Vec<CurationSummary>, BackendCommandError> {
        self.state.curation().list().await.map_err(Into::into)
    }
    pub(super) async fn load(&self, id: &str) -> Result<CurationSpec, BackendCommandError> {
        self.state.curation().load(id).await.map_err(Into::into)
    }
    pub(super) async fn save(
        &self,
        curation: CurationSpec,
    ) -> Result<CurationSpec, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state.curation().save(curation).await.map_err(Into::into)
            })
            .await
    }
    pub(super) async fn run(
        &self,
        curation: CurationSpec,
    ) -> Result<CurationRun, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state.curation().run(curation).await.map_err(Into::into)
            })
            .await
    }
}
impl From<ProjectContext> for CurationCommands {
    fn from(state: ProjectContext) -> Self {
        Self { state }
    }
}
