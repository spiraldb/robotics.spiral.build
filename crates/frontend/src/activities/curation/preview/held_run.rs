//! The run the backend holds for one preview pass: started with every query
//! at once, polled until it settles, and read one answer at a time. Each
//! request returns at once, so a pass never holds one of the page's few
//! connections to the backend while a model answers.
use crate::client::CommandError;
use crate::client::project::ProjectClient;
use crate::client::transport::HttpRequest;
use spiral_robotics_common::curation::{PreviewQuery, PreviewRunStatus};
use spiral_robotics_common::relations::SpiralQuerySnapshot;

/// A preview run the backend holds, as this page reaches it.
pub(super) struct HeldRun {
    project: ProjectClient,
    id: String,
}

impl HeldRun {
    /// Hand `queries` to the backend, which starts on them at once.
    pub(super) async fn start(
        queries: &[PreviewQuery],
    ) -> Result<(Self, PreviewRunStatus), CommandError> {
        let project = ProjectClient::current();
        let status = project
            .request::<PreviewRunStatus>(
                HttpRequest::new("POST", "/api/curation-previews").json(&queries),
            )
            .await?;
        let run = Self {
            project,
            id: status.id().to_owned(),
        };
        Ok((run, status))
    }

    /// Where the run stands. Asking is also what keeps it wanted.
    pub(super) async fn status(&self) -> Result<PreviewRunStatus, CommandError> {
        self.project
            .request(HttpRequest::new("GET", self.url("")))
            .await
    }

    /// Start none of the queries still waiting; those running land.
    pub(super) async fn stop(&self) -> Result<(), CommandError> {
        self.project
            .request(HttpRequest::new("POST", self.url("/stop")))
            .await
    }

    /// What query `query`, asked for as rows, answered.
    pub(super) async fn rows(&self, query: usize) -> Result<SpiralQuerySnapshot, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                self.url(&format!("/answers/{query}")),
            ))
            .await
    }

    /// What query `query`, asked for as an Arrow stream, answered.
    pub(super) async fn stream(&self, query: usize) -> Result<Vec<u8>, CommandError> {
        self.project
            .request_bytes(HttpRequest::new(
                "GET",
                self.url(&format!("/answers/{query}")),
            ))
            .await
    }

    fn url(&self, rest: &str) -> String {
        format!("/api/curation-previews/{}{rest}", self.id)
    }
}
