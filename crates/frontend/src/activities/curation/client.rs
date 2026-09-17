//! Stateless recipe documents and judged runs.
use crate::client::{
    http::UrlComponent,
    project::ProjectClient,
    transport::{CommandError, HttpRequest},
};
use spiral_robotics_common::curation::{CurationSpec, CurationSummary, run::CurationRun};
use spiral_robotics_common::datasets::DatasetSelection;

#[derive(Clone)]
pub(crate) struct CurationClient {
    project: ProjectClient,
}
impl CurationClient {
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) async fn list(&self) -> Result<Vec<CurationSummary>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/curations"))
            .await
    }
    pub(crate) async fn load(&self, id: &str) -> Result<CurationSpec, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                format!("/api/curations/{}", id.url_component()),
            ))
            .await
    }
    pub(crate) async fn save(&self, curation: &CurationSpec) -> Result<CurationSpec, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/curations/save").json(curation))
            .await
    }
    pub(crate) async fn runs(&self) -> Result<Vec<CurationRun>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/curation-runs"))
            .await
    }
    pub(crate) async fn run(&self, id: &str) -> Result<CurationRun, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                format!("/api/curation-runs/{}", id.url_component()),
            ))
            .await
    }
    pub(crate) async fn start(&self, curation: &CurationSpec) -> Result<CurationRun, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/curation-runs").json(curation))
            .await
    }
    /// The dataset's recorded camera topics, for suggesting a measurement's topic.
    pub(crate) async fn cameras(
        &self,
        dataset: &DatasetSelection,
    ) -> Result<serde_json::Value, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/cameras").json(dataset))
            .await
    }
}
