//! The saved curations: list them, load one by id, store one back, delete one.
//!
//! Build one inside the task that awaits it, as every other Studio client is
//! built, so the request names the mounting scope's project.

use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::curation::{CurationDeletion, CurationDocument, CurationSummary};

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
}
