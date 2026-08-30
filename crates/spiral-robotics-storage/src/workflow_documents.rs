//! Storage-owned, explicitly versioned Workflow documents.
//!
//! The Workflow graph and the Assistant history are still evolving, so they
//! are not yet decomposed into node and edge relations. What storage does own
//! is the *persisted form*: every row carries the version of the document
//! encoding under its key, so a payload is never interpreted as "whatever the
//! current Rust type happens to serialize to".
//!
//! A reader that meets a document version it does not implement must refuse it;
//! it must never overwrite the operator's document with an empty one.

use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use sha2::{Digest as _, Sha256};

use crate::db::ArrowColumns;
use crate::db::column as db;
use crate::db::{ManagedRelationSchema, Table};
use crate::db::{ProjectStore, StorageClock};

/// Workflow document relations contributed to project bundles.
pub(crate) struct WorkflowDocumentBundleSection;

impl ManagedRelationSchema for WorkflowDocumentBundleSection {
    fn relations(&self) -> &'static [Table] {
        &[Table::WorkflowRuns]
    }

    fn managed_relation_schema(&self, table: Table) -> Option<Schema> {
        (table == Table::WorkflowRuns).then(|| WorkflowDocumentSchema::documents().as_ref().clone())
    }
}

/// Workflow document operations for one project.
pub(crate) struct WorkflowDocumentPersistence<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> WorkflowDocumentPersistence<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    pub(crate) async fn load_graph(&self) -> Result<Option<StoredWorkflowDocument>, String> {
        self.load(WorkflowDocumentKey::Graph).await
    }

    pub(crate) async fn save_graph(&self, document: &StoredWorkflowDocument) -> Result<(), String> {
        self.save(WorkflowDocumentWrite::new(
            WorkflowDocumentKey::Graph,
            document,
        ))
        .await
    }

    pub(crate) async fn load_assistant(&self) -> Result<Option<StoredWorkflowDocument>, String> {
        self.load(WorkflowDocumentKey::Assistant).await
    }

    pub(crate) async fn save_assistant(
        &self,
        document: &StoredWorkflowDocument,
    ) -> Result<(), String> {
        self.save(WorkflowDocumentWrite::new(
            WorkflowDocumentKey::Assistant,
            document,
        ))
        .await
    }

    /// Replace an unreadable Assistant document only when its durable payload
    /// is still the one the caller inspected.
    pub(crate) async fn reset_assistant(
        &self,
        expected_payload: &str,
        document: &StoredWorkflowDocument,
    ) -> Result<(), String> {
        self.save(
            WorkflowDocumentWrite::new(WorkflowDocumentKey::Assistant, document)
                .replacing(expected_payload),
        )
        .await
    }

    async fn load(
        &self,
        key: WorkflowDocumentKey,
    ) -> Result<Option<StoredWorkflowDocument>, String> {
        if !self.project_store.is_configured() {
            return Ok(None);
        }
        let relation = Table::WorkflowRuns.qualified_name(self.project_store.namespace());
        let mut connection = self.project_store.connect().await?;
        Ok(connection
            .relation_rows(&relation, WorkflowDocumentSchema::decode)
            .await?
            .into_iter()
            .find(|row| row.matches(key))
            .map(WorkflowDocumentRow::into_document))
    }

    /// Rewrite the document relation with one key replaced, preserving the
    /// other document in the same transaction.
    async fn save(&self, write: WorkflowDocumentWrite<'_>) -> Result<(), String> {
        let relation = Table::WorkflowRuns.qualified_name(self.project_store.namespace());
        let mut connection = self.project_store.connect_exclusive().await?;
        connection.begin_transaction().await?;
        let mutation = async {
            let existing = WorkflowDocumentReplacement::new(
                connection
                    .relation_rows(&relation, WorkflowDocumentSchema::decode)
                    .await?,
            );
            if let Some(expected) = write.expected_payload {
                let matches = existing.current(write.key).is_some_and(|current| {
                    Sha256::digest(current.payload().as_bytes()).as_slice()
                        == Sha256::digest(expected.as_bytes()).as_slice()
                });
                if !matches {
                    return Err(
                        "Assistant history changed before reset. Reload the current project."
                            .to_owned(),
                    );
                }
            }
            let rows = existing.replaced(WorkflowDocumentRow::new(
                write.key.persisted().to_owned(),
                write.document.clone(),
                StorageClock::unix_millis(),
            ));
            connection
                .replace_batches(&relation, vec![WorkflowDocumentSchema::batch(&rows)?])
                .await
        }
        .await;
        connection
            .finish_transaction(mutation, "Workflow history")
            .await
    }
}

/// One Workflow document write, optionally guarded by the payload the caller
/// expects to still be durable.
struct WorkflowDocumentWrite<'a> {
    key: WorkflowDocumentKey,
    document: &'a StoredWorkflowDocument,
    expected_payload: Option<&'a str>,
}

impl<'a> WorkflowDocumentWrite<'a> {
    const fn new(key: WorkflowDocumentKey, document: &'a StoredWorkflowDocument) -> Self {
        Self {
            key,
            document,
            expected_payload: None,
        }
    }

    const fn replacing(mut self, expected_payload: &'a str) -> Self {
        self.expected_payload = Some(expected_payload);
        self
    }
}

/// One persisted Workflow document and the version of its encoding.
///
/// Storage never parses `payload`; it only guarantees that the payload and the
/// version that describes it are written and read together.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredWorkflowDocument {
    version: u32,
    payload: String,
}

impl StoredWorkflowDocument {
    pub const fn new(version: u32, payload: String) -> Self {
        Self { version, payload }
    }

    pub const fn version(&self) -> u32 {
        self.version
    }

    pub fn payload(&self) -> &str {
        &self.payload
    }
}

