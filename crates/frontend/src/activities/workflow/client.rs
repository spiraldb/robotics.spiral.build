//! Stateless document, execution, and model capabilities.
use crate::client::{
    http::UrlComponent,
    project::ProjectClient,
    transport::{CommandError, HttpRequest},
};
use spiral_robotics_common::relations::RelationCatalog;
use spiral_robotics_common::workflow::{
    WorkflowSpec, WorkflowSummary,
    document::{WorkflowDocument, WorkflowPlan, WorkflowRun},
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
    pub(super) async fn list(&self) -> Result<Vec<WorkflowSummary>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/workflows"))
            .await
    }
    pub(super) async fn load(&self, id: &str) -> Result<WorkflowDocument, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                format!("/api/workflows/{}", id.url_component()),
            ))
            .await
    }
    pub(super) async fn save(
        &self,
        document: &WorkflowDocument,
    ) -> Result<WorkflowDocument, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/workflows/save").json(document))
            .await
    }
    pub(super) async fn plan(&self, workflow: &WorkflowSpec) -> Result<WorkflowPlan, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/workflows/plan").json(workflow))
            .await
    }
    pub(super) async fn runs(&self) -> Result<Vec<WorkflowRun>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/workflow-runs"))
            .await
    }
    pub(super) async fn run(&self, id: &str) -> Result<WorkflowRun, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                format!("/api/workflow-runs/{}", id.url_component()),
            ))
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
