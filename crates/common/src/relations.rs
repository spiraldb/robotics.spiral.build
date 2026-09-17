//! Spiral query and user-visible relation contracts.

use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
    /// Encoded previews produced by the shared native query result adapter.
    pub fn image_data_url<'a>(&self, value: Option<&'a serde_json::Value>) -> Option<&'a str> {
        matches!(
            self.semantic_type(),
            Some("spql.image.Image" | "arrow.variable_shape_tensor")
        )
        .then(|| value?.as_str())?
        .filter(|value| {
            value.starts_with("data:image/jpeg;base64,")
                || value.starts_with("data:image/png;base64,")
        })
    }
    pub fn semantic_type(&self) -> Option<&str> {
        self.semantic_type.as_deref()
    }
}

/// Schema-only catalog used by relation consumers such as Data and Workflow.
/// Scope is attached by the activity manager so an asynchronous catalog read
/// cannot be reconciled into a different Spiral project.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

    pub const fn with_row_count(mut self, row_count: Option<u64>) -> Self {
        self.row_count = row_count;
        self
    }

    pub const fn row_count(&self) -> Option<u64> {
        self.row_count
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SpiralQuerySnapshot {
    columns: Vec<QueryColumn>,
    rows: Vec<serde_json::Map<String, serde_json::Value>>,
    truncated: bool,
    elapsed_ms: u64,
}

impl SpiralQuerySnapshot {
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

mod requests;
pub use requests::{EpisodeQueryRequest, QueryRequest};
