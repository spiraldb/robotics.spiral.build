//! Spiral query contracts.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueryColumn {
    name: String,
    /// Arrow extension name when the column carries a semantic value such as
    /// a decoded image, tensor, media reference, or Blob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    semantic_type: Option<String>,
}

impl QueryColumn {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SpiralQuerySnapshot {
    columns: Vec<QueryColumn>,
    rows: Vec<serde_json::Map<String, serde_json::Value>>,
    truncated: bool,
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
        }
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
}

mod requests;
pub use requests::{EpisodeQueryRequest, QueryRequest};
