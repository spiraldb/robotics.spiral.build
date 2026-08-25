//! Spiral query and user-visible relation contracts.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SpiralQueryRequest {
    pub sql: String,
    #[serde(default = "default_query_rows")]
    pub max_rows: usize,
}

fn default_query_rows() -> usize {
    100
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueryColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    /// Arrow extension name when the column carries a semantic value such as
    /// a decoded image, tensor, media reference, or Blob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_type: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SpiralQueryResponse {
    pub columns: Vec<QueryColumn>,
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// User-facing relation catalog for the active Spiral project.
///
/// This intentionally excludes Studio's settings, run snapshots, derived
/// caches, and managed media plumbing. The Data activity uses it to explain
/// the project's data rather than Studio's persistence implementation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectDataRelationsResponse {
    pub relations: Vec<ProjectDataRelation>,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limitation: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectDataRelation {
    pub name: String,
    pub relation_type: String,
    pub columns: Vec<QueryColumn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_count: Option<u64>,
}
