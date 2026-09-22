//! Explicit request messages shared by HTTP clients and the backend.
use super::*;

/// A read-only SQL statement and its explicit result row bound.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    statement: String,
    max_rows: usize,
}
impl QueryRequest {
    pub fn new(statement: String, max_rows: usize) -> Self {
        Self {
            statement,
            max_rows,
        }
    }
    pub fn into_parts(self) -> (String, usize) {
        (self.statement, self.max_rows)
    }
}

/// A read-only statement scoped to one exact dataset member: `$1` binds the
/// episode's UUID as an Arrow parameter, since Spiral compares member ids
/// only against bound UUIDs, never against text literals.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeQueryRequest {
    episode: crate::datasets::EpisodeSelection,
    statement: String,
    max_rows: usize,
}
impl EpisodeQueryRequest {
    pub fn new(
        episode: crate::datasets::EpisodeSelection,
        statement: String,
        max_rows: usize,
    ) -> Self {
        Self {
            episode,
            statement,
            max_rows,
        }
    }
    pub fn episode(&self) -> &crate::datasets::EpisodeSelection {
        &self.episode
    }
    pub fn statement(&self) -> &str {
        &self.statement
    }
    pub const fn max_rows(&self) -> usize {
        self.max_rows
    }
}
