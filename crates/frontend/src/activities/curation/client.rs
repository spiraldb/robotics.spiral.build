//! Previewing a recipe and judging episodes with it. The recipe document
//! itself is listed, loaded, and saved by `client::documents`.
use crate::client::{
    http::UrlComponent,
    project::ProjectClient,
    transport::{CommandError, HttpRequest},
};
use spiral_robotics_common::curation::{CurationSpec, run::CurationRun};

#[derive(Clone)]
pub(crate) struct CurationClient {
    project: ProjectClient,
}
impl CurationClient {
    pub(super) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(super) async fn preview(
        &self,
        curation: &CurationSpec,
    ) -> Result<spiral_robotics_common::curation::run::CurationOutcome, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/curations/preview").json(curation))
            .await
    }
    pub(super) async fn score_episode(
        &self,
        request: &spiral_robotics_common::curation::progress::ProgressScoreRequest,
    ) -> Result<spiral_robotics_common::curation::progress::ProgressScoreResult, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/curations/progress-score").json(request))
            .await
    }
    pub(super) async fn runs(&self) -> Result<Vec<CurationRun>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/curation-runs"))
            .await
    }
    pub(super) async fn run(&self, id: &str) -> Result<CurationRun, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                format!("/api/curation-runs/{}", id.url_component()),
            ))
            .await
    }
    pub(super) async fn start(&self, curation: &CurationSpec) -> Result<CurationRun, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/curation-runs").json(curation))
            .await
    }
}
