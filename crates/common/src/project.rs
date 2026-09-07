//! Project lifetime identities shared by commands and fetched state.

use serde::{Deserialize, Serialize};

/// One activated project lifetime. Reopening the same project creates a new ID.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectSessionInfo {
    id: String,
    project_id: String,
}

impl ProjectSessionInfo {
    pub fn new(id: impl Into<String>, project_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            project_id: project_id.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }
}

/// Identity and readiness are fetched together from one captured project.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectStatus {
    session: ProjectSessionInfo,
    readiness: crate::status::BackendReadiness,
}

impl ProjectStatus {
    pub fn new(session: ProjectSessionInfo, readiness: crate::status::BackendReadiness) -> Self {
        Self { session, readiness }
    }
    pub fn session(&self) -> &ProjectSessionInfo {
        &self.session
    }
    pub fn readiness(&self) -> &crate::status::BackendReadiness {
        &self.readiness
    }
}
