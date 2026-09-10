//! One row per workflow. The list reads metadata; graph and conversation JSON
//! are loaded by identity and saved independently in the same transaction.

mod cameras;

use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use spiral_robotics_common::workflow::{WorkflowSpec, WorkflowSummary};

use crate::db::{ArrowColumns, ManagedRelationSchema, ProjectStore, StorageClock, Table};

pub const MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION: usize = 80;

pub(crate) struct WorkflowBundleSection;

impl ManagedRelationSchema for WorkflowBundleSection {
    fn relations(&self) -> &'static [Table] {
        &[Table::Workflows]
    }

    fn managed_relation_schema(&self, table: Table) -> Option<Schema> {
        (table == Table::Workflows).then(|| WorkflowSchema::schema().as_ref().clone())
    }
}

pub(crate) struct WorkflowPersistence<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> WorkflowPersistence<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    pub(crate) async fn list(&self) -> Result<Vec<WorkflowSummary>, String> {
        if !self.project_store.is_configured() {
            return Ok(Vec::new());
        }
        let relation = Table::Workflows.qualified_name(self.project_store.table_prefix());
        let batches = self
            .project_store
            .connect()
            .await?
            .read_batches(&format!(
                "SELECT workflow_id, name FROM {relation} ORDER BY updated_ms DESC, workflow_id"
            ))
            .await?;
        let mut workflows = Vec::new();
        for batch in batches {
            let columns = ArrowColumns::new(&batch, "Workflow list");
            for row in 0..batch.num_rows() {
                workflows.push(WorkflowSummary::new(
                    columns.text("workflow_id", row)?,
                    columns.text("name", row)?,
                ));
            }
        }
        Ok(workflows)
    }

    pub(crate) async fn load_graph(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowJson>, String> {
        self.load_json(workflow_id, "graph").await
    }

    pub(crate) async fn load_assistant(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowJson>, String> {
        self.load_json(workflow_id, "conversation").await
    }

    async fn load_json(
        &self,
        workflow_id: &str,
        field: &str,
    ) -> Result<Option<WorkflowJson>, String> {
        if !self.project_store.is_configured() {
            return Ok(None);
        }
        let relation = Table::Workflows.qualified_name(self.project_store.table_prefix());
        let id = workflow_id.replace('\'', "''");
        let batches = self
            .project_store
            .connect()
            .await?
            .read_batches(&format!(
                "SELECT {field}_version, {field}_json FROM {relation} WHERE workflow_id = '{id}'"
            ))
            .await?;
        let mut found = None;
        let mut count = 0;
        for batch in batches {
            for row in 0..batch.num_rows() {
                count += 1;
                if count > 1 {
                    return Err("Duplicate workflow identity.".to_owned());
                }
                found = WorkflowSchema::json(&ArrowColumns::new(&batch, "Workflow"), row, field)?;
            }
        }
        Ok(found)
    }

    pub(crate) async fn save_graph(
        &self,
        workflow: &WorkflowSpec,
        graph: &WorkflowJson,
    ) -> Result<(), String> {
        self.update(|rows| {
            let updated_ms = StorageClock::unix_millis().max(
                rows.iter()
                    .map(|row| row.updated_ms)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| "Workflow timestamp overflowed.".to_owned())?,
            );
            if let Some(row) = rows.iter_mut().find(|row| row.id == workflow.id()) {
                row.name = workflow.name().to_owned();
                row.graph = graph.clone();
                row.updated_ms = updated_ms;
            } else {
                rows.push(WorkflowRow::new(workflow, graph.clone(), updated_ms));
            }
            Ok(())
        })
        .await
    }

    pub(crate) async fn save_assistant(
        &self,
        workflow_id: &str,
        conversation: &WorkflowJson,
    ) -> Result<(), String> {
        self.update(|rows| {
            let row = rows
                .iter_mut()
                .find(|row| row.id == workflow_id)
                .ok_or_else(|| "Save the workflow before its conversation.".to_owned())?;
            row.conversation = Some(conversation.clone());
            Ok(())
        })
        .await
    }

    async fn update(
        &self,
        change: impl FnOnce(&mut Vec<WorkflowRow>) -> Result<(), String>,
    ) -> Result<(), String> {
        let relation = Table::Workflows.qualified_name(self.project_store.table_prefix());
        let mut connection = self.project_store.connect_exclusive().await?;
        connection.begin_transaction().await?;
        let mutation = async {
            let mut rows = connection
                .relation_rows(&relation, WorkflowSchema::decode)
                .await?;
            change(&mut rows)?;
            connection
                .replace_batches(&relation, vec![WorkflowSchema::batch(&rows)?])
                .await
        }
        .await;
        connection
            .finish_transaction(mutation, "Workflow save")
            .await
    }
}

/// Encoding version and payload of one graph or conversation.
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

