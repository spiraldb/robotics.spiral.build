//! Bucketed message counts for the streams of one episode.
//!
//! The Data timeline asks for every stream at once; each stream's query
//! projects only `log_time` and counts rows as they stream, so no message
//! payloads are read.
use super::EpisodeTopicQuery;
use crate::{
    StorageManager,
    db::{ArrowColumns, DatabaseSession},
};
use futures::StreamExt as _;
use spiral_robotics_common::datasets::{
    EpisodeCoverageRequest, StreamCoverage, TopicCoverage, TopicCoverageRequest,
};

/// Streams counted at the same time for one request; the rest queue. Each
/// count is one small query, so an episode's two dozen streams answer in a
/// few rounds rather than a few seconds each.
const CONCURRENT_COVERAGE_QUERIES: usize = 8;

impl StorageManager {
    /// Every stream of an episode, a few queries at a time, answered in
    /// request order. One stream that cannot be counted reports its error in
    /// place instead of failing the others.
    pub async fn episode_coverage(
        &self,
        request: EpisodeCoverageRequest,
    ) -> Result<Vec<StreamCoverage>, String> {
        request.validate()?;
        let outcomes = futures::stream::iter(request.streams().iter().cloned())
            .map(|stream| async move {
                match self.count_log_times(&stream).await {
                    Ok(coverage) => StreamCoverage::Ready { coverage },
                    Err(error) => StreamCoverage::Failed { error },
                }
            })
            .buffered(CONCURRENT_COVERAGE_QUERIES)
            .collect()
            .await;
        Ok(outcomes)
    }

    /// Bucket one stream's `log_time`s.
    async fn count_log_times(
        &self,
        request: &TopicCoverageRequest,
    ) -> Result<TopicCoverage, String> {
        let bounds = request.bounds()?;
        let read = request.read();
        let statement = EpisodeTopicQuery::for_sql(
            read.topic(),
            format!(
                "SELECT log_time FROM {} WHERE id = $1 AND log_time >= {} AND log_time < {}",
                read.source_sql(),
                bounds.0,
                bounds.1
            ),
        )?
        .statement;
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
        Ok(coverage)
    }
}