/// The closed set of documents Studio persists in the Workflow relation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkflowDocumentKey {
    /// The saved graph together with its latest run observation.
    Graph,
    /// The Assistant conversation history.
    Assistant,
}

impl WorkflowDocumentKey {
    const fn persisted(self) -> &'static str {
        match self {
            Self::Graph => "default",
            Self::Assistant => "agent:default",
        }
    }
}

/// One decoded row of the Workflow document relation.
struct WorkflowDocumentRow {
    key: String,
    document: StoredWorkflowDocument,
    updated_ms: i64,
}

impl WorkflowDocumentRow {
    const fn new(key: String, document: StoredWorkflowDocument, updated_ms: i64) -> Self {
        Self {
            key,
            document,
            updated_ms,
        }
    }

    fn matches(&self, key: WorkflowDocumentKey) -> bool {
        self.key == key.persisted()
    }

    const fn document(&self) -> &StoredWorkflowDocument {
        &self.document
    }

    fn into_document(self) -> StoredWorkflowDocument {
        self.document
    }
}

/// Arrow schema for the managed `workflow_runs` relation.
struct WorkflowDocumentSchema;

impl WorkflowDocumentSchema {
    fn documents() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(db::RUN_ID, DataType::Utf8, false),
            Field::new(db::DOCUMENT_VERSION, DataType::UInt32, false),
            Field::new(db::SNAPSHOT_JSON, DataType::Utf8, false),
            Field::new(db::UPDATED_MS, DataType::Int64, false),
        ]))
    }

    /// Encode every document in the relation as one replacement batch. Studio
    /// keeps at most one row per key, so the whole relation is small enough to
    /// rewrite atomically on every save.
    fn batch(rows: &[WorkflowDocumentRow]) -> Result<RecordBatch, String> {
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.key.as_str()),
            )),
            Arc::new(UInt32Array::from_iter_values(
                rows.iter().map(|row| row.document.version()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.document.payload()),
            )),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|row| row.updated_ms),
            )),
        ];
        RecordBatch::try_new(WorkflowDocumentSchema::documents(), columns)
            .map_err(|error| format!("building Workflow document batch: {error}"))
    }

    fn decode(batch: &RecordBatch, row: usize) -> Result<WorkflowDocumentRow, String> {
        let columns = ArrowColumns::new(batch, "Workflow document");
        Ok(WorkflowDocumentRow::new(
            columns.text(db::RUN_ID, row)?.to_owned(),
            StoredWorkflowDocument::new(
                columns.ordinal(db::DOCUMENT_VERSION, row)?,
                columns.text(db::SNAPSHOT_JSON, row)?.to_owned(),
            ),
            columns
                .optional_timestamp(db::UPDATED_MS, row)?
                .unwrap_or(0),
        ))
    }
}

/// Replace one key's document inside the relation's other rows.
struct WorkflowDocumentReplacement {
    rows: Vec<WorkflowDocumentRow>,
}

impl WorkflowDocumentReplacement {
    const fn new(rows: Vec<WorkflowDocumentRow>) -> Self {
        Self { rows }
    }

    fn current(&self, key: WorkflowDocumentKey) -> Option<&StoredWorkflowDocument> {
        self.rows
            .iter()
            .find(|row| row.matches(key))
            .map(WorkflowDocumentRow::document)
    }

    /// Swap in one row, keeping every other document key untouched.
    fn replaced(mut self, row: WorkflowDocumentRow) -> Vec<WorkflowDocumentRow> {
        self.rows.retain(|existing| existing.key != row.key);
        self.rows.push(row);
        self.rows.sort_by(|left, right| left.key.cmp(&right.key));
        self.rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<WorkflowDocumentRow> {
        vec![
            WorkflowDocumentRow::new(
                WorkflowDocumentKey::Graph.persisted().to_owned(),
                StoredWorkflowDocument::new(1, r#"{"workflow":{}}"#.to_owned()),
                10,
            ),
            WorkflowDocumentRow::new(
                WorkflowDocumentKey::Assistant.persisted().to_owned(),
                StoredWorkflowDocument::new(2, r#"{"conversations":[]}"#.to_owned()),
                20,
            ),
        ]
    }

    #[test]
    fn documents_round_trip_with_their_declared_version() {
        let batch = WorkflowDocumentSchema::batch(&rows()).unwrap();
        assert_eq!(batch.num_rows(), 2);
        let decoded = (0..batch.num_rows())
            .map(|row| WorkflowDocumentSchema::decode(&batch, row).unwrap())
            .collect::<Vec<_>>();
        let replacement = WorkflowDocumentReplacement::new(decoded);
        assert_eq!(
            replacement.current(WorkflowDocumentKey::Assistant),
            Some(&StoredWorkflowDocument::new(
                2,
                r#"{"conversations":[]}"#.to_owned()
            ))
        );
    }

    #[test]
    fn replacing_one_key_preserves_every_other_document() {
        let replaced = WorkflowDocumentReplacement::new(rows()).replaced(WorkflowDocumentRow::new(
            WorkflowDocumentKey::Graph.persisted().to_owned(),
            StoredWorkflowDocument::new(1, r#"{"workflow":{"revision":2}}"#.to_owned()),
            30,
        ));
        assert_eq!(replaced.len(), 2);
        let replacement = WorkflowDocumentReplacement::new(replaced);
        assert_eq!(
            replacement
                .current(WorkflowDocumentKey::Graph)
                .map(StoredWorkflowDocument::payload),
            Some(r#"{"workflow":{"revision":2}}"#)
        );
        assert_eq!(
            replacement
                .current(WorkflowDocumentKey::Assistant)
                .map(StoredWorkflowDocument::payload),
            Some(r#"{"conversations":[]}"#)
        );
    }
}
