//! Execute a workflow's native SQL to completion.
use spiral_robotics_common::{relations::SpiralQuerySnapshot, workflow::WorkflowSpec};
use spiral_robotics_storage::ProjectStore;
use std::{sync::Arc, time::Instant};

pub(super) struct WorkflowQuery {
    store: Arc<ProjectStore>,
}
impl WorkflowQuery {
    pub(super) fn new(store: Arc<ProjectStore>) -> Self {
        Self { store }
    }

    pub(super) async fn run(&self, workflow: &WorkflowSpec) -> Result<SpiralQuerySnapshot, String> {
        Self::validate(workflow)?;
        let started = Instant::now();
        let mut connection = self.store.connect().await?;
        let result = connection
            .execute_query(workflow.execution_sql()?)
            .await?
            .into_preview_drained(100)
            .await?;
        Ok(result.with_elapsed_ms(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)))
    }

    fn validate(workflow: &WorkflowSpec) -> Result<(), String> {
        workflow
            .validate()
            .errors()
            .first()
            .map_or(Ok(()), |error| Err(error.message().into()))
    }
}
