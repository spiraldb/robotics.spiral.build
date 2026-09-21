//! Build documents over the project HTTP API. Stateless; the controller owns
//! the loaded document.
use crate::client::{
    http::UrlComponent,
    project::ProjectClient,
    transport::{CommandError, HttpRequest},
};
use spiral_robotics_common::build::{BuildDocument, BuildDocumentSummary};

#[derive(Clone)]
pub(crate) struct BuildClient {
    project: ProjectClient,
}
impl BuildClient {
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) async fn list(&self) -> Result<Vec<BuildDocumentSummary>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/build-documents"))
            .await
    }
    pub(crate) async fn load(&self, id: &str) -> Result<BuildDocument, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                format!("/api/build-documents/{}", id.url_component()),
            ))
            .await
    }
    /// Saves at the document's revision; the stored copy comes back one
    /// revision higher, or the call fails with `conflict`.
    pub(crate) async fn save(
        &self,
        document: &BuildDocument,
    ) -> Result<BuildDocument, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/build-documents/save").json(document))
            .await
    }
}
