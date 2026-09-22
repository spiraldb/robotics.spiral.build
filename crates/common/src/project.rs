//! The project a request acts on, named by the browser.

use serde::{Deserialize, Serialize};

/// One Spiral project: the endpoint it lives on and its ID. Local Studio data
/// is stored under the same pair. The browser chooses the project and names it
/// on every project request; the backend keeps no "current" project. An empty
/// ID names the endpoint before a project is chosen, which Studio-only
/// features such as robot setup still accept.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectKey {
    endpoint: String,
    project_id: String,
}

impl ProjectKey {
    pub const ENDPOINT_HEADER: &str = "X-Spiral-Endpoint";
    pub const PROJECT_HEADER: &str = "X-Spiral-Project";

    pub fn new(endpoint: impl Into<String>, project_id: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            project_id: project_id.into(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Whether a Spiral project is chosen, so catalog features can run.
    pub fn is_chosen(&self) -> bool {
        !self.project_id.trim().is_empty()
    }
}
