//! Execute each source once and reuse its typed Arrow output through native temporary inputs.
use crate::sync::MutexRecovery;
use arrow_array::RecordBatch;
use arrow_schema::Schema;
use futures::{StreamExt, TryStreamExt};
use spiral_robotics_common::{
    relations::SpiralQuerySnapshot,
    sql::{ReadOnlyQuery, ReadOnlyQueryMode},
    workflow::{
        WorkflowSpec,
        comparison_inputs::{CapturedComparisonInputs, ComparisonInputRequest},
    },
};
use spiral_robotics_storage::{
    BatchIngestion, DatabaseSession, IngestMode, ProjectStore, RecordBatchStream, RelationName,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Instant,
};

pub(super) struct ComparisonInputs {
    store: Arc<ProjectStore>,
    snapshots: Mutex<BTreeMap<String, Arc<CapturedOutputs>>>,
}
struct CapturedOutputs {
    left: WorkflowOutput,
    right: Option<WorkflowOutput>,
}
struct WorkflowOutput {
    schema: Arc<Schema>,
    batches: Vec<RecordBatch>,
}
impl WorkflowOutput {
    async fn capture(store: &ProjectStore, workflow: &WorkflowSpec) -> Result<Self, String> {
        if !workflow.validate().valid() {
            return Err("Fix SQL validation errors before comparing.".into());
        }
        let mut connection = store.connect().await?;
        let stream = connection.execute_query(workflow.execution_sql()?).await?;
        let schema = stream.schema().clone();
        let batches = stream
            .try_collect()
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self { schema, batches })
    }
    fn stream(&self) -> RecordBatchStream {
        RecordBatchStream::new(
            self.schema.clone(),
            futures::stream::iter(self.batches.clone().into_iter().map(Ok)).boxed(),
        )
    }
    async fn preview(&self) -> Result<SpiralQuerySnapshot, String> {
        self.stream().into_preview(8).await
    }
    async fn bind(&self, connection: &mut impl DatabaseSession, name: &str) -> Result<(), String> {
        connection
            .ingest(BatchIngestion::new(
                RelationName::parse(name).map_err(|error| error.to_string())?,
                IngestMode::ReplaceTemporary,
                self.stream(),
            ))
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}
impl ComparisonInputs {
    pub(super) fn new(store: Arc<ProjectStore>) -> Self {
        Self {
            store,
            snapshots: Mutex::new(BTreeMap::new()),
        }
    }
    pub(super) async fn capture(
        &self,
        request: ComparisonInputRequest,
    ) -> Result<CapturedComparisonInputs, String> {
        let started = Instant::now();
        let (left, right) = futures::future::try_join(
            WorkflowOutput::capture(&self.store, request.left()),
            async {
                match request.right() {
                    Some(workflow) => WorkflowOutput::capture(&self.store, workflow)
                        .await
                        .map(Some),
                    None => Ok(None),
                }
            },
        )
        .await?;
        let previews = (
            left.preview().await?,
            match &right {
                Some(output) => Some(output.preview().await?),
                None => None,
            },
        );
        let id = uuid::Uuid::new_v4().to_string();
        let output = CapturedComparisonInputs::new(id.clone(), previews);
        self.snapshots
            .recover_lock()
            .insert(id, Arc::new(CapturedOutputs { left, right }));
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            "Comparison source capture timing"
        );
        Ok(output)
    }
    pub(super) async fn query(
        &self,
        id: &str,
        query: (String, usize),
    ) -> Result<SpiralQuerySnapshot, String> {
        ReadOnlyQuery::new(&query.0, ReadOnlyQueryMode::Workflow)
            .authorize()
            .map_err(|error| error.to_string())?;
        let inputs = self
            .snapshots
            .recover_lock()
            .get(id)
            .cloned()
            .ok_or("Comparison inputs expired. Compare again.")?;
        let started = Instant::now();
        let mut connection = self.store.connect().await?;
        inputs
            .left
            .bind(&mut connection, "comparison_input_left")
            .await?;
        if let Some(right) = &inputs.right {
            right
                .bind(&mut connection, "comparison_input_right")
                .await?;
        }
        let bound = started.elapsed();
        let output = connection
            .execute_query(query.0)
            .await?
            .into_preview_drained(query.1)
            .await?;
        tracing::info!(
            bind_ms = bound.as_millis(),
            query_ms = (started.elapsed() - bound).as_millis(),
            "Comparison query timing"
        );
        Ok(output.with_elapsed_ms(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)))
    }
    pub(super) fn release(&self, id: &str) {
        self.snapshots.recover_lock().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn complete_outputs_are_shared_by_independent_queries_until_released() {
        let inputs = ComparisonInputs::new(Arc::new(ProjectStore::workflow_test()));
        let values = (0..150)
            .map(|value| format!("({value})"))
            .collect::<Vec<_>>()
            .join(",");
        let left = WorkflowSpec::new("left", "Left").with_sql(format!(
            "SELECT value, INTERVAL '1 second' AS elapsed FROM (VALUES {values}) AS numbers(value)"
        ));
        let right = WorkflowSpec::new("right", "Right").with_sql("SELECT 149 AS value");
        let captured = inputs
            .capture(ComparisonInputRequest::new(left, Some(right)))
            .await
            .unwrap();
        assert_eq!(captured.left().rows().len(), 8);
        assert!(captured.left().truncated());
        let sql = "SELECT COUNT(*) AS n, SUM(value) AS total FROM comparison_input_left";
        let (one, two) = futures::future::try_join(
            inputs.query(captured.id(), (sql.into(), 1)),
            inputs.query(captured.id(), ("SELECT l.value, l.elapsed = INTERVAL '1 second' AS duration_preserved FROM comparison_input_left l JOIN comparison_input_right r ON l.value=r.value".into(), 1)),
        ).await.unwrap();
        assert_eq!(one.rows()[0]["n"], 150);
        assert_eq!(one.rows()[0]["total"], 11175);
        assert_eq!(two.rows()[0]["value"], 149);
        assert_eq!(two.rows()[0]["duration_preserved"], true);
        // A second comparison must have its own independent outputs.
        let another = inputs
            .capture(ComparisonInputRequest::new(
                WorkflowSpec::new("left", "Left").with_sql("SELECT 7 AS value"),
                None,
            ))
            .await
            .unwrap();
        assert_ne!(captured.id(), another.id());
        inputs.release(captured.id());
        assert!(
            inputs
                .query(captured.id(), (sql.into(), 1))
                .await
                .unwrap_err()
                .contains("expired")
        );
        assert_eq!(
            inputs
                .query(another.id(), (sql.into(), 1))
                .await
                .unwrap()
                .rows()[0]["n"],
            1
        );
        inputs.release(another.id());
        assert!(inputs.snapshots.recover_lock().is_empty());
    }
}
