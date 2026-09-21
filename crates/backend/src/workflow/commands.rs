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
    pub(super) fn runs(&self) -> Result<Vec<WorkflowRun>, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.workflow().runs())
    }
    pub(super) fn run(&self, id: &str) -> Result<WorkflowRun, BackendCommandError> {
        let _activity = self.state.admit()?;
        self.state.workflow().run(id).map_err(Into::into)
    }
    pub(super) fn start(&self, workflow: WorkflowSpec) -> Result<WorkflowRun, BackendCommandError> {
        let _activity = self.state.admit()?;
        let lease = self.state.catalog_lease();
        let _reservation = lease
            .try_reserve(CatalogLeaseOwner::WorkflowStart)
            .ok_or_else(|| {
                BackendCommandError::conflict(
                    "Wait for recording or catalog changes before running a workflow.",
                )
            })?;
        self.state.workflow().start(workflow).map_err(Into::into)
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
    pub(super) fn model_status(&self) -> Result<WorkflowAgentReadiness, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(match ProviderConfig::from_env() {
            Ok(Some(_)) => WorkflowAgentReadiness::ready("Ready to help author this workflow."),
            Ok(None) => WorkflowAgentReadiness::error(
                "Configure LLM_API_KEY on the backend to enable the assistant.",
            ),
            Err(error) => WorkflowAgentReadiness::error(error),
        })
    }
    pub(super) fn complete(
        &self,
        request: ModelRequest,
    ) -> Result<ModelStream, BackendCommandError> {
        let _activity = self.state.admit()?;
        let config = ProviderConfig::from_env()
            .map_err(OperationError::External)?
            .ok_or_else(|| {
                OperationError::InvalidInput("Assistant provider is not configured.".into())
            })?;
        let config = request
            .model()
            .map_or(config.clone(), |model| config.for_model(model));
        let provider = OpenAiProvider::new(config).map_err(OperationError::External)?;
        Ok(ModelStream::new(provider, request))
    }
    pub(super) async fn decide(
        &self,
        request: spiral_robotics_common::workflow::document::DecisionRequest,
    ) -> Result<spiral_robotics_common::workflow::document::DecisionResponse, BackendCommandError>
    {
        let _activity = self.state.admit()?;
        OpenAiProvider::decide(request)
            .await
            .map_err(OperationError::External)
            .map_err(Into::into)
    }
}
impl From<ProjectContext> for WorkflowCommands {
    fn from(state: ProjectContext) -> Self {
        Self { state }
    }
}
