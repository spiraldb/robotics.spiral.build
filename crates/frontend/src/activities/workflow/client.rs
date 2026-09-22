//! Planning and executing a workflow, and what Spiral can answer about. The
//! workflow document itself is listed, loaded, and saved by
//! `client::documents`.
use crate::client::{
    project::ProjectClient,
    transport::{CommandError, HttpRequest},
};
use spiral_robotics_common::relations::RelationCatalog;
use spiral_robotics_common::workflow::{
    WorkflowSpec,
    document::{WorkflowPlan, WorkflowRun},
};

#[derive(Clone)]
pub(crate) struct WorkflowClient {
    project: ProjectClient,
}
impl WorkflowClient {
    pub(super) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(super) async fn plan(&self, workflow: &WorkflowSpec) -> Result<WorkflowPlan, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/workflows/plan").json(workflow))
            .await
    }
    pub(super) async fn start(&self, workflow: &WorkflowSpec) -> Result<WorkflowRun, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/workflow-runs").json(workflow))
            .await
    }
    pub(super) async fn capabilities(
        &self,
    ) -> Result<spiral_robotics_common::relations::SpiralQuerySnapshot, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/models/capabilities"))
            .await
    }
    pub(super) async fn relations(&self) -> Result<RelationCatalog, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/relations"))
            .await
    }
}
