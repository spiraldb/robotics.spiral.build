//! Resource capabilities for saved explorations.
use crate::BackendCommandError;
use crate::projects::context::ProjectContext;
use spiral_robotics_common::exploration::{ExplorationDocument, ExplorationSummary};

#[derive(Clone)]
pub(crate) struct ExplorationCommands {
    state: ProjectContext,
}
impl ExplorationCommands {
    pub(super) async fn list(&self) -> Result<Vec<ExplorationSummary>, BackendCommandError> {
        self.state.explorations().list().await.map_err(Into::into)
    }
    pub(super) async fn load(&self, id: &str) -> Result<ExplorationDocument, BackendCommandError> {
        self.state.explorations().load(id).await.map_err(Into::into)
    }
    /// A save outlives its HTTP caller: a browser that disconnects mid-write
    /// must not leave the stored revision ambiguous.
    pub(super) async fn save(
        &self,
        document: ExplorationDocument,
    ) -> Result<ExplorationDocument, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state
                    .explorations()
                    .save(document)
                    .await
                    .map_err(Into::into)
            })
            .await
    }
}
impl From<ProjectContext> for ExplorationCommands {
    fn from(state: ProjectContext) -> Self {
        Self { state }
    }
}
