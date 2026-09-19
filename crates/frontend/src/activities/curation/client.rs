//! Stateless recipe documents and judged runs.
use crate::client::{
    http::UrlComponent,
    project::ProjectClient,
    transport::{CommandError, HttpRequest},
};
use spiral_robotics_common::curation::{CurationSpec, CurationSummary, run::CurationRun};

#[derive(Clone)]
pub(crate) struct CurationClient {
    project: ProjectClient,
}
impl CurationClient {
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
    pub(super) async fn interpret(
        &self,
        request: &spiral_robotics_common::curation::interpretation::InterpretationRequest,
    ) -> Result<spiral_robotics_common::curation::interpretation::RuleProposal, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/curations/interpret").json(request))
            .await
    }
    pub(super) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(super) async fn list(&self) -> Result<Vec<CurationSummary>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/curations"))
            .await
    }
    pub(super) async fn load(&self, id: &str) -> Result<CurationSpec, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                format!("/api/curations/{}", id.url_component()),
            ))
            .await
    }
    pub(super) async fn save(&self, curation: &CurationSpec) -> Result<CurationSpec, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/curations/save").json(curation))
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
