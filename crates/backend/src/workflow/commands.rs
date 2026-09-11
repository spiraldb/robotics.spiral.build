//! Resource capabilities; authoring and the agent loop belong to the caller.
use super::provider::{ModelStream, OpenAiProvider, ProviderConfig};
use crate::data::catalog_lease::CatalogLeaseOwner;
use crate::project_session::context::ProjectContext;
use crate::{BackendCommandError, OperationError};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::workflow::{
    WorkflowAgentReadiness, WorkflowSpec, WorkflowSummary,
    document::{ModelRequest, WorkflowDocument, WorkflowPlan, WorkflowRun},
};

#[derive(Clone)]
pub(crate) struct WorkflowCommands {
    state: ProjectContext,
}
impl WorkflowCommands {
    fn admit(&self) -> Result<crate::project_admission::ProjectActivity, BackendCommandError> {
        let activity = self.state.admit()?;
        self.state.require_catalog_ready()?;
        Ok(activity)
    }
    pub(crate) async fn list(&self) -> Result<Vec<WorkflowSummary>, BackendCommandError> {
        let _activity = self.admit()?;
        self.state.workflow().list().await.map_err(Into::into)
    }
    pub(crate) async fn load(&self, id: &str) -> Result<WorkflowDocument, BackendCommandError> {
        let _activity = self.admit()?;
        self.state.workflow().load(id).await.map_err(Into::into)
    }
    pub(crate) async fn save(
        &self,
        document: WorkflowDocument,
    ) -> Result<WorkflowDocument, BackendCommandError> {
        self.state
            .execute(move |state, _activity| async move {
                state.require_catalog_ready()?;
                state.workflow().save(document).await.map_err(Into::into)
            })
            .await
    }
    pub(crate) async fn plan(
        &self,
        workflow: WorkflowSpec,
    ) -> Result<WorkflowPlan, BackendCommandError> {
        let _activity = self.admit()?;
        Ok(self.state.workflow().plan(&workflow).await)
    }
    pub(crate) fn runs(&self) -> Result<Vec<WorkflowRun>, BackendCommandError> {
        let _activity = self.admit()?;
        Ok(self.state.workflow().runs())
    }
    pub(crate) fn run(&self, id: &str) -> Result<WorkflowRun, BackendCommandError> {
        let _activity = self.admit()?;
        self.state.workflow().run(id).map_err(Into::into)
    }
    pub(crate) fn start(&self, workflow: WorkflowSpec) -> Result<WorkflowRun, BackendCommandError> {
        let _activity = self.admit()?;
        let lease = self.state.catalog_lease();
        let token = lease
            .try_acquire(CatalogLeaseOwner::WorkflowStart)
            .ok_or_else(|| {
                BackendCommandError::conflict(
                    "Wait for recording or catalog changes before running a workflow.",
                )
            })?;
        let result = self.state.workflow().start(workflow);
        let released = lease.release(token);
        debug_assert!(released);
        result.map_err(Into::into)
    }
    pub(crate) fn cancel(&self, id: &str) -> Result<WorkflowRun, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.state.workflow().cancel(id).map_err(Into::into)
    }
    pub(crate) async fn cameras(
        &self,
        selection: DatasetSelection,
    ) -> Result<serde_json::Value, BackendCommandError> {
        let _activity = self.admit()?;
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
    pub(crate) fn model_status(&self) -> Result<WorkflowAgentReadiness, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(match ProviderConfig::from_env() {
            Ok(Some(_)) => WorkflowAgentReadiness::ready("Ready to help author this workflow."),
            Ok(None) => WorkflowAgentReadiness::error(
                "Configure LLM_API_KEY on the backend to enable the assistant.",
            ),
            Err(error) => WorkflowAgentReadiness::error(error),
        })
    }
    pub(crate) fn complete(
        &self,
        request: ModelRequest,
    ) -> Result<ModelStream, BackendCommandError> {
        let _activity = self.state.admit()?;
        let config = ProviderConfig::from_env()
            .map_err(OperationError::External)?
            .ok_or_else(|| {
                OperationError::InvalidInput("Assistant provider is not configured.".into())
            })?;
        let provider = OpenAiProvider::new(config).map_err(OperationError::External)?;
        Ok(ModelStream::new(provider, request))
    }
}
impl From<ProjectContext> for WorkflowCommands {
    fn from(state: ProjectContext) -> Self {
        Self { state }
    }
}
