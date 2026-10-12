//! The saved curations: list them, load one by id, store one back, delete one;
//! which episodes a column of their answers already holds; and the fill runs
//! that compute answers-table columns, which curations and the dataset health
//! check both start.
//!
//! Build one inside the task that awaits it, as every other Studio client is
//! built, so the request names the mounting scope's project.

use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::curation::{
    CurationDeletion, CurationDocument, CurationSummary, FillRequest, FillStatus,
    FilledEpisodesRequest, PublishedView,
};
use spiral_robotics_common::datasets::EpisodeId;
use std::collections::BTreeSet;

pub(super) struct CurationClient(ProjectClient);

impl CurationClient {
    pub(super) fn current() -> Self {
        Self(ProjectClient::current())
    }

    /// Every saved curation, in the order Studio stores them.
    pub(super) async fn list(&self) -> Result<Vec<CurationSummary>, CommandError> {
        self.0
            .request(HttpRequest::new("GET", "/api/curations"))
            .await
    }

    pub(super) async fn load(&self, id: &str) -> Result<CurationDocument, CommandError> {
        self.0
            .request(HttpRequest::new(
                "GET",
                format!("/api/curations/{}", id.url_component()),
            ))
            .await
    }

    /// Stores at the document's own revision. The stored copy comes back one
    /// revision higher, or the call fails with `conflict`.
    pub(super) async fn save(
        &self,
        document: &CurationDocument,
    ) -> Result<CurationDocument, CommandError> {
        self.0
            .request(HttpRequest::new("POST", "/api/curations/save").json(document))
            .await
    }

    /// Deletes `id` as stored at `revision`, or fails with `conflict` when a
    /// later save moved it on.
    pub(super) async fn delete(&self, id: &str, revision: u64) -> Result<(), CommandError> {
        self.0
            .request(
                HttpRequest::new("POST", "/api/curations/delete")
                    .json(&CurationDeletion::new(id, revision)),
            )
            .await
    }

    /// The episodes `request`'s column holds an answer for, failed ones
    /// included.
    pub(super) async fn filled(
        &self,
        request: &FilledEpisodesRequest,
    ) -> Result<BTreeSet<EpisodeId>, CommandError> {
        self.0
            .request(HttpRequest::new("POST", "/api/curation-answers/filled").json(request))
            .await
    }

    /// Every view the project's curations published, with the statement each
    /// is read through.
    pub(super) async fn published_views(&self) -> Result<Vec<PublishedView>, CommandError> {
        self.0
            .request(HttpRequest::new("GET", "/api/curated-views"))
            .await
    }
}

/// The backend's fill runs: each computes columns of a dataset's answers
/// table, and a curation's may publish views once they are written.
pub(in crate::activities) struct FillRuns(ProjectClient);

impl FillRuns {
    pub(in crate::activities) fn current() -> Self {
        Self(ProjectClient::current())
    }

    /// Start filling what `request` asks for; the backend works on it after
    /// this returns.
    pub(in crate::activities) async fn start(
        &self,
        request: &FillRequest,
    ) -> Result<FillStatus, CommandError> {
        self.0
            .request(HttpRequest::new("POST", "/api/curation-fills").json(request))
            .await
    }

    /// Where run `id` stands.
    pub(in crate::activities) async fn status(&self, id: &str) -> Result<FillStatus, CommandError> {
        self.0
            .request(HttpRequest::new(
                "GET",
                format!("/api/curation-fills/{}", id.url_component()),
            ))
            .await
    }

    /// Ask run `id` to start nothing more; what is in flight is written.
    pub(in crate::activities) async fn stop(&self, id: &str) -> Result<(), CommandError> {
        self.0
            .request(HttpRequest::new(
                "POST",
                format!("/api/curation-fills/{}/stop", id.url_component()),
            ))
            .await
    }

    /// Every run that publishes a view.
    pub(super) async fn publications(&self) -> Result<Vec<FillStatus>, CommandError> {
        self.0
            .request(HttpRequest::new("GET", "/api/curation-fills"))
            .await
    }
}
