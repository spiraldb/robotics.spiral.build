//! Resource capabilities; authoring and the agent loop belong to the caller.
use crate::projects::context::ProjectContext;
use crate::{BackendCommandError, OperationError};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::workflow::{
    WorkflowSpec, WorkflowSummary,
    document::{WorkflowDocument, WorkflowRun},
};

#[derive(Clone)]
pub(crate) struct WorkflowCommands {
    state: ProjectContext,
}
impl WorkflowCommands {
    pub(super) async fn list(&self) -> Result<Vec<WorkflowSummary>, BackendCommandError> {
        self.state.workflow().list().await.map_err(Into::into)
    }
    pub(super) async fn load(&self, id: &str) -> Result<WorkflowDocument, BackendCommandError> {
        self.state.workflow().load(id).await.map_err(Into::into)
    }
    pub(super) async fn save(
        &self,
        document: WorkflowDocument,
    ) -> Result<WorkflowDocument, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state.workflow().save(document).await.map_err(Into::into)
            })
            .await
    }
    pub(super) async fn run(
        &self,
        workflow: WorkflowSpec,
    ) -> Result<WorkflowRun, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state.workflow().run(workflow).await.map_err(Into::into)
            })
            .await
    }
    pub(super) async fn cameras(
        &self,
        selection: DatasetSelection,
    ) -> Result<serde_json::Value, BackendCommandError> {
        let inspection = self
            .state
            .storage()
            .inspect_dataset(selection)
            .await
            .map_err(OperationError::External)?;
        let cameras = self
            .state
            .storage()
            .workflow_cameras(&inspection)
            .await
            .map_err(OperationError::External)?;
        Ok(serde_json::json!({"dataset": inspection, "cameras": cameras}))
    }
}
impl From<ProjectContext> for WorkflowCommands {
    fn from(state: ProjectContext) -> Self {
        Self { state }
    }
}
