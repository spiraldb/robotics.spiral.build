//! Workflow graphs and conversations are independently written JSON values,
//! keyed by workflow identity in one Spiral relation. Legacy rows stay intact.
//! Payload versions describe the encoding, independently of edit revisions.

use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};

use crate::db::ArrowColumns;
use crate::db::column as db;
use crate::db::{ManagedRelationSchema, Table};
use crate::db::{ProjectStore, StorageClock};

/// Maximum number of messages retained in one Assistant conversation.
pub const MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION: usize = 80;

/// Workflow document relations contributed to project bundles.
pub(crate) struct WorkflowBundleSection;

impl ManagedRelationSchema for WorkflowBundleSection {
    fn relations(&self) -> &'static [Table] {
        &[Table::WorkflowRuns]
    }

    fn managed_relation_schema(&self, table: Table) -> Option<Schema> {
        (table == Table::WorkflowRuns).then(|| WorkflowSchema::documents().as_ref().clone())
    }
}

/// Workflow document operations for one project.
pub(crate) struct WorkflowPersistence<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> WorkflowPersistence<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    pub(crate) async fn graphs(&self) -> Result<Vec<(String, WorkflowJson)>, String> {
        let mut rows = self.rows().await?;
        rows.sort_by(|left, right| {
            right
                .updated_ms
                .cmp(&left.updated_ms)
                .then_with(|| left.key.cmp(&right.key))
        });
        let current_ids = rows
            .iter()
            .filter_map(|row| row.key.strip_prefix("workflow:"))
            .collect::<std::collections::BTreeSet<_>>();
        Ok(rows
            .iter()
            .filter_map(|row| {
                let id = row.key.strip_prefix("workflow:").or_else(|| {
                    (row.key == "default" && !current_ids.contains("default")).then_some("default")
                })?;
                Some((id.to_owned(), row.document.clone()))
            })
            .collect())
    }

    pub(crate) async fn load_graph(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowJson>, String> {
        Ok(self
            .graphs()
            .await?
            .into_iter()
            .find(|(id, _)| id == workflow_id)
            .map(|(_, graph)| graph))
    }

    pub(crate) async fn save_graph(
        &self,
        workflow_id: &str,
        graph: &WorkflowJson,
    ) -> Result<(), String> {
        self.save(&format!("workflow:{workflow_id}"), graph).await
    }

    pub(crate) async fn load_assistant(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowJson>, String> {
        let rows = self.rows().await?;
        let key = format!("assistant:{workflow_id}");
        Ok(rows
            .iter()
            .find(|row| row.key == key)
            .or_else(|| {
                rows.iter()
                    .find(|row| workflow_id == "default" && row.key == "agent:default")
            })
            .map(|row| row.document.clone()))
    }

    pub(crate) async fn save_assistant(
        &self,
        workflow_id: &str,
        conversation: &WorkflowJson,
    ) -> Result<(), String> {
        self.save(&format!("assistant:{workflow_id}"), conversation)
            .await
    }

    async fn rows(&self) -> Result<Vec<WorkflowRow>, String> {
        if !self.project_store.is_configured() {
            return Ok(Vec::new());
        }
        let relation = Table::WorkflowRuns.qualified_name(self.project_store.table_prefix());
        self.project_store
            .connect()
            .await?
            .relation_rows(&relation, WorkflowSchema::decode)
            .await
    }

    /// Rewrite the document relation with one key replaced, preserving the
    /// other document in the same transaction.
    async fn save(&self, key: &str, document: &WorkflowJson) -> Result<(), String> {
        let relation = Table::WorkflowRuns.qualified_name(self.project_store.table_prefix());
        let mut connection = self.project_store.connect_exclusive().await?;
        connection.begin_transaction().await?;
        let mutation = async {
            let mut rows = connection
                .relation_rows(&relation, WorkflowSchema::decode)
                .await?;
            let updated_ms = StorageClock::unix_millis().max(
                rows.iter()
                    .map(|row| row.updated_ms)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| "Workflow timestamp overflowed.".to_owned())?,
            );
            rows.retain(|row| row.key != key);
            rows.push(WorkflowRow::new(
                key.to_owned(),
                document.clone(),
                updated_ms,
            ));
            connection
                .replace_batches(&relation, vec![WorkflowSchema::batch(&rows)?])
                .await
        }
        .await;
        connection
            .finish_transaction(mutation, "Workflow history")
            .await
    }
}

/// One persisted Workflow document and the version of its encoding.
///
/// Storage never parses `payload`; it only guarantees that the payload and the
/// version that describes it are written and read together.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowJson {
    version: u32,
    payload: String,
}

impl WorkflowJson {
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

/// One decoded row of the Workflow document relation.
struct WorkflowRow {
    key: String,
    document: WorkflowJson,
    updated_ms: i64,
}

impl WorkflowRow {
    const fn new(key: String, document: WorkflowJson, updated_ms: i64) -> Self {
        Self {
            key,
            document,
            updated_ms,
        }
    }
}

/// Arrow schema for the managed `workflow_runs` relation.
struct WorkflowSchema;

impl WorkflowSchema {
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
    fn batch(rows: &[WorkflowRow]) -> Result<RecordBatch, String> {
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
        RecordBatch::try_new(WorkflowSchema::documents(), columns)
            .map_err(|error| format!("building Workflow document batch: {error}"))
    }

    fn decode(batch: &RecordBatch, row: usize) -> Result<WorkflowRow, String> {
        let columns = ArrowColumns::new(batch, "Workflow document");
        Ok(WorkflowRow::new(
            columns.text(db::RUN_ID, row)?.to_owned(),
            WorkflowJson::new(
                columns.ordinal(db::DOCUMENT_VERSION, row)?,
                columns.text(db::SNAPSHOT_JSON, row)?.to_owned(),
            ),
            columns
                .optional_timestamp(db::UPDATED_MS, row)?
                .unwrap_or(0),
        ))
    }
}
