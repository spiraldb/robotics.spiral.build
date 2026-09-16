//! Bucketed message counts for one topic within one episode.
//!
//! The Data timeline asks for these per stream, so the query projects only
//! `log_time` and counts rows as they stream; no message payloads are read.
use super::messages::MessageQuery;
use crate::{
    StorageManager,
    db::{ArrowColumns, DatabaseSession},
};
use futures::StreamExt as _;
use spiral_robotics_common::datasets::{TopicCoverage, TopicCoverageRequest};

impl StorageManager {
    pub async fn topic_coverage(
        &self,
        request: TopicCoverageRequest,
    ) -> Result<TopicCoverage, String> {
        let bounds = request.bounds()?;
        self.require_topic_revision(request.read()).await?;
        let statement = MessageQuery::new(request.read()).statement(
            "log_time",
            &format!("AND log_time >= {} AND log_time < {}", bounds.0, bounds.1),
        )?;
        let mut connection = self.project_store().connect().await?;
        let mut stream = connection
            .query(statement)
            .await
            .map_err(|error| error.to_string())?;
        let mut coverage = TopicCoverage::empty(request.buckets());
        while let Some(batch) = stream.next().await {
            let batch = batch.map_err(|error| error.to_string())?;
            let columns = ArrowColumns::new(&batch, "topic coverage");
            for row in 0..batch.num_rows() {
                if let Some(log_time) = columns.optional_unsigned("log_time", row)? {
                    coverage.observe(log_time, bounds);
                }
            }
        }
        self.require_same_revision_after(request.read()).await?;
        Ok(coverage)
    }
}