struct WorkflowRow {
    id: String,
    name: String,
    graph: WorkflowJson,
    conversation: Option<WorkflowJson>,
    updated_ms: i64,
}

impl WorkflowRow {
    fn new(workflow: &WorkflowSpec, graph: WorkflowJson, updated_ms: i64) -> Self {
        Self {
            id: workflow.id().to_owned(),
            name: workflow.name().to_owned(),
            graph,
            conversation: None,
            updated_ms,
        }
    }
}

pub(crate) struct WorkflowSchema;

impl WorkflowSchema {
    pub(crate) fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("workflow_id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("graph_version", DataType::UInt32, false),
            Field::new("graph_json", DataType::Utf8, false),
            Field::new("conversation_version", DataType::UInt32, true),
            Field::new("conversation_json", DataType::Utf8, true),
            Field::new("updated_ms", DataType::Int64, false),
        ]))
    }

    fn batch(rows: &[WorkflowRow]) -> Result<RecordBatch, String> {
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.name.as_str()),
            )),
            Arc::new(UInt32Array::from_iter_values(
                rows.iter().map(|row| row.graph.version()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.graph.payload()),
            )),
            Arc::new(UInt32Array::from_iter(
                rows.iter()
                    .map(|row| row.conversation.as_ref().map(WorkflowJson::version)),
            )),
            Arc::new(StringArray::from_iter(
                rows.iter()
                    .map(|row| row.conversation.as_ref().map(WorkflowJson::payload)),
            )),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|row| row.updated_ms),
            )),
        ];
        RecordBatch::try_new(Self::schema(), columns)
            .map_err(|error| format!("building Workflow batch: {error}"))
    }

    fn decode(batch: &RecordBatch, row: usize) -> Result<WorkflowRow, String> {
        let columns = ArrowColumns::new(batch, "Workflow");
        Ok(WorkflowRow {
            id: columns.text("workflow_id", row)?.to_owned(),
            name: columns.text("name", row)?.to_owned(),
            graph: Self::json(&columns, row, "graph")?
                .ok_or_else(|| "Workflow graph is missing.".to_owned())?,
            conversation: Self::json(&columns, row, "conversation")?,
            updated_ms: columns
                .optional_timestamp("updated_ms", row)?
                .ok_or_else(|| "Workflow timestamp is missing.".to_owned())?,
        })
    }

    pub(crate) fn json(
        columns: &ArrowColumns<'_>,
        row: usize,
        field: &str,
    ) -> Result<Option<WorkflowJson>, String> {
        let version = columns.optional_unsigned(&format!("{field}_version"), row)?;
        let payload = columns.optional_text(&format!("{field}_json"), row)?;
        match (version, payload) {
            (None, None) => Ok(None),
            (Some(version), Some(payload)) => Ok(Some(WorkflowJson::new(
                version
                    .try_into()
                    .map_err(|_| "Workflow encoding version exceeds u32.".to_owned())?,
                payload.to_owned(),
            ))),
            _ => Err(format!(
                "Workflow {field} encoding and payload must be present together."
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow_assistant::WorkflowConversation;

    #[test]
    fn workflow_row_round_trip_keeps_graph_and_conversation_together() {
        let workflow = WorkflowSpec::new("workflow-one", "Lighting");
        let graph = WorkflowJson::new(2, serde_json::to_string(&workflow).unwrap());
        let mut row = WorkflowRow::new(&workflow, graph.clone(), 42);
        let empty = WorkflowSchema::batch(&[row]).unwrap();
        row = WorkflowSchema::decode(&empty, 0).unwrap();
        assert!(row.conversation.is_none());
        let conversation = WorkflowConversation::new(workflow.id())
            .persisted()
            .unwrap();
        row.conversation = Some(conversation.clone());
        let batch = WorkflowSchema::batch(&[row]).unwrap();
        let decoded = WorkflowSchema::decode(&batch, 0).unwrap();
        assert_eq!(decoded.id, workflow.id());
        assert_eq!(decoded.name, workflow.name());
        assert_eq!(decoded.graph, graph);
        assert_eq!(decoded.conversation, Some(conversation));
        assert_eq!(decoded.updated_ms, 42);
    }

    #[test]
    fn conversation_encoding_and_payload_cannot_be_half_present() {
        for (version, payload) in [(Some(4), None), (None, Some("{}"))] {
            let batch = RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("conversation_version", DataType::UInt32, true),
                    Field::new("conversation_json", DataType::Utf8, true),
                ])),
                vec![
                    Arc::new(UInt32Array::from(vec![version])),
                    Arc::new(StringArray::from(vec![payload])),
                ],
            )
            .unwrap();
            assert!(
                WorkflowSchema::json(&ArrowColumns::new(&batch, "Workflow"), 0, "conversation")
                    .is_err()
            );
        }
    }
}
