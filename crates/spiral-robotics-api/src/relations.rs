//! Spiral query and user-visible relation contracts.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SpiralQueryRequest {
    sql: String,
    #[serde(default = "SpiralQueryRequest::default_max_rows")]
    max_rows: usize,
}

impl SpiralQueryRequest {
    const fn default_max_rows() -> usize {
        100
    }

    pub fn new(sql: impl Into<String>, max_rows: usize) -> Self {
        Self {
            sql: sql.into(),
            max_rows,
        }
    }

    pub fn sql(&self) -> &str {
        &self.sql
    }

    pub const fn max_rows(&self) -> usize {
        self.max_rows
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueryColumn {
    name: String,
    data_type: String,
    nullable: bool,
    /// Arrow extension name when the column carries a semantic value such as
    /// a decoded image, tensor, media reference, or Blob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    semantic_type: Option<String>,
}

impl QueryColumn {
    pub fn new(name: impl Into<String>, data_type: impl Into<String>, nullable: bool) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable,
            semantic_type: None,
        }
    }
    pub fn with_semantic_type(mut self, semantic_type: Option<String>) -> Self {
        self.semantic_type = semantic_type;
        self
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn data_type(&self) -> &str {
        &self.data_type
    }
    pub const fn nullable(&self) -> bool {
        self.nullable
    }
    pub fn semantic_type(&self) -> Option<&str> {
        self.semantic_type.as_deref()
    }
}

/// Schema-only catalog used by relation consumers such as Data and Workflow.
/// Scope is attached by the activity manager so an asynchronous catalog read
/// cannot be reconciled into a different Spiral project.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RelationCatalog {
    #[serde(default)]
    scope_id: String,
    relations: Vec<CatalogRelation>,
    complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limitation: Option<String>,
}

impl RelationCatalog {
    pub fn new(relations: Vec<CatalogRelation>, complete: bool) -> Self {
        Self {
            scope_id: String::new(),
            relations,
            complete,
            limitation: None,
        }
    }

    pub fn with_scope(mut self, scope_id: impl Into<String>) -> Self {
        self.scope_id = scope_id.into();
        self
    }

    pub fn with_limitation(mut self, limitation: Option<String>) -> Self {
        self.limitation = limitation;
        self
    }

    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    pub fn relations(&self) -> &[CatalogRelation] {
        &self.relations
    }

    pub const fn complete(&self) -> bool {
        self.complete
    }

    pub fn limitation(&self) -> Option<&str> {
        self.limitation.as_deref()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CatalogRelation {
    name: String,
    relation_type: String,
    columns: Vec<QueryColumn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    row_count: Option<u64>,
}

impl CatalogRelation {
    pub fn new(
        name: impl Into<String>,
        relation_type: impl Into<String>,
        columns: Vec<QueryColumn>,
    ) -> Self {
        Self {
            name: name.into(),
            relation_type: relation_type.into(),
            columns,
            row_count: None,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn relation_type(&self) -> &str {
        &self.relation_type
    }

    pub fn columns(&self) -> &[QueryColumn] {
        &self.columns
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SpiralQueryResponse {
    columns: Vec<QueryColumn>,
    rows: Vec<serde_json::Map<String, serde_json::Value>>,
    truncated: bool,
    elapsed_ms: u64,
}

impl SpiralQueryResponse {
    pub fn new(
        columns: Vec<QueryColumn>,
        rows: Vec<serde_json::Map<String, serde_json::Value>>,
        truncated: bool,
    ) -> Self {
        Self {
            columns,
            rows,
            truncated,
            elapsed_ms: 0,
        }
    }
    pub const fn with_elapsed_ms(mut self, elapsed_ms: u64) -> Self {
        self.elapsed_ms = elapsed_ms;
        self
    }
    pub fn columns(&self) -> &[QueryColumn] {
        &self.columns
    }
    pub fn rows(&self) -> &[serde_json::Map<String, serde_json::Value>] {
        &self.rows
    }
    pub fn into_rows(self) -> Vec<serde_json::Map<String, serde_json::Value>> {
        self.rows
    }
    pub const fn truncated(&self) -> bool {
        self.truncated
    }
    pub const fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }
}

/// User-facing relation catalog for the active Spiral project.
///
/// This intentionally excludes Studio's settings, run snapshots, derived
/// caches, and managed media plumbing. The Data activity uses it to explain
/// the project's data rather than Studio's persistence implementation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectDataRelationsResponse {
    relations: Vec<ProjectDataRelation>,
    complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limitation: Option<String>,
}

impl ProjectDataRelationsResponse {
    pub fn new(relations: Vec<ProjectDataRelation>, complete: bool) -> Self {
        Self {
            relations,
            complete,
            limitation: None,
        }
    }
    pub fn with_limitation(mut self, limitation: Option<String>) -> Self {
        self.limitation = limitation;
        self
    }
    pub fn relations(&self) -> &[ProjectDataRelation] {
        &self.relations
    }
    pub const fn complete(&self) -> bool {
        self.complete
    }
    pub fn limitation(&self) -> Option<&str> {
        self.limitation.as_deref()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectDataRelation {
    name: String,
    relation_type: String,
    columns: Vec<QueryColumn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    row_count: Option<u64>,
}

impl ProjectDataRelation {
    pub fn new(
        name: impl Into<String>,
        relation_type: impl Into<String>,
        columns: Vec<QueryColumn>,
    ) -> Self {
        Self {
            name: name.into(),
            relation_type: relation_type.into(),
            columns,
            row_count: None,
        }
    }
    pub const fn with_row_count(mut self, row_count: Option<u64>) -> Self {
        self.row_count = row_count;
        self
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn columns(&self) -> &[QueryColumn] {
        &self.columns
    }
    pub const fn row_count(&self) -> Option<u64> {
        self.row_count
    }
}
