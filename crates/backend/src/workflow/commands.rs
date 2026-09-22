//! Resource capabilities; authoring and the agent loop belong to the caller.
use crate::data::catalog_lease::CatalogLeaseOwner;
use crate::project_session::context::ProjectContext;
use crate::{BackendCommandError, OperationError};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::workflow::{
    WorkflowSpec, WorkflowSummary,
    document::{WorkflowDocument, WorkflowPlan, WorkflowRun},
};

#[derive(Clone)]
pub(crate) struct WorkflowCommands {
    state: ProjectContext,
}
impl WorkflowCommands {
    pub(super) async fn list(&self) -> Result<Vec<WorkflowSummary>, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.state.workflow().list().await.map_err(Into::into)
    }
    pub(super) async fn load(&self, id: &str) -> Result<WorkflowDocument, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.state.workflow().load(id).await.map_err(Into::into)
    }
    pub(super) async fn save(
        &self,
        document: WorkflowDocument,
    ) -> Result<WorkflowDocument, BackendCommandError> {
        self.state
            .execute(move |state, _activity| async move {
                state.workflow().save(document).await.map_err(Into::into)
            })
            .await
    }
    pub(super) async fn plan(
        &self,
        workflow: WorkflowSpec,
    ) -> Result<WorkflowPlan, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.workflow().plan(&workflow).await)
    }
    pub(super) async fn run(
        &self,
        workflow: WorkflowSpec,
    ) -> Result<WorkflowRun, BackendCommandError> {
        let admission = self
            .state
            .catalog_lease()
            .try_reserve(CatalogLeaseOwner::WorkflowStart)
            .ok_or_else(|| {
                BackendCommandError::conflict(
                    "Wait for recording or catalog changes before running a workflow.",
                )
            })?;
        self.state
            .execute(move |state, _activity| async move {
                state
                    .workflow()
                    .run(workflow, admission)
                    .await
                    .map_err(Into::into)
            })
            .await
    }
    pub(super) async fn capabilities(
        &self,
    ) -> Result<spiral_robotics_common::relations::SpiralQuerySnapshot, BackendCommandError> {
        let _activity = self.state.admit()?;
        super::query::WorkflowQuery::new(self.state.storage().workflow_store())
            .capabilities()
            .await
            .map_err(OperationError::External)
            .map_err(Into::into)
    }
    pub(super) async fn cameras(
        &self,
        selection: DatasetSelection,
    ) -> Result<serde_json::Value, BackendCommandError> {
        let _activity = self.state.admit()?;
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
