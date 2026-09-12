//! Submit native SQL and expose the engine's structured diagnostic plans.
use spiral_robotics_common::{
    relations::SpiralQuerySnapshot,
    workflow::{WorkflowSpec, document::WorkflowPlan},
};
use spiral_robotics_storage::ProjectStore;
use std::{sync::Arc, time::Instant};

pub(super) struct WorkflowQuery {
    store: Arc<ProjectStore>,
}
impl WorkflowQuery {
    pub(super) fn new(store: Arc<ProjectStore>) -> Self {
        Self { store }
    }

    pub(super) async fn plan(&self, workflow: &WorkflowSpec) -> Result<WorkflowPlan, String> {
        Self::validate(workflow)?;
        let mut connection = self.store.connect().await?;
        let result = connection
            .execute_query(format!("EXPLAIN (FORMAT JSON) {}", workflow.sql()))
            .await?
            .into_preview_drained(1)
            .await?;
        if result.truncated() {
            return Err("The native plan exceeds the display limit.".into());
        }
        let report = result
            .rows()
            .first()
            .and_then(|row| row.values().next())
            .and_then(serde_json::Value::as_str)
            .ok_or("Spiral did not return a JSON plan report.")?;
        WorkflowPlan::from_native_json(report)
    }

    pub(super) async fn run(&self, workflow: &WorkflowSpec) -> Result<SpiralQuerySnapshot, String> {
        Self::validate(workflow)?;
        let started = Instant::now();
        let mut connection = self.store.connect().await?;
        let result = connection
            .execute_query(workflow.sql())
            .await?
            .into_preview_drained(100)
            .await?;
        Ok(result.with_elapsed_ms(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)))
    }

    pub(super) async fn capabilities(&self) -> Result<SpiralQuerySnapshot, String> {
        let mut connection = self.store.connect().await?;
        let result = connection.execute_query(
            "SELECT * FROM spql.meta.scalar_functions() WHERE function_set LIKE 'spql.ai.%' ORDER BY function_set, overload"
        ).await?.into_preview_drained(100).await?;
        if result.truncated() {
            return Err("The native AI catalog exceeds the capability response limit.".into());
        }
        Ok(result)
    }

    fn validate(workflow: &WorkflowSpec) -> Result<(), String> {
        workflow
            .validate()
            .errors()
            .first()
            .map_or(Ok(()), |error| Err(error.message().into()))
    }
}
