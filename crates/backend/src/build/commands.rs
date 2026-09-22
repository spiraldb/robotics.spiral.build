//! Resource capabilities for saved Build documents.
use crate::BackendCommandError;
use crate::project_session::context::ProjectContext;
use spiral_robotics_common::build::{BuildDocument, BuildDocumentSummary};

#[derive(Clone)]
pub(crate) struct BuildCommands {
    state: ProjectContext,
}
impl BuildCommands {
    pub(super) async fn list(&self) -> Result<Vec<BuildDocumentSummary>, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.state
            .build_documents()
            .list()
            .await
            .map_err(Into::into)
    }
    pub(super) async fn load(&self, id: &str) -> Result<BuildDocument, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.state
            .build_documents()
            .load(id)
            .await
            .map_err(Into::into)
    }
    /// A save outlives its HTTP caller: a browser that disconnects mid-write
    /// must not leave the stored revision ambiguous.
    pub(super) async fn save(
        &self,
        document: BuildDocument,
    ) -> Result<BuildDocument, BackendCommandError> {
        self.state
            .execute(move |state, _activity| async move {
                state
                    .build_documents()
                    .save(document)
                    .await
                    .map_err(Into::into)
            })
            .await
    }
}
impl From<ProjectContext> for BuildCommands {
    fn from(state: ProjectContext) -> Self {
        Self { state }
    }
}
