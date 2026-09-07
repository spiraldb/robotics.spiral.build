//! Explicit request messages shared by HTTP clients and the backend.
use super::*;

/// A read-only SQL statement and its explicit result row bound.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
